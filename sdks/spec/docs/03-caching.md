# 03 — Caching

The SDK maintains **two distinct caches**. Both are entirely in-process. Nothing
is persisted to disk between SDK instance lifetimes.

## Cache 1 — Definition Snapshot

A single in-memory snapshot containing **all** flag definitions, rule-based
segment definitions, and list-segment metadata for the SDK key's environment.

### Lifecycle

| Event | Effect |
|---|---|
| `init()` | First definition sync (blocking). On success, snapshot is populated. On failure, `init` returns error. |
| Definition poll loop (every `definition_poll_interval`, default 30s) | Issues unary gRPC call to `SdkService.SyncDefinitions`. On success, atomically swap the snapshot. On failure, log warning; retry next interval with exponential backoff. |
| `shutdown()` | Background loop stops. Snapshot retained until SDK instance dropped. |

### Read Pattern

Every `evaluate()` call reads from the snapshot. Implementations:

- MUST support concurrent reads from many threads.
- MUST NOT block reads on the polling task's swap.
- SHOULD use a lock-free pattern (atomic pointer swap / copy-on-write) so reads
  never wait. Language idioms:
  - Rust: `arc_swap::ArcSwap<DefinitionSnapshot>`
  - Java: `AtomicReference<DefinitionSnapshot>`
  - JS / Python / Go: language-appropriate equivalent

### Update Pattern

The polling task ALWAYS produces a full snapshot — there are no deltas. On
successful response:

1. Build the new `DefinitionSnapshot` from the response.
2. Atomically replace the current snapshot.
3. (Optional, recommended) compute the diff against the previous snapshot for
   logging — but the swap itself is unconditional.

The previous snapshot is discarded only after all in-flight `evaluate()` calls
that captured it have completed (managed by the underlying smart pointer / GC).

## Cache 2 — List-Segment Membership LRU

A bounded LRU keyed on **`(context_type, context_key)`** storing a
`Map<segment_id, bool>` — the membership of that specific context across all
list-based segments currently referenced by any flag rule.

### Why LRU?

A typical server-side SDK sees a high number of distinct contexts but
disproportionately re-encounters a small "hot" subset. Bounded LRU caps memory
while exploiting locality. The bound is configurable
(`lru_max_entries`, default 10,000).

### Cache Key Granularity

```
key   = (context_type, context_key)         -- the LRU entry
value = Map<segment_id, bool>               -- one bool per list-segment in the snapshot
```

A single entry covers **every** list-segment the SDK currently cares about
(filtered to "currently referenced by a flag rule" — see *Filtering* below).

### Population (Lazy on Miss)

Entries are created **only** when `evaluate()` encounters a context that has
not yet been seen.

```
on evaluate(context, flag) with flag.condition referencing list_segment_X:
    membership = LRU.get((context.type, context.key))
    if membership is None:
        # Cache miss — synchronous fetch
        response = POST /v1/sdk/segments/list:batch
                   body: { queries: [{
                     context_type: context.type,
                     key:          context.key,
                     segment_ids:  all_currently_referenced_list_segment_ids
                   }]}
        membership = response.results[0].memberships
        LRU.put((context.type, context.key), membership)
    return membership[list_segment_X]
```

The on-miss fetch requests memberships for **all** list-segments currently
referenced (not just the one being checked). This warms the entry for any other
list-segment that subsequent rules in the same evaluation might reference,
avoiding multiple network round-trips for a single `evaluate()` call.

### Refresh (Background Polling)

A background task runs every `list_segment_refresh_interval` (default 60s):

1. Snapshot the current list of LRU keys.
2. Build a single batch request: one query per LRU key, with `segment_ids` set
   to all currently-referenced list-segments (see *Filtering*).
3. `POST /v1/sdk/segments/list:batch`.
4. For each result, update the LRU entry **in place** (do not promote
   recency — the refresh should not interfere with LRU eviction order).
5. On failure: log warning, do not evict, retry next interval.

The refresh task **never adds new entries** — it only refreshes existing ones.
New entries are added exclusively by the lazy-on-miss path above.

### Filtering — Only Referenced Segments

The refresh task and the on-miss fetch both filter the `segment_ids` list to
those segments referenced by at least one rule in the current definition
snapshot. Segments that are defined but unused by any flag are **not** polled —
this avoids wasted bandwidth for orphan or test segments.

Computation: walk every `Rule.condition` in every `FlagDefinition`, collect all
`InSegment(segment_id)` / `NotInSegment(segment_id)` references, intersect with
`snapshot.list_segments.keys()`. Recompute on every snapshot swap.

### Eviction

Standard LRU: when capacity exceeded, evict the least-recently-used entry.
"Used" means a successful `LRU.get()` (i.e. an `evaluate()` call resolved its
membership from this entry). Background refresh writes do NOT count as use.

### Staleness Budget

Between refresh ticks, a context's membership can be stale for at most
`list_segment_refresh_interval` (default 60s). This is the **acceptable
staleness** for membership data. If your use case cannot tolerate 60s of
staleness, lower the interval (at the cost of more network traffic).

If a hard guarantee is required (e.g. compliance: "this context must NEVER see
this flag after revocation"), use a rule-based segment, not a list-based
segment — rule-based segments are evaluated against the live definition
snapshot, which refreshes every `definition_poll_interval` (default 30s) and
contains no per-context state.

### Invalidation

There is no explicit per-key invalidation API. The next refresh tick reconciles
all resident keys. An admin-driven membership change becomes visible within at
most `list_segment_refresh_interval` for currently-cached contexts, or
immediately for fresh contexts (lazy fetch goes straight to the gateway).

## Shared Invariants

- Both caches MUST be cleared on `shutdown()`.
- Neither cache spans SDK instance lifetimes (no disk persistence).
- Both caches MUST be safe for concurrent reads from `evaluate()` calls while
  background tasks write.
