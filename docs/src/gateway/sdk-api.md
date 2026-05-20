# SDK APIs

REST endpoints consumed by the stitchd SDK (`stitchd-sdk-rust`). All SDK routes authenticate
via the `x-sdk-key` header — no JWT required.

## Auth Model

Include the environment's SDK key in every request:

```
x-sdk-key: sdk_live_abc123...
```

SDK keys are scoped to a single environment. A request with an invalid or missing key returns `401 Unauthorized`.

## Endpoints

### Evaluate Flag

```
POST /v1/environments/{environment_id}/evaluate
```

Evaluate a feature flag for a context.

**Request body:**

```json
{
  "flag_key": "my-flag",
  "context_type": "user",
  "context_key": "user-123",
  "attributes": {
    "plan": "pro",
    "country": "US"
  }
}
```

**Response:**

```json
{
  "flag_key": "my-flag",
  "variant_key": "treatment",
  "is_enabled": true
}
```

---

### Ingest Event

```
POST /v1/environments/{environment_id}/events
```

Record a single metric event.

**Request body:**

```json
{
  "metric_key": "button_click",
  "context_type": "user",
  "context_key": "user-123",
  "value": true,
  "timestamp_ms": 1714000000000
}
```

`value` is optional and can be a boolean, integer, or float. `timestamp_ms` defaults to server-received time if omitted.

**Response:**

```json
{
  "accepted_count": 1,
  "rejected_keys": []
}
```

---

### Batch Ingest Events

```
POST /v1/environments/{environment_id}/events/batch
```

Record multiple events in a single request.

**Request body:**

```json
{
  "events": [
    { "metric_key": "page_view", "context_type": "user", "context_key": "u1" },
    { "metric_key": "purchase", "context_type": "user", "context_key": "u1", "value": 49.99 }
  ]
}
```

---

### Batch Segment Membership Check (new SDK surface)

```
POST /v1/sdk/segments/list:batch
```

Check membership for multiple (segment, context) pairs in one call. Available to the
`stitchd-sdk-rust` client; uses `sdk_auth_middleware`.

---

### Batch SDK Event Ingestion (new SDK surface)

```
POST /v1/sdk/events:batch
```

Batch flag evaluation event ingestion (202 Accepted). Part of the new clean SDK surface
under `/v1/sdk/`, separated from the legacy routes.

---

### Track Admin-Defined Events (`events_metrics_20260519`)

```
POST /v1/events/track
```

Batch ingestion of admin-defined events emitted by the SDK `Client::track()` API.
Each event is validated against its `EventDefinition` (key, typed value, archived
flag) by `stitchd-analytics-service`; per-event failures populate `rejected[]`
without failing the whole batch.

**Request body** (5 MiB max; larger requests get `413 Payload Too Large`):

```json
{
  "events": [
    {
      "event_key": "checkout_completed",
      "context_type": "user",
      "context_key": "user-123",
      "value": { "kind": "float", "value": 49.99 },
      "properties": { "currency": "USD" },
      "occurred_at": "2026-05-20T08:30:00Z"
    }
  ]
}
```

- `value` — typed metric value; optional when the event definition's value type
  is `Unit`. Discriminator `kind` is one of `unit`, `bool`, `int`, `float`, `string`.
- `properties` — flat `string -> string` map used for downstream metric filters.
- `occurred_at` — RFC 3339 client wall-clock; defaults to ingestion time.

**Response (`202 Accepted`):**

```json
{
  "accepted_count": 1,
  "rejected": []
}
```

Each `rejected[]` entry carries `event_key` and a `reason` discriminator:
`unknown_event_key`, `archived_event_key`, `value_type_mismatch`, `missing_value`,
or `invalid_occurred_at`. Per-second batch quota is governed by
`STITCHD_EVENT_QUOTA_PER_SEC` (default `1000`).

## Error Envelope

Errors follow the standard gateway envelope:

```json
{ "error": "sdk key not found", "code": "UNAUTHENTICATED" }
```

## Rate Limits

SDK routes are designed for high-throughput SDK usage. No explicit rate limits are enforced by the gateway itself; operators should place a reverse proxy (e.g., nginx, envoy) in front for production rate limiting.
