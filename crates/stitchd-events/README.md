# stitchd-events

ClickHouse event ingestion library. Provides the `EventWriter` and schema migration helpers used by `stitchd-event-service`.

## Modules

| Module | Purpose |
|--------|---------|
| `writer` | `EventWriter` — batched inserts into ClickHouse |
| `migrations` | ClickHouse DDL migration runner |
| `clickhouse` | ClickHouse client configuration and connection helpers |

## Usage

This crate is an internal library — `stitchd-event-service` is the only binary that depends on it. It is not intended to be used directly by application code.

## ClickHouse Schema

Migration files live in `migrations/`. Run them via the migration helper before starting `stitchd-event-service`:

```bash
# Migrations are applied automatically at service startup
cargo run -p stitchd-event-service
```

## Dependencies

- `stitchd-core` — `MetricEvent`, `EvaluationEvent` domain types
- `clickhouse` — HTTP client for ClickHouse inserts
