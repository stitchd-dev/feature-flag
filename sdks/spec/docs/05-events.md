# 05 — Events

The SDK emits exactly one `FlagEvaluationEvent` per evaluated flag, regardless
of which entry point (`evaluate` or `evaluate_with_reasoning`) was used. Events
are queued in memory, batched, and flushed to the gateway via REST.

## Event Schema

Formal schema: `sdks/spec/schemas/flag_evaluation_event.schema.json`.

```
FlagEvaluationEvent {
  flag_key:            String       // required
  environment_id:      String (UUID) // required — resolved from the SDK key
  variant_key:         String       // required — the variant key that was returned
  context_type:        String       // required
  context_key:         String       // required
  evaluated_at:        String (RFC3339, UTC, millisecond precision)
  matched_rule_id:     String (UUID) | null   // null if outcome != "matched"
  outcome:             "matched" | "default_rule" | "disabled" | "flag_not_found"
  reasoning_included:  Bool         // whether the caller asked for reasoning (NOT whether reasoning is in this event payload)
  context_parameters:  Object | null  // see *Parameter Redaction* below
}
```

Notes:

- `environment_id` is supplied by the gateway when the events batch is forwarded —
  the SDK does NOT know its own environment id at the wire level. **Implementations
  SHOULD omit this field from the SDK-side payload; the gateway will inject it.**
  (TODO: confirm in OpenAPI spec — currently the SDK leaves the field absent
  and the gateway populates it before forwarding to `IngestSdkEvalLog`.)
- `evaluated_at` is the SDK's local clock at the moment evaluation returned.
  Backend storage uses ingestion time as well; ordering across SDK instances is
  best-effort.

## Parameter Redaction

Some attributes on the `Context` may be sensitive (PII, internal identifiers).
The caller supplies `context.private_parameters` listing attribute names to redact.

When emitting an event:

- If `private_parameters` is empty: emit all attributes under `context_parameters`.
- If `private_parameters` is non-empty: omit those keys entirely from the emitted
  `context_parameters` map. (Do NOT include them with a sentinel like `"[REDACTED]"` —
  the spec is **omit**.)
- If the entire `context_parameters` field would be empty after redaction: emit `null`.

The SDK MUST NOT serialize the `Context.private_parameters` list itself into
the event — that's metadata, not payload.

## Batching and Flush

| Trigger | Behaviour |
|---|---|
| `event_flush_interval` elapsed (default 5s) | Flush whatever is currently buffered (may be 0 — skip the network call if empty) |
| Buffer reaches `event_batch_size` items (default 100) | Flush immediately; restart the interval timer |
| `shutdown()` called | Final drain — flush all remaining events synchronously before returning |

Each flush is a single `POST /v1/sdk/events:batch` request.

## Wire Payload

```
POST /v1/sdk/events:batch
Content-Type: application/json
x-sdk-key: <key>

{
  "events": [
    { /* FlagEvaluationEvent (without environment_id) */ },
    ...
  ]
}
```

Gateway response: `202 Accepted` with no body. The gateway forwards the batch
to `stitchd-flag-service::IngestSdkEvalLog`, which adapts each event to an
`EvalLogRow` and hands it to the existing eval-log-writer pipeline that already
serves server-side evaluations. From the storage layer's perspective, SDK
evaluations are indistinguishable from gateway-internal evaluations.

## Delivery Guarantees

**At-least-once.** The SDK retries failed flushes; in extreme cases a batch may
be delivered twice. The flag evaluation log is downstream-deduplicated by
`(flag_key, environment_id, context_type, context_key, evaluated_at)` if your
analytics queries care about exact-once semantics.

## What Happens When the Gateway is Unreachable

| State | Behaviour |
|---|---|
| Single flush fails | Re-enqueue the batch at the head of the buffer; retry on next tick |
| Buffer is full and new events arrive | Drop **oldest** events (back-pressure on the producer is NOT acceptable — `evaluate()` must never block on event emission). Log a warning each time eviction happens (rate-limited to avoid log floods). |
| Sustained unreachability | Continue dropping oldest events to keep buffer bounded; the application continues evaluating normally with stale events lost |
| Reconnection | Resume normal flushing; no special "catch-up" mode |

Buffer capacity (configurable, default 10× `event_batch_size` = 1000) caps
worst-case memory for unflushed events.

## What the SDK Does NOT Do

- The SDK does NOT emit application-defined events (custom metric events for
  experiments). Those are emitted by the application directly via the
  `stitchd-event-service` ingestion gRPC.
- The SDK does NOT batch events across multiple SDK instances — each instance
  has its own buffer.
- The SDK does NOT persist unflushed events across instance restarts. A crash
  with pending events loses those events.
