# SDK APIs

REST endpoints consumed by the stitchd SDK. All SDK routes authenticate via the `x-sdk-key` header — no JWT required.

## Auth Model

Include the environment's SDK key in every request:

```
x-sdk-key: sdk_live_abc123...
```

SDK keys are scoped to a single environment. A request with an invalid or missing key returns `401 Unauthorized`.

## Endpoints

### Evaluate Flag

```
POST /v1/environments/{env_id}/evaluate
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
POST /v1/environments/{env_id}/events
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
POST /v1/environments/{env_id}/events/batch
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

### List-Check Segment Membership

```
POST /v1/environments/{env_id}/segments/list-check
```

Check whether a context is a member of a list segment.

**Request body:**

```json
{
  "segment_key": "beta-users",
  "context_type": "user",
  "context_key": "user-123"
}
```

**Response:**

```json
{
  "is_member": true
}
```

---

### Batch List-Check Segment Membership

```
POST /v1/environments/{env_id}/segments/batch-list-check
```

Check membership for multiple (segment, context) pairs in one call.

## Error Envelope

Errors follow the standard gateway envelope:

```json
{ "error": "sdk key not found", "code": "UNAUTHENTICATED" }
```

## Rate Limits

SDK routes are designed for high-throughput SDK usage. No explicit rate limits are enforced by the gateway itself; operators should place a reverse proxy (e.g., nginx, envoy) in front for production rate limiting.
