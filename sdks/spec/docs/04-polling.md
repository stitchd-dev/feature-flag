# 04 — Polling

The SDK runs **two independent polling loops** as background tasks. Both are
started at `init()` and stopped at `shutdown()`.

## Loop 1 — Definition Sync

| Property | Value |
|---|---|
| Endpoint | gateway gRPC `sdk.v1.SdkService/SyncDefinitions` (unary) |
| Default interval | 30 seconds |
| Config key | `definition_poll_interval` |
| Request payload | empty (env is resolved from `x-sdk-key`) |
| Response payload | full `DefinitionsSnapshot` (no deltas) |
| On success | Atomically swap the local snapshot |
| On failure | Log warning; back off (see below); retry next tick |

### First Sync (Blocking)

`init()` MUST issue the first sync **synchronously** and only return success if
it produced a non-empty snapshot. This ensures `evaluate()` is never called
against an empty snapshot.

### Backoff on Failure

When a poll fails (network error, gRPC `Unavailable`, etc.):

- First failure: log at INFO; wait one interval, retry.
- 2nd–5th consecutive failures: log at WARN; double the wait each time
  (`interval × 2^N`), capped at `5 × interval`.
- 6th+ consecutive failure: log at ERROR; continue retrying at the capped
  interval. Never give up — the application continues serving evaluations from
  the last-known snapshot.

| Failure # | Wait before retry |
|---|---|
| 1 | 1 × interval |
| 2 | 2 × interval |
| 3 | 4 × interval |
| 4 | 5 × interval (capped) |
| 5+ | 5 × interval |

On the next success, the counter resets to 0.

### gRPC Metadata

Every request MUST include:

```
x-sdk-key: <the SDK key>
```

The SDK MUST NOT include `x-env-id` — that's the gateway's responsibility.

### Failure Modes the Loop Must Handle

| Status | Loop response |
|---|---|
| `Ok` with valid snapshot | Swap; reset failure counter |
| `Ok` with empty snapshot (no flags, no segments) | Swap (legitimate — env may have no flags configured); reset counter |
| `Unauthenticated` (401 / gRPC code 16) | Log error; DO NOT swap. Most likely SDK key revoked. Continue retrying — admin may un-revoke. |
| `Unavailable` (gRPC code 14), connection refused, timeout | Treat as transient. Back off. |
| Other (5xx, malformed response) | Log error; back off; do not swap. |

## Loop 2 — LRU Refresh

| Property | Value |
|---|---|
| Endpoint | gateway REST `POST /v1/sdk/segments/list:batch` |
| Default interval | 60 seconds |
| Config key | `list_segment_refresh_interval` |
| Request payload | One query per resident LRU entry (filtered to flag-referenced segments) |
| Response payload | Membership matrix per query |
| On success | Update each LRU entry in place (no recency promotion) |
| On failure | Log warning; back off; retry next tick. Entries are NOT evicted on failure — last-known membership continues to serve. |

### Behaviour When LRU is Empty

If no entries are currently resident (e.g. SDK just started; no evaluations
have hit list-segment rules yet), the refresh loop skips the network request
entirely — there is nothing to refresh.

### Behaviour When No List-Segments Are Referenced

If the current definition snapshot contains zero list-segments referenced by
any flag rule, the refresh loop skips the network request — there's nothing
useful to fetch.

### Batch Size

There is no per-batch size limit imposed by the SDK; the entire LRU is sent in
one request. The gateway may impose a server-side limit (current cap: TBD; if
the SDK ever sees a `400 BatchTooLarge` response, it should split the batch in
half and retry — but this fallback is OPTIONAL and is a future enhancement).

### Same Backoff Policy as Loop 1

Identical exponential backoff schedule. Failures here are not fatal — they just
mean staleness grows beyond the budget until connectivity recovers.

## Loop 3 — Event Flush

Specified in detail in `05-events.md`. Summary:

| Property | Value |
|---|---|
| Endpoint | gateway REST `POST /v1/sdk/events:batch` |
| Default flush interval | 5 seconds |
| Default batch size | 100 events |
| Trigger | `interval elapsed` OR `buffer reaches batch_size`, whichever first |
| On shutdown | Final drain — flush all pending events before stopping |

## Coordination Between Loops

The three loops are **independent** — none of them block the others. If
definition sync is wedged, the event flush continues; if event flush is
wedged, definition polling continues; etc.

The LRU refresh loop **reads** the definition snapshot to compute the referenced-segments
filter. This read is lock-free (atomic snapshot pointer), so a slow refresh
loop never blocks the definition swap.

## Why Polling and Not Streaming?

Server-streaming gRPC, SSE, and WebSocket push were all considered. For this
revision:

- Polling is simpler to implement and debug.
- Polling makes connection state implicit — there is no "is the stream alive?"
  bookkeeping.
- Polling at 30s is acceptable for the current usage profile (admin-driven
  changes propagate within at most 30s; SDK key revocations within at most 60s).

Streaming is a future enhancement track, especially if change-frequency or
latency-to-propagation requirements tighten.
