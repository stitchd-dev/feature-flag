# Event Service (`stitchd-event-service`)

## Responsibility

Receives and stores metric events for downstream experimentation analysis:

- Real-time event ingestion from the gateway (SDK key and JWT paths)
- Event definition registry (validates metric keys before accepting events)
- Storing events for query by the experimentation service

## Port

| Transport | Default Port |
|-----------|-------------|
| gRPC | `50054` |

## Service: `EventIngestionService`

**Package:** `stitchd.events.v1`

### `IngestEvent`

```
rpc IngestEvent(IngestRequest) returns (IngestResponse)
```

Ingest a batch of metric events. The SDK key is supplied via gRPC metadata (`x-sdk-key`). Unknown metric keys — those not pre-registered as `EventDefinition` entries — are rejected and returned in `rejected_keys`.

**`IngestRequest` fields:**

| Field | Type | Description |
|-------|------|-------------|
| `events` | repeated `Event` | Events to ingest |

**`Event` fields:**

| Field | Type | Description |
|-------|------|-------------|
| `metric_key` | string | Registered metric key |
| `context_type` | string | Context type (e.g., `user`) |
| `context_key` | string | Context identifier |
| `value` | `MetricValue` (optional) | Measured value — bool, int64, or double |
| `timestamp_ms` | int64 | Unix epoch milliseconds; server time used if 0 |

**`MetricValue` oneof:**

| Variant | Type |
|---------|------|
| `bool_value` | bool |
| `int_value` | int64 |
| `double_value` | double |

**`IngestResponse` fields:**

| Field | Type | Description |
|-------|------|-------------|
| `accepted_count` | uint32 | Number of events successfully stored |
| `rejected_keys` | repeated string | Metric keys that were unknown or invalid |

## Event Definition Registry

Before events can be ingested, their metric keys must be registered as `EventDefinition` records. The gateway exposes CRUD endpoints for event definitions under `/v1/environments/{env}/event-definitions/`.

**`EventDefinition` fields:**

| Field | Type | Description |
|-------|------|-------------|
| `key` | string | Unique metric key (e.g., `button_click`) |
| `description` | string | Human-readable description |
| `value_type` | `MetricValueType` | `BOOL`, `INT`, or `DOUBLE` |
| `environment_id` | string | Environment scope |

**`MetricValueType` enum values:**

| Value | Description |
|-------|-------------|
| `METRIC_VALUE_TYPE_BOOL` | Boolean metric (e.g., conversion: true/false) |
| `METRIC_VALUE_TYPE_INT` | Integer metric (e.g., page count) |
| `METRIC_VALUE_TYPE_DOUBLE` | Floating-point metric (e.g., revenue) |

## Auth Requirements

SDK ingestion: SDK key supplied as `x-sdk-key` gRPC metadata, validated by the gateway before the gRPC call is made.

Event definition management: Bearer JWT, validated by the gateway; RBAC context injected as metadata.
