# SDK APIs

REST endpoints called by the embedded SDK (`sdks/rust/`) over `x-sdk-key`
auth. The streaming definition-sync RPC lives on the gRPC port and is
documented in [Internal gRPC](./grpc.md).

## Auth

Every request carries an environment-scoped SDK key in the `x-sdk-key`
header:

```
x-sdk-key: stk_live_abc123...
```

The gateway validates the key through `AuthService::ValidateCredential`,
resolves the environment, then proxies to the right service with the
resolved env_id stamped onto the gRPC metadata as `x-env-id`. Backend
services trust `x-env-id` and never see the raw SDK key.

Bearer tokens are **rejected** on this surface — even a valid admin JWT
gets `401 invalid_sdk_key` if it arrives without `x-sdk-key`. SDK
telemetry must only come from embedded SDKs, never the admin browser
session. (The one exception is `POST /v1/admin/events/track`, the
test-event widget on the EventDetail page; it lives on the JWT tier and
is documented under [Admin & Management APIs](./admin-api.md).)

A missing key returns `{ "error": "missing_sdk_key" }` with `401`; a key
the auth service rejects returns `{ "error": "invalid_sdk_key" }`.

## Endpoint summary

| Method | Path                            | Auth        | Purpose                                                            |
|--------|---------------------------------|-------------|--------------------------------------------------------------------|
| POST   | `/v1/sdk/segments/list:batch`   | `x-sdk-key` | Batched list-segment membership check across many `(ctx, segments)` pairs. |
| POST   | `/v1/sdk/events:batch`          | `x-sdk-key` | Batched flag-evaluation log ingest (at-least-once, `202 Accepted`).         |
| POST   | `/v1/events/track`              | `x-sdk-key` | The SDK `Client::track()` admin-defined-event API (5 MiB cap, per-env quota). |

> The streaming `SdkService::SyncDefinitions` RPC on port `50050` carries
> flag + segment definitions to the SDK in real time. It is a gRPC-only
> surface — see [Internal gRPC](./grpc.md).

## `POST /v1/sdk/segments/list:batch`

Batched list-membership check. The SDK collects a list of `(context_type,
context_key, segment_ids[])` tuples and asks the gateway which of the
named segments each context belongs to. Forwards to
`SegmentationSdkBackendService.BatchCheckListMembership`.

```bash
curl -X POST http://localhost:8080/v1/sdk/segments/list:batch \
  -H 'x-sdk-key: stk_live_abc123' \
  -H 'content-type: application/json' \
  -d '{
    "queries": [
      {
        "context_type": "user",
        "context_key":  "user-42",
        "segment_ids":  ["e5a4-...", "1b2c-..."]
      }
    ]
  }'
```

```json
{
  "results": [
    {
      "context_type": "user",
      "context_key": "user-42",
      "memberships": {
        "e5a4-...": true,
        "1b2c-...": false
      }
    }
  ]
}
```

The SDK uses this exclusively for **list-based segments** (explicit
allow-/deny-lists). Rule-based segments are evaluated locally from the
synced segment definition; only list segments require a round-trip
because the list bodies are too large to stream.

## `POST /v1/sdk/events:batch`

Batched ingest of flag-evaluation log rows. The SDK records every flag
evaluation locally and flushes a batch to this endpoint periodically.
Forwards to `FlagSdkBackendService.IngestSdkEvalLog`.

```bash
curl -X POST http://localhost:8080/v1/sdk/events:batch \
  -H 'x-sdk-key: stk_live_abc123' \
  -H 'content-type: application/json' \
  -d '{
    "events": [
      {
        "flag_key":      "new-checkout",
        "flag_id":       "9c3a-...",
        "variant_key":   "treatment",
        "context_type":  "user",
        "context_key":   "user-42",
        "evaluated_at":  "2026-05-22T08:30:00Z",
        "matched_rule_id": "f1a2-...",
        "outcome":       "match",
        "reasoning_included": false,
        "context_parameters": { "plan": "pro", "country": "US" }
      }
    ]
  }'
```

Response: empty body, `202 Accepted`. Delivery is **at-least-once** —
clients should not infer that backend acceptance equals exactly-once
persistence. Any `environment_id` field the SDK puts in the body is
**ignored**; the gateway authoritatively stamps it from the resolved
`SdkContext`.

Unsupported `context_parameters` values (`null`, arrays, objects) are
silently dropped per-event rather than failing the whole batch.

## `POST /v1/events/track`

The SDK `Client::track()` API for admin-defined custom events (the
event-definitions surface described in [Admin & Management
APIs](./admin-api.md)). Each event in the batch is validated against the
matching `EventDefinition` by `stitchd-analytics-service` — per-event
failures populate `rejected[]` without failing the whole batch.

Special semantics:

- **5 MiB body limit.** Larger requests get `413 Payload Too Large` from
  the `DefaultBodyLimit` layer before the handler runs.
- **Per-env rate limit.** A token-bucket layer caps the per-environment
  throughput at `STITCHD_EVENT_QUOTA_PER_SEC` (default `1000`) events/s.
  Requests above the cap return `429 Too Many Requests`. The limit is
  applied **only** to `/v1/events/track`, not to the SDK telemetry batch
  endpoints above.

```bash
curl -X POST http://localhost:8080/v1/events/track \
  -H 'x-sdk-key: stk_live_abc123' \
  -H 'content-type: application/json' \
  -d '{
    "events": [
      {
        "event_key":    "checkout_completed",
        "context_type": "user",
        "context_key":  "user-42",
        "value":        { "kind": "float", "value": 49.99 },
        "properties":   { "currency": "USD" },
        "occurred_at":  "2026-05-22T08:30:00Z"
      }
    ]
  }'
```

```json
{
  "accepted_count": 1,
  "rejected": []
}
```

`value` is a discriminated union — `kind` is one of `unit`, `bool`,
`int`, `float`, `string`. `unit` is required when the matching event
definition's value type is `Unit`; for any other type, supplying the
wrong `kind` puts the event into `rejected[]` with
`reason: "value_type_mismatch"`.

`properties` is a flat `string -> string` map used for downstream metric
filters; deeper structures must be flattened client-side.

`occurred_at` is RFC 3339 client wall-clock; it defaults to ingestion
time when omitted.

### Per-event rejection reasons

Each `rejected[]` entry has shape `{ "event_key": ..., "reason": ... }`
where `reason` is one of:

| Reason                  | Meaning                                                        |
|-------------------------|----------------------------------------------------------------|
| `unknown_event_key`     | No event definition with that key exists in the environment.   |
| `archived_event_key`    | An event definition exists but was archived.                   |
| `value_type_mismatch`   | The `value.kind` does not match the definition's value type.   |
| `missing_value`         | The definition requires a value but none was supplied.         |
| `invalid_occurred_at`   | `occurred_at` could not be parsed as RFC 3339.                 |

The whole batch still returns `202 Accepted`. The SDK is expected to log
the rejections, not retry the rejected entries.

## Error envelope

Outside the per-event rejection path, errors return the standard
gateway envelope:

```json
{ "error": "invalid_sdk_key", "message": "SDK key is invalid, revoked, or unknown" }
```

The mapping from upstream gRPC status to HTTP follows
`crate::error::GatewayError` — see [Gateway](./overview.md#error-envelope).
