# ScyllaDB Setup

ScyllaDB stores all list-segment memberships — the include/exclude key lists
that can grow to **millions of rows per segment** without touching PostgreSQL.
Only `stitchd-segmentation-service` talks to Scylla; every other service
goes through gRPC if it needs membership data.

This page covers the topology landed in
`segment_scylla_20260516`: a dedicated `stitchd_segments` keyspace, generation
pointer + LWT swap pattern, and the background generation sweeper.

## Why ScyllaDB

List-based segments are unbounded. A single segment can hold millions of
user IDs, account IDs, or any context-key value type. PostgreSQL partitioned
tables work for moderate sizes, but at scale the row count and the bulk
UPDATE volume (replacing entire lists on every upload) cause write
amplification and VACUUM pressure that compete with the rest of the OLTP
workload.

ScyllaDB's wide-row model maps cleanly onto the access pattern:

| Access Pattern                | CQL behaviour                                                                |
|-------------------------------|------------------------------------------------------------------------------|
| Bulk write (replace list)     | `INSERT` rows for a new generation, atomic pointer flip via LWT.             |
| Point membership check        | Partition-key lookup — O(1) regardless of list size.                         |
| Bulk delete (after pointer flip) | Partition delete by `(segment_id, context_type, generation)`.             |

## System Requirements

- ScyllaDB 6.2 (matches the `scylladb/scylla:6.2` image in
  [`docker-compose.yml`](https://github.com/stitchd-dev/feature-flag/blob/main/docker-compose.yml)).
- The driver in use is
  [`scylla`](https://docs.rs/scylla/) (the official Rust driver). The
  CQL/native protocol port is `9042`.

## Schema Overview

Five tables managed by versioned CQL migrations in
[`crates/stitchd-db/scylla-migrations/`](https://github.com/stitchd-dev/feature-flag/tree/main/crates/stitchd-db/scylla-migrations):

| Migration                                | Object                                  | Purpose                                                                                              |
|------------------------------------------|------------------------------------------|------------------------------------------------------------------------------------------------------|
| `0001_keyspace.cql`                      | `stitchd_segments` keyspace              | Audit-trail marker only — actual keyspace creation happens in the migration runner's `bootstrap()`. |
| `0002_segment_list_entries.cql`          | `segment_list_entries`                   | Entry rows; partition key `(segment_id, context_type, generation)`.                                  |
| `0003_segment_list_generations.cql`      | `segment_list_generations`               | Active-generation pointer per `(segment_id, context_type)`. LWT-updated on bulk replace.             |
| `0004_segment_list_summary.cql`          | `segment_list_summary`                   | Counter table: include/exclude entry counts per `(segment_id, context_type, generation)`.            |
| `0005_segment_list_orphaned_gens.cql`    | `segment_list_orphaned_gens`             | Tracks superseded generations pending cleanup by the background sweeper.                             |

The migration applier — [`stitchd_db::scylla::migrate::run`](https://github.com/stitchd-dev/feature-flag/blob/main/crates/stitchd-db/src/scylla/migrate.rs) —
keeps an audit row per applied version in `<keyspace>.scylla_migrations` so
re-runs are idempotent. Files are applied in lexicographic order; use
zero-padded numeric prefixes for new migrations.

## 1. Provision the Cluster

For local dev, compose brings up a single-node Scylla container:

```bash
docker compose up -d --wait scylladb
```

For production, the keyspace should use `NetworkTopologyStrategy` with
`RF ≥ 3`:

```cql
CREATE KEYSPACE IF NOT EXISTS stitchd_segments
WITH REPLICATION = {
    'class': 'NetworkTopologyStrategy',
    'datacenter1': 3
};
```

The migration applier's `bootstrap()` creates the keyspace with
`SimpleStrategy, RF=1` on first run, which is fine for dev but **must be
ALTERed** before going to production.

## 2. Run Migrations

Use the `xtask` task:

```bash
# Defaults: STITCHD_SCYLLA_URI=127.0.0.1:9042, STITCHD_SCYLLA_KEYSPACE=stitchd_segments
cargo xtask scylla-migrate
```

What this does (see
[`crates/xtask/src/main.rs`](https://github.com/stitchd-dev/feature-flag/blob/main/crates/xtask/src/main.rs)):

1. Reads `STITCHD_SCYLLA_URI` / `STITCHD_SCYLLA_KEYSPACE` (defaults
   `127.0.0.1:9042` and `stitchd_segments`).
2. Connects via the driver, bootstraps the keyspace + migration-tracking
   table if absent.
3. Iterates `crates/stitchd-db/scylla-migrations/*.cql` in lexicographic
   order, applying each `.cql` whose version is not already in
   `stitchd_segments.scylla_migrations`.

To target a remote cluster:

```bash
STITCHD_SCYLLA_URI=scylla-0.prod.internal:9042 \
STITCHD_SCYLLA_KEYSPACE=stitchd_segments \
cargo xtask scylla-migrate
```

## 3. Connection Configuration

Only `stitchd-segmentation-service` reads these:

| Variable                                          | Default                | Description                                                                                |
|---------------------------------------------------|------------------------|--------------------------------------------------------------------------------------------|
| `STITCHD_SCYLLA_URI`                              | `127.0.0.1:9042`       | CQL contact point (`host:port`). Pass the first seed; the driver discovers the rest.       |
| `STITCHD_SCYLLA_KEYSPACE`                         | `stitchd_segments`     | Keyspace for all list-segment tables. Renamed from `stitchd` in `boundaries_20260518`.     |
| `STITCHD_SEGMENTATION_SWEEPER_RETENTION_SECS`     | `86400` (24 h)         | Minimum age before a superseded generation is hard-deleted.                                |
| `STITCHD_SEGMENTATION_SWEEPER_INTERVAL_SECS`      | `3600` (1 h)           | How often the generation sweeper task runs.                                                |

See [`./env-vars.md`](./env-vars.md) for the full env reference.

## Consistency Levels

Driver defaults map to the per-operation expectation:

| Operation                          | Consistency        | Rationale                                                          |
|------------------------------------|--------------------|--------------------------------------------------------------------|
| Entry write (`INSERT`)             | `QUORUM`           | Durable across a majority of replicas before ack.                  |
| Generation pointer swap (LWT)      | `SERIAL` (Paxos)   | Atomic compare-and-set — must be linearisable.                     |
| Membership lookup (`SELECT`)       | `ONE`              | Low-latency reads on the hot path; eventual consistency is fine.   |

## Docker Compose (Development)

The compose file already includes a developer-tuned Scylla service:

```yaml
scylladb:
  image: scylladb/scylla:6.2
  command: --smp 1 --memory 512M --overprovisioned 1 --developer-mode 1
  ports:
    - "${STITCHD_SCYLLA_CQL_PORT:-9042}:9042"
  healthcheck:
    test: ["CMD-SHELL", "cqlsh -e 'describe cluster' || exit 1"]
    interval: 10s
    retries: 20
    start_period: 60s
```

`segmentation-service` waits on the Scylla healthcheck before starting. The
60-second `start_period` is intentional — Scylla cold-starts take noticeably
longer than ClickHouse or Postgres on a laptop.

To open a `cqlsh` shell against the running container:

```bash
docker exec -it stitchd-scylladb cqlsh -k stitchd_segments
```

## Observability

The driver emits Prometheus gauges and counters from
`stitchd_db::scylla::metrics`. Scraped every 15 s on the segmentation
service's Prometheus port:

| Metric                              | Description                                       |
|-------------------------------------|---------------------------------------------------|
| `scylla_queries_total`              | Cumulative non-paged queries.                     |
| `scylla_query_errors_total`         | Cumulative non-paged errors.                      |
| `scylla_paged_queries_total`        | Cumulative paged queries.                         |
| `scylla_paged_query_errors_total`   | Cumulative paged errors.                          |
| `scylla_retries_total`              | Retry-policy activations.                         |
| `scylla_connections_active`         | Open connections to the cluster.                  |
| `scylla_connection_timeouts_total`  | Connection-establishment timeouts.                |
| `scylla_request_timeouts_total`     | Client-side request timeouts.                     |
| `scylla_prepared_cache_size`        | Statements in the prepared-statement cache.       |
| `scylla_query_latency_p50_ms`       | p50 latency (ms).                                 |
| `scylla_query_latency_p95_ms`       | p95 latency (ms).                                 |
| `scylla_query_latency_p99_ms`       | p99 latency (ms).                                 |

Every `prepare()` call emits an OpenTelemetry-compatible `tracing` span
(`db.system = "scylladb"`, `db.operation = "prepare"`) — propagated to any
OTel exporter wired in via
[`tracing-opentelemetry`](https://docs.rs/tracing-opentelemetry/).

## Generation Sweeper

The background sweeper task lives in
`stitchd-segmentation-service` and periodically purges Scylla partitions
left behind after bulk-replace operations:

1. A `set_list_entries` call writes a new generation, then atomically swaps
   the active-generation pointer via an LWT compare-and-set.
2. The superseded generation ID is recorded in
   `segment_list_orphaned_gens` along with an `orphaned_at` timestamp.
3. After `STITCHD_SEGMENTATION_SWEEPER_RETENTION_SECS` have elapsed, the
   sweeper deletes the partition rows from `segment_list_entries` and
   marks the orphan record as swept.

The retention window (24 h by default) gives any in-flight membership
lookups against the old generation time to complete before the rows
disappear.

## Sizing Guidance

| Segment Scale | Estimated Footprint per Segment | Notes                                                                  |
|---------------|--------------------------------|------------------------------------------------------------------------|
| 10 k entries  | ~1 MB                          | Dev / staging; SimpleStrategy RF=1 is fine.                            |
| 1 M entries   | ~100 MB                        | Production baseline; switch to NetworkTopologyStrategy, RF=3, 3+ nodes.|
| 100 M entries | ~10 GB                         | Dedicated keyspace per env; ensure token-aware routing is enabled.     |

Row size estimate: ~50–100 bytes per entry (UUID + `context_type` + `generation`
+ `list_type` + `entry_key` as TEXT).
