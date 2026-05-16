# Spec: Database & Query Optimizations

## Overview

A comprehensive overhaul of all data-access bottlenecks across PostgreSQL and
ClickHouse. Covers index gaps, N+1 query patterns, missing partial indexes, an
in-process SDK key cache, ClickHouse injection hardening, experiment materialized
views, partition tuning, and admin list pagination. No items deferred.

## Functional Requirements

### FR-1: SDK Key Hash Index (Critical)
Add a composite index `(key_hash, is_active)` on `sdk_keys`. The authentication
hot path queries `WHERE key_hash = $1 AND is_active = TRUE`; the current index
`(environment_id, is_active)` forces a full table scan on every SDK evaluation.

### FR-2: Batch Segment Load — eliminate N+1 (Critical)
`fetch_segment_definitions` in `stitchd-flag-service` issues 3N sequential
queries when a flag references N segments. Replace with three bulk queries:
- `find_batch_by_ids(ids)` → all segment records in one query
- `find_rules_batch(ids)` → all rule-based segment rules in one query
- `find_lists_batch(ids)` → all list-based segment configs in one query

### FR-3: Partial Indexes on Soft-Delete Columns (High)
Add `WHERE deleted_at IS NULL` partial indexes for every table that carries a
`deleted_at` column and is regularly queried with that filter:
`feature_flags`, `segments`, `projects`, `environments`, `variants`,
`feature_flag_rules`, `event_definitions`, `experiments`.

### FR-4: Covering Index for Segment List Entry Lookups (High)
Extend the segment list entry lookup index from
`(segment_id, context_type, list_type)` to
`(segment_id, context_type, list_type, entry_key)` so the EXISTS subqueries
used in membership checks are index-only scans.

### FR-5: In-Process SDK Key Cache in Auth Service (High)
Add a moka cache inside `stitchd-auth-service` keyed on `key_hash`.
- TTL: 60 seconds (acceptable staleness window for revocation propagation)
- On cache hit: skip Postgres lookup entirely
- On cache miss: query DB, populate cache
- On revocation: proactively invalidate that entry

### FR-6: Context Registry Purge Indexes (Medium)
Add single-column indexes on `last_seen_at` for both `context_type_registry`
and `context_param_registry` to accelerate the 90-day purge DELETE.

### FR-7: ClickHouse eval_stats Injection Fix (Medium)
`GET /v1/projects/{project_id}/flags/{flag_id}/eval-stats` builds its SQL via
`format!()` with `flag_id_str` interpolated directly. Replace with clickhouse-rs
bind parameters. `flag_id` must be validated as a UUID at handler entry.

### FR-8: ClickHouse Experiment Materialized Views (Low → now required)
Build experiment-scoped materialized views to replace raw-events aggregation:
- `events_experiment_daily_mv` (AggregatingMergeTree): keyed on
  `(env_id, experiment_id, variant_key, metric_key, day)` — stores
  count_state, sum_state, uniq_state aggregate functions
- Backfill migration: populate MVs from existing raw events data (idempotent)
- Rewrite `experiment_queries.rs` to read from MVs via `finalizeAggregation()`

### FR-9: ClickHouse Partition Tuning (Low → now required)
Change partition granularity from monthly (`toYYYYMM`) to weekly (`toMonday`)
for `flag_evaluation_log` and `events` tables. Weekly partitions reduce data
scanned on typical 1–30 day query windows. Migration creates new tables, copies
data, then renames.

### FR-10: Offset Pagination on Admin List Endpoints (Medium)
All admin list endpoints gain `?page=N&per_page=100` (default per_page=50,
max=200). Applies to: flags, segments, experiments, event definitions, SDK keys,
org users, audit log. Response wraps items in `{ items, total, page, per_page }`.

## Non-Functional Requirements

- All PostgreSQL index additions must use `CREATE INDEX CONCURRENTLY` to avoid
  table locks in production migrations.
- SDK key cache must never serve a revoked key beyond its 60-second TTL window.
- ClickHouse MV backfill must be idempotent (re-runnable without duplicating data).
- All new query paths require unit tests; hot-path changes (FR-1, FR-2, FR-5)
  require integration tests.
- Pagination changes are additive — existing callers with no query params receive
  the first page (backwards-compatible, no breaking API change).

## Acceptance Criteria

- [ ] `EXPLAIN` on SDK key lookup shows index scan on `key_hash` (not seq scan)
- [ ] Segment fetch for a flag with 10 segments issues exactly 3 DB queries
      (verified by mock repository counting call invocations)
- [ ] Soft-delete list queries use partial index (confirmed via EXPLAIN)
- [ ] Auth service cache hit skips DB loader (unit test with call counter mock)
- [ ] `eval_stats` query uses bind parameters — no `format!()` SQL construction
- [ ] Experiment result queries read from MVs (confirmed by test assertions on MV data)
- [ ] `flag_evaluation_log` and `events` use weekly partitions post-migration
- [ ] `GET /v1/projects/.../flags?page=2&per_page=10` returns correct slice with `total` field

## Out of Scope

- Client-side SDK caching (separate SDK track)
- ClickHouse Kafka ingestion path
- PostgreSQL connection pooling tuning (PgBouncer)
- Cursor-based pagination
- ClickHouse query profiling / EXPLAIN tooling
