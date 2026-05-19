# ClickHouse Setup

Stitchd uses ClickHouse 24+ for high-volume event storage, experiment metric
aggregations, and flag evaluation telemetry. The `stitchd-event-writer` library
crate handles all ClickHouse writes and schema migrations.

## Tables

| Table | Engine | Notes |
|---|---|---|
| `events` | MergeTree, monthly partitions | Primary ingestion table |
| `events_v2` | MergeTree, weekly `toMonday()` partitions | Optimized partition granularity |
| `flag_evaluation_log_v2` | MergeTree, weekly `toMonday()` partitions + TTL | Eval log |
| `events_experiment_daily` | AggregatingMergeTree | Pre-aggregated experiment stats by `(env_id, experiment_id, variant_key, metric_key, day)` |
| `events_experiment_daily_mv` | Materialized View | Auto-populates `events_experiment_daily` on `events` insert using `*State` combiners |
| `experiment_results` | MergeTree | Pre-computed per-experiment results; written by `stitchd-stats-service` every 60 min; replaces the retired PostgreSQL `experiment_results` table |

## Why ClickHouse

PostgreSQL is optimized for transactional config reads/writes. Experiment events are
append-only, high-throughput, and require analytical queries (aggregations, time-series).
ClickHouse handles this workload efficiently without impacting the main config store.

The `experiment_results` table previously lived in PostgreSQL. As of `boundaries_20260518`
it lives exclusively in ClickHouse — `stitchd-analytics-service` owns the schema and
`stitchd-stats-service` writes to it via gRPC only.

## Environment Variables

| Variable | Description |
|---|---|
| `STITCHD_CLICKHOUSE_URL` | HTTP interface, e.g. `http://localhost:8123` |
| `STITCHD_CLICKHOUSE_DB` | Database name (e.g. `stitchd`) |
| `STITCHD_CLICKHOUSE_USER` | Username |
| `STITCHD_CLICKHOUSE_PASSWORD` | Password |

## Docker (Development)

```bash
# Start ClickHouse alongside Postgres and ScyllaDB
docker compose up postgres clickhouse scylladb -d --wait
```

```yaml
services:
  clickhouse:
    image: clickhouse/clickhouse-server:24
    ports:
      - "8123:8123"   # HTTP interface (writes + queries)
      - "9000:9000"   # Native protocol
    volumes:
      - ch_data:/var/lib/clickhouse

volumes:
  ch_data:
```

## AggregatingMergeTree Invariants

- **Insert:** use `*State` combiners (`countState()`, `sumState(Float64)`, `uniqState()`)
- **Read:** use `*Merge` combiners (`countMerge`, `sumMerge`, `uniqMerge`) in GROUP BY
- `sumState(Nullable(Float64))` mismatches `AggregateFunction(sum, Float64)` — wrap with `ifNull(..., 0.0)`
