# stitchd-event-service

gRPC microservice that ingests experiment and evaluation events.

Listens on `:50054` and exposes **`EventIngestionService`** — accepts batches of metric events from the gateway and writes them to ClickHouse.

## Responsibilities

- Receiving batched event payloads from SDK clients (via gateway)
- Writing events to ClickHouse for downstream experimentation analysis
- Reporting accepted/rejected counts per batch

## Dependencies

- `stitchd-core` — domain types
- `stitchd-db` — PostgreSQL access (for SDK key validation and metadata)
- `stitchd-events` — ClickHouse writer and migration helpers
- `stitchd-proto` — `events.v1` tonic stubs

## Environment Variables

| Variable | Default | Description |
|---|---|---|
| `EVENT_SERVICE_PORT` | `50054` | gRPC listen port |
| `METRICS_PORT` | `9104` | Prometheus metrics port |
| `DATABASE_URL` | — | PostgreSQL connection string (required) |
| `CLICKHOUSE_URL` | — | ClickHouse HTTP URL (required, e.g. `http://user:pass@host:8123/db`) |
| `RUST_LOG` | `info` | Log filter |

## Running

```bash
DATABASE_URL=postgres://stitchd:stitchd@localhost/stitchd \
CLICKHOUSE_URL=http://stitchd:stitchd@localhost:8123/stitchd \
cargo run -p stitchd-event-service
```
