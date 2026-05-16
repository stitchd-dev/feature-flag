# Spec: List-Based Segment Storage on ScyllaDB

## Overview

List-based segments currently store entries in PostgreSQL's `segment_list_entries`
table (monthly range-partitioned via `pg_partman`). At our target scale, individual
list segments can grow to millions of keys per `(segment_id, context_type)` — a
workload PostgreSQL is not well-suited for. We will introduce ScyllaDB as the
storage backend for list-segment entries and retain PostgreSQL for segment
metadata only (the `segments` table, rules, condition expressions, audit log).

Because the system is greenfield (no production data yet), we hard-cut over: drop
the `segment_list_entries` table and its `pg_partman` configuration entirely, and
remove the "fetch all keys" read path (which can return millions of rows) from
both backend and admin UI.

## Functional Requirements

### FR-1: ScyllaDB Schema
A new `segment_list_entries` table in ScyllaDB optimised for point membership
reads, plus a `segment_list_generations` pointer table for atomic swaps:
- **Entries PK:** `(segment_id, context_type, generation)`
- **Entries CK:** `(list_type, entry_key)`
- **Pointer table:** `(segment_id, context_type) → active_generation`
- Schema bootstrapped via versioned `.cql` files under
  `crates/stitchd-db/scylla-migrations/`.

### FR-2: Generation-Swap Full Replace
`set_list_entries(segment_id, context_type, include, exclude)` works via:
1. Read current generation from the pointer table (defaults to 0).
2. Stream all include+exclude rows under `new_gen = current + 1`, chunked into
   prepared-statement batches well under Scylla's `batch_size_fail_threshold`.
3. CAS-update the pointer (LWT `IF active_generation = current`) to flip to
   `new_gen`; retry on contention.
4. Background sweeper later drops the orphaned generation.
- Atomic from the reader's perspective: a reader sees exactly the old or new
  generation, never a mix.

### FR-3: Diff-Based API
First-class operations for incremental edits, scoped to the current generation:
- `add_entries(segment_id, context_type, list_type, keys[])` — INSERTs.
- `remove_entries(segment_id, context_type, list_type, keys[])` — DELETEs.
Used by the admin UI when adding/removing individual keys.

### FR-4: Membership Read Paths
Re-implement these `SegmentRepository` methods against ScyllaDB:
- `check_list_membership(env_id, ctx_type, ctx_key, segment_keys[])` — single
  context, many segments. Resolved via parallel `(segment_id, context_type,
  current_gen)` partition + CK point reads for `include` and `exclude` slices.
- `batch_check_list_membership(env_id, contexts[], segment_keys[])`.
- `find_memberships_batch(env_id, contexts[], segment_ids[])` — SDK hot path.
- Token-aware routing + prepared statements throughout.

### FR-5: Remove "Fetch All Keys" Path
- Remove `SegmentRepository::find_with_list` and consumers that dump entries.
- Add `get_list_segment_summary(segment_id) → { context_type → { include_count,
  exclude_count } }` (counts only, maintained via small per-`(segment_id,
  context_type)` counter rows updated atomically with entry mutations).
- Update the admin UI list-segment detail page to display counts and a
  search-by-key Add/Remove UI; no full key list is fetched or rendered.
- Update gRPC `GetSegment` to return summary metadata for list-typed segments.

### FR-6: PostgreSQL Cleanup
- New forward-only migration drops `segment_list_entries` and its `pg_partman`
  configuration.
- Remove all `sqlx::query!` macros that hit `segment_list_entries` from
  `crates/stitchd-db/src/repository/pg/segment.rs`.
- Update `.sqlx/` offline cache. Update / remove
  `crates/stitchd-db/tests/segment_repository.rs`,
  `crates/stitchd-db/tests/segment_extended.rs`, and the segment-list portions of
  `crates/stitchd-db/tests/indexes.rs`.

### FR-7: Docker Compose Integration
- Add a `scylladb` service to `docker-compose.yml` with appropriate ports,
  volumes, healthcheck, and dependency wiring.
- `stitchd-segmentation-service` waits for ScyllaDB to be ready before serving.

### FR-8: ScyllaDB Rust Driver Integration
- Adopt the official `scylla` crate (`scylla-rust-driver`) with Tokio.
- A `ScyllaClient` abstraction wraps the session, prepared-statement cache, and
  config, analogous to the existing `sqlx::PgPool` pattern.
- Token-aware load-balancing policy enabled; prepared statements built once at
  startup.
- Contact points, RF, and consistency level configurable via env vars / config.

### FR-9: Generation Sweeper
- Background task in `stitchd-segmentation-service` that periodically scans the
  pointer table and deletes generations older than the active one, beyond a
  configurable retention window (default 24 h).
- Configurable interval (default: every 30 min). Uses the existing
  `tokio::time::interval` scheduler pattern.

### FR-10: Observability
- ScyllaDB driver metrics (request latency, pool, prepared-statement cache,
  error rates) exported via the existing `metrics-exporter-prometheus` pipeline.
- Distributed tracing via OpenTelemetry on Scylla query spans.

### FR-11: Documentation Updates
- Update `conductor/tech-stack.md`: add ScyllaDB to Data Stores, Key Dependencies
  table, and the segmentation-service description.
- Add a "ScyllaDB" page to the mdBook docs covering RF/CL guidance, sizing
  notes, and schema reference.
- Update generated gRPC docs to reflect the new `GetSegment` response and the
  add/remove RPCs.

## Non-Functional Requirements

### NFR-1: Performance Targets
- Single membership check: p99 < 5 ms (warm cache, prepared statement).
- Batch lookup of 20 segments × 100 contexts (2 000 point reads): p99 < 50 ms.
- Bulk replace of 1 M entries: completes within 60 s against a 3-node Scylla
  cluster with `LOCAL_QUORUM` writes.

### NFR-2: Consistency
- Reads and writes default to `LOCAL_QUORUM`.
- Generation pointer flip uses Scylla LWT (`IF`) to prevent concurrent-replace
  races on the same `(segment_id, context_type)`.

### NFR-3: Test Coverage
- ≥ 90 % coverage on new Scylla-related code (per `conductor/workflow.md`).
- Integration tests run against a Dockerised ScyllaDB in CI.

### NFR-4: Local Dev Experience
- `docker compose up postgres clickhouse scylladb -d --wait` starts the full
  data tier.
- Scylla migrations applied automatically at service startup or via an `xtask`
  command (consistent with existing patterns).

## Acceptance Criteria

- [ ] PG `segment_list_entries` table and its `pg_partman` configuration are
      dropped via a forward-only migration; `idx_segment_list_entries_covering`
      is gone with the table.
- [ ] `scylladb` is in `docker-compose.yml` with healthcheck; the full
      `cargo test --workspace` suite passes after `docker compose up postgres
      clickhouse scylladb -d --wait`.
- [ ] `crates/stitchd-db/scylla-migrations/` contains versioned CQL migrations
      applied at startup or via `xtask`.
- [ ] `SegmentRepository` exposes `set_list_entries`, `add_entries`,
      `remove_entries`, `check_list_membership`, `batch_check_list_membership`,
      `find_memberships_batch`, and `get_list_segment_summary` against Scylla.
      `find_with_list` is removed.
- [ ] Generation-swap full-replace passes a property test: concurrent reads
      during a 100 k-key swap never observe a mixed (old + new) state.
- [ ] LWT-protected pointer flip prevents two concurrent `set_list_entries`
      calls from corrupting the active generation.
- [ ] `add_entries` / `remove_entries` mutate the current generation and are
      reflected in subsequent membership checks.
- [ ] Generation sweeper removes orphaned generations after the retention
      window in an integration test.
- [ ] Admin UI list-segment detail page no longer fetches/renders full key
      lists; it shows counts and an Add/Remove search-by-key UI.
- [ ] gRPC `GetSegment` for list-typed segments returns counts + metadata, not
      full entries.
- [ ] Prometheus exposes Scylla driver metrics; OTel traces include Scylla
      spans.
- [ ] `conductor/tech-stack.md` and the mdBook docs are updated.
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` passes.
- [ ] Per-crate `cargo tarpaulin` coverage ≥ 90 % for `stitchd-db` and
      `stitchd-segmentation-service`.

## Out of Scope

- **Backfill of existing PG `segment_list_entries` data.** System is greenfield;
  no production rows to migrate.
- **Multi-region replication.** Single-DC `LOCAL_QUORUM` only; multi-DC
  topology deferred.
- **Rule-based segments.** They remain entirely in PostgreSQL.
- **Events / experiment storage.** Stays on ClickHouse.
- **Other PG tables.** Only `segment_list_entries` is removed.
- **Full paginated list-key browse UI.** Detail page is counts + search-by-key
  only; a future track may add paginated browsing if needed.
- **gRPC streaming export of all keys for a segment.** A separate track if ever
  needed.
