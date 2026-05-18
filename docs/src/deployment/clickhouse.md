# ClickHouse Setup

> **Status: Upcoming** — ClickHouse integration is planned but not yet implemented.
> The `stitchd-events` crate scaffolding exists; the client and schema are in progress.

Stitchd will use ClickHouse 24+ for high-volume event storage:

- Experiment evaluation events (per-context metric values)
- Experiment results and metric aggregations
- Flag evaluation telemetry (optional)

## Why ClickHouse

PostgreSQL is optimized for transactional config reads/writes. Experiment events are
append-only, high-throughput, and require analytical queries (aggregations, time-series).
ClickHouse handles this workload efficiently without impacting the main config store.

## Planned Setup

```bash
# Create the events database
clickhouse-client --query "CREATE DATABASE stitchd_events"
```

A future `STITCHD_CLICKHOUSE_URL` environment variable will point the server at the HTTP
interface (default port 8123):

```
http://clickhouse.internal:8123
```

## Docker Example (for future use)

```yaml
services:
  clickhouse:
    image: clickhouse/clickhouse-server:24
    ports:
      - "8123:8123"   # HTTP interface
      - "9000:9000"   # Native protocol
    volumes:
      - ch_data:/var/lib/clickhouse

volumes:
  ch_data:
```

Check back when the `stitchd-events` crate reaches its first release milestone.
