# ScyllaDB Setup

ScyllaDB stores all list-segment entries — the include/exclude key lists that can grow to **millions of rows per segment** without impacting PostgreSQL.

## Why ScyllaDB?

List-based segments are unbounded: a single segment can hold millions of user IDs or organisation keys. PostgreSQL partitioned tables are adequate for moderate sizes, but at scale the row count and UPDATE volume (replacing entire lists on each bulk upload) create unacceptable write amplification and VACUUM pressure.

ScyllaDB's wide-row model maps cleanly onto the access pattern:

| Access Pattern | CQL |
|---|---|
| Bulk write (replace list) | `INSERT` rows for new generation, atomic generation pointer flip via LWT |
| Point membership check | Partition key lookup — O(1) regardless of list size |
| Bulk delete (after swap) | Partition delete by `(segment_id, context_type, generation)` |

## Schema Overview

Five tables managed by versioned CQL migrations in `crates/stitchd-db/scylla-migrations/`:

| Migration | Table | Purpose |
|---|---|---|
| `0001_keyspace.cql` | `stitchd` keyspace | Replication strategy (SimpleStrategy, RF=1 dev default) |
| `0002_segment_list_entries.cql` | `segment_list_entries` | Entry rows per `(segment_id, context_type, generation, list_type, entry_key)` |
| `0003_segment_list_generations.cql` | `segment_list_generations` | Current active generation pointer per `(segment_id, context_type)` |
| `0004_segment_list_summary.cql` | `segment_list_summary` | Counter table: include/exclude entry counts per `(segment_id, context_type, generation)` |
| `0005_segment_list_orphaned_gens.cql` | `segment_list_orphaned_gens` | Tracks superseded generations pending cleanup by the background sweeper |

## Replication Factor & Consistency Level

For **production** deployments the keyspace should be created with `NetworkTopologyStrategy` and `RF ≥ 3`:

```cql
CREATE KEYSPACE IF NOT EXISTS stitchd
WITH REPLICATION = {
    'class': 'NetworkTopologyStrategy',
    'datacenter1': 3
};
```

Default consistency levels used by the driver:

| Operation | Consistency | Rationale |
|---|---|---|
| Entry write (INSERT) | `QUORUM` | Durable across majority of replicas |
| Generation pointer swap (LWT) | `SERIAL` (Paxos) | Atomic CAS — must be linearisable |
| Membership lookup (SELECT) | `ONE` | Low-latency reads; eventual consistency acceptable |

## Environment Variables

| Variable | Default | Description |
|---|---|---|
| `STITCHD_SCYLLA_URI` | `127.0.0.1:9042` | Contact point (host:port) |
| `STITCHD_SCYLLA_KEYSPACE` | `stitchd` | Keyspace used for all tables |
| `SWEEPER_RETENTION_SECS` | `86400` (24 h) | How old an orphaned generation must be before it's deleted |
| `SWEEPER_INTERVAL_SECS` | `3600` (1 h) | How often the generation sweeper runs |

## Docker Compose (Development)

The development `docker-compose.yml` includes a `scylladb` service:

```yaml
scylladb:
  image: scylladb/scylla:6.2
  ports:
    - "9042:9042"
  healthcheck:
    test: ["CMD", "cqlsh", "-e", "describe keyspaces"]
    interval: 10s
    timeout: 5s
    retries: 12
```

Start with:

```bash
docker compose up postgres clickhouse scylladb -d --wait
```

The segmentation service waits for the ScyllaDB healthcheck to pass before it starts.

## Observability

The Scylla driver emits the following Prometheus gauges (scraped every 15 s):

| Metric | Description |
|---|---|
| `scylla_queries_total` | Cumulative non-paged queries |
| `scylla_query_errors_total` | Cumulative non-paged errors |
| `scylla_paged_queries_total` | Cumulative paged queries |
| `scylla_paged_query_errors_total` | Cumulative paged errors |
| `scylla_retries_total` | Retry-policy activations |
| `scylla_connections_active` | Open connections to the cluster |
| `scylla_connection_timeouts_total` | Connection-establishment timeouts |
| `scylla_request_timeouts_total` | Client-side request timeouts |
| `scylla_prepared_cache_size` | Statements in the prepared-statement cache |
| `scylla_query_latency_p50_ms` | p50 latency (ms) |
| `scylla_query_latency_p95_ms` | p95 latency (ms) |
| `scylla_query_latency_p99_ms` | p99 latency (ms) |

All Scylla `prepare()` calls are instrumented with an OpenTelemetry-compatible
`tracing` span (`db.system = "scylladb"`, `db.operation = "prepare"`) that flows to
any OTel exporter connected via `tracing-opentelemetry`.

## Generation Sweeper

The generation sweeper is a background task that periodically removes orphaned
Scylla partitions left over from list bulk-replace operations:

1. A `set_list_entries` call atomically swaps the active generation pointer via LWT CAS.
2. The superseded generation is recorded in `segment_list_orphaned_gens` with an `orphaned_at` timestamp.
3. After `SWEEPER_RETENTION_SECS` have elapsed, the sweeper deletes the partition rows and marks the orphan record as swept.

The retention window (default 24 h) ensures that any in-flight membership lookups against the old generation complete safely before data is removed.

## Sizing Guidance

| Segment Scale | Estimated Scylla Footprint | Notes |
|---|---|---|
| 10 k entries | ~1 MB per segment | Dev/staging; RF=1 acceptable |
| 1 M entries | ~100 MB per segment | Production baseline; RF=3, 3+ nodes |
| 100 M entries | ~10 GB per segment | Dedicated keyspace; token-aware routing critical |

Row size estimate: ~50–100 bytes per entry (UUID + context_type + generation + list_type + entry_key as text).
