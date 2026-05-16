# 02 — Evaluation Semantics

This document specifies the **canonical evaluation algorithm** that every
Stitchd SDK must implement. The Rust reference implementation in `sdks/rust/`
delegates to `stitchd-core` internally, but other languages MUST re-implement
the algorithm in their own runtime — the wire-level definition snapshot is
self-contained (see `sdk_service.proto` and JSON schemas), and the algorithm
here is precise enough to implement from this document alone.

## The `Context`

Every evaluation requires a `Context`:

```
Context {
  type:                String,                  // e.g. "user", "device", "org"
  key:                 String,                  // unique identifier within this type
  parameters:          Map<String, ParamValue>, // typed attributes
  private_parameters:  List<String>,            // attribute names to redact from event payloads
}

ParamValue = Int(i64) | Double(f64) | String(s) | Bool(b) | Semver(major.minor.patch)
```

`(type, key)` is the LRU cache key for list-segment membership.

## Evaluation Entry Points

Every SDK MUST expose two evaluation operations:

```
evaluate(requests: [EvalRequest])                  -> [EvalResult]
evaluate_with_reasoning(requests: [EvalRequest])   -> [EvalResultWithReasoning]
```

Both accept a **batch** of `EvalRequest { flag_key, context }`. Each request in
the batch is evaluated independently against the local snapshot — no
cross-request state.

The non-reasoning variant returns minimal output (variant only). The reasoning
variant returns the variant **plus** a `ReasoningTrace` describing which rule
matched, which segment(s) were consulted, and how the rollout bucket was
computed.

## Definition Snapshot Shape

The snapshot returned by `SyncDefinitions` contains:

```
DefinitionsSnapshot {
  flags:           Map<flag_key, FlagDefinition>
  rule_segments:   Map<segment_id, RuleSegmentDefinition>     // full rule condition tree
  list_segments:   Map<segment_id, ListSegmentMetadata>       // segment_id + context_type + name (NO entries)
}

FlagDefinition {
  key, name, value_type ("bool"|"int"|"double"|"string"|"json"),
  enabled: bool,
  variants: [{ key, value }],
  default_rule: Rule,
  rules: [Rule],          // evaluated in order; first match wins
  project_id, environment_id, salt
}

Rule {
  rule_id, rule_name?,
  condition: ConditionTree,
  variant_assignment: SpecificVariant(key) | PercentageRollout([{variant_key, percentage}])
}
```

## Condition Tree Primitives

Every SDK must support these condition nodes (the wire format is JSON inside
`Rule.condition`; see `schemas/condition_tree.schema.json` for the formal
schema). Nodes:

| Node | Shape | Semantics |
|---|---|---|
| `Eq` | `{context_type, param, value}` | `context.parameters[param] == value` (after type coercion) |
| `Neq` | same | logical complement of `Eq` |
| `Lt`, `Lte`, `Gt`, `Gte` | `{context_type, param, value}` | numeric or semver comparison; non-numeric param → `false` |
| `Contains` | `{context_type, param, substr}` | `param` is a string AND contains `substr` |
| `StartsWith`, `EndsWith` | same | string prefix / suffix match |
| `SemverGte`, `SemverLt`, etc. | `{context_type, param, value}` | parse both sides as semver; non-semver → `false` |
| `InList` | `{context_type, param, values: [String]}` | `param` matches any value in `values` |
| `Matches` | `{context_type, param, pattern: String}` | wildcard match (`*` as wildcard); use `strip_suffix("*")` for simple prefix |
| `InSegment` | `{segment_id}` | true iff context is a member of the named segment (see *Segment Resolution* below) |
| `NotInSegment` | `{segment_id}` | logical complement of `InSegment` |
| `And` | `{children: [ConditionTree]}` | all children must be true; short-circuit on first `false`; **MissingContext / MissingParameter sub-result treated as `false`** |
| `Or` | `{children: [ConditionTree]}` | any child true; short-circuit on first `true`; MissingContext/MissingParameter treated as `false` |
| `Not` | `{child: ConditionTree}` | logical complement; MissingContext bubbles up unchanged |

`context_type` selector: every leaf condition is anchored to a specific context
type (e.g. `"user"`, `"device"`). If the caller's `Context.type` doesn't match,
the condition resolves to `MissingContext` — which `And`/`Or` treat as `false`
but `Not` preserves.

## Segment Resolution

When a condition tree contains `InSegment(segment_id)` or `NotInSegment(segment_id)`:

1. Look up the segment in the definition snapshot.
2. **If `rule_segments[segment_id]` exists** (rule-based segment): recursively
   evaluate the segment's condition tree against the same `Context`.
3. **If `list_segments[segment_id]` exists** (list-based segment): consult the
   LRU cache for `(context.type, context.key)`:
   - **Hit:** read `membership[segment_id]` (default `false` if not in map).
   - **Miss:** synchronously fetch via `POST /v1/sdk/segments/list:batch` with a
     single query containing `[{context_type, key, segment_ids: [segment_id]}]`,
     insert the response into the LRU, then read membership.
   - **Miss fetch fails:** treat as `false` for this evaluation. Log a warning.
     Do not insert anything into the LRU. The next evaluation for the same
     `(type, key)` will retry the fetch.
4. **If segment is unknown** (not in snapshot at all): treat as `false`. Log a
   warning. The most likely cause is a stale snapshot — the next definition
   poll should reconcile.

## Evaluation Algorithm

For each `(flag_key, context)` request:

1. Look up `flag_def` in the local definition snapshot.
   - **If missing:** return `EvalResult { variant: default_for_type(flag.value_type), outcome: "flag_not_found" }`. Still emit an event with `matched_rule_id = null` and `outcome = "flag_not_found"`.
2. **If `flag_def.enabled == false`:** return the disabled-state default variant.
   Do not iterate rules. Emit an event with `outcome = "disabled"`.
3. **Iterate `flag_def.rules` in order.** For each rule:
   - Evaluate `rule.condition` against the context (see Condition Tree Primitives + Segment Resolution above).
   - If the result is `true`: assign the variant using `rule.variant_assignment`. Stop iteration.
4. **If no rule matched:** apply `flag_def.default_rule.variant_assignment`.

### Variant Assignment

- `SpecificVariant(key)`: return `variants[key].value`.
- `PercentageRollout([{variant_key, percentage}])`: compute the bucket (see Hash Function below); walk allocations in order, accumulating percentages until the cumulative crosses the bucket; return that variant. Percentages MUST sum to exactly `1000` (100.0% × 10 for 0.1% granularity).

## Hash Function (Percentage Rollout Bucket)

The bucket for a `(context, flag)` pair is computed deterministically. All SDKs
MUST produce the same bucket for the same inputs.

**Inputs (concatenated as UTF-8 bytes, separated by ASCII `0x1F` Unit Separator):**

```
flag.salt           // server-supplied per-flag salt (32-char hex string)
0x1F
flag.project_id     // UUID string, lowercase, no braces
0x1F
flag.environment_id // UUID string, lowercase, no braces
0x1F
flag.key
0x1F
context.type
0x1F
context.key
```

**Algorithm:** MurmurHash3 x64_128 with seed `0` (matching `stitchd-core`).
Take the **first 8 bytes** of the 128-bit output, interpret as **big-endian
uint64**, and compute `bucket = uint64 % 1000`.

Granularity: 0.1% (1 in 1000). Bucket is in range `[0, 999]`.

**Reference vectors** (used in `fixtures/`):

| salt | project_id | env_id | flag_key | ctx_type | ctx_key | expected_bucket |
|---|---|---|---|---|---|---|
| `00000000000000000000000000000000` | `00000000-0000-0000-0000-000000000000` | `00000000-0000-0000-0000-000000000000` | `flag1` | `user` | `alice` | (TBD: filled in by Phase 1 Task 6 fixtures with reference hash) |

(Concrete vectors will be generated by the Rust SDK's reference hash function
and stored in `fixtures/` so other languages can validate against them.)

## Reasoning Trace Shape

```
ReasoningTrace {
  outcome:               "matched" | "default_rule" | "disabled" | "flag_not_found"
  matched_rule:          { rule_id, rule_name, condition_summary } | null
  segment_evaluations:   [
    { segment_id, segment_type: "rule" | "list", matched: bool,
      source: "snapshot" | "lru_hit" | "lru_miss_fetched" | "lru_miss_failed" }
  ]
  rollout:               { bucket: int, allocation: [{variant_key, percentage}] } | null
  evaluated_at:          RFC3339_timestamp
}
```

The reasoning variant is **not free** — it adds bookkeeping on the hot path. Use
it for debugging, admin "test this flag" preview UIs, or selective sampling, not
for every production evaluation.

## Determinism Guarantees

- Same `(flag_def, context)` → same `variant`. **Always**, on every SDK instance
  in the world, until either the flag definition changes or the percentage
  rollout's salt changes.
- List-segment membership is the only piece of state that can drift: the LRU
  may serve stale membership between background refreshes (default staleness
  budget: 60s). This is intentional — see `03-caching.md`.

## Batch Semantics

A batch passed to `evaluate()` is evaluated in submission order. There is no
short-circuit on error: every request gets a result, and failures (unknown flag,
list-segment fetch error) are recorded in the result rather than thrown.

## Error Handling Per-Request

| Condition | Result | Event emitted? |
|---|---|---|
| Flag not found in snapshot | Default variant for `value_type` | Yes (`outcome = "flag_not_found"`) |
| List-segment fetch on miss fails | Treat referenced list-segments as `false`; continue rule iteration | Yes (`outcome = "matched"` or `"default_rule"`; reasoning's `segment_evaluations[].source = "lru_miss_failed"`) |
| Definition snapshot stale (gateway unreachable for > N intervals) | Continue serving last-known snapshot | Warn-logged; no eval-time error |
| Snapshot empty (only possible before first sync — but `init` blocks on that, so this should never happen) | Implementation-defined; SHOULD panic / throw a programming error | No event |
