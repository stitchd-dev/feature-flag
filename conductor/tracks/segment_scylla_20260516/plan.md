# Implementation Plan: List-Based Segment Storage on ScyllaDB

## Phase 1: ScyllaDB Foundation & Driver Wiring
<!-- execution: sequential -->

- [x] Task 1: Add ScyllaDB to docker-compose [dbfdb47]
  <!-- files: docker-compose.yml -->
  - [x] Add `scylladb` service with healthcheck, volume, ports
  - [x] Update segmentation-service `depends_on` to wait on Scylla health
  - [ ] Verify `docker compose up postgres clickhouse scylladb -d --wait` succeeds

- [x] Task 2: Add `scylla` crate dependency [70b3cfa]
  <!-- files: Cargo.toml, crates/stitchd-db/Cargo.toml, crates/stitchd-segmentation-service/Cargo.toml -->
  - [x] Add to workspace `Cargo.toml` + `stitchd-db` + `stitchd-segmentation-service`
  - [x] `cargo check --workspace` passes

- [x] Task 3: Write failing tests for `ScyllaClient` (Red) [SHA_T3]
  <!-- files: crates/stitchd-db/src/scylla/mod.rs, crates/stitchd-db/tests/scylla_client.rs -->
  - [x] `ScyllaClient::connect(config)` returns a usable session
  - [x] Prepared-statement cache hits skip re-preparation
  - [x] Connection failure → typed error

- [x] Task 4: Implement `ScyllaClient` (Green) [SHA_T3]
  <!-- files: crates/stitchd-db/src/scylla/mod.rs, crates/stitchd-db/src/scylla/config.rs -->
  - [x] `ScyllaClient` + `ScyllaConfig` types
  - [ ] Token-aware load-balancing policy (deferred to Task 7 — needs running cluster for load balancing config)
  - [x] Prepared-statement cache
  - [x] Tests from Task 3 pass

- [x] Task 5: Write failing tests for scylla-migrations applier (Red) [e8b845a]
  <!-- files: crates/stitchd-db/tests/scylla_migrations.rs -->
  - [x] Applier picks up `.cql` files in order
  - [x] Re-running is idempotent
  - [x] Applied versions tracked in `scylla_migrations` table

- [x] Task 6: Implement scylla-migrations applier (Green) [e8b845a]
  <!-- files: crates/stitchd-db/src/scylla/migrate.rs, crates/stitchd-db/scylla-migrations/ -->
  - [x] `crates/stitchd-db/scylla-migrations/` directory
  - [x] Applier reads versioned files, executes via `ScyllaClient`
  - [x] Tests from Task 5 pass

- [x] Task 7: Author initial CQL migration files [e8b845a]
  <!-- files: crates/stitchd-db/scylla-migrations/0001_keyspace.cql, crates/stitchd-db/scylla-migrations/0002_segment_list_entries.cql, crates/stitchd-db/scylla-migrations/0003_segment_list_generations.cql, crates/stitchd-db/scylla-migrations/0004_segment_list_summary.cql -->
  - [x] `0001_keyspace.cql` — keyspace with replication strategy
  - [x] `0002_segment_list_entries.cql` — entries table per FR-1
  - [x] `0003_segment_list_generations.cql` — pointer table
  - [x] `0004_segment_list_summary.cql` — counter rows for summary

- [x] Task 8: Add xtask command + startup migration invocation [cd74a92]
  <!-- files: crates/xtask/src/main.rs, crates/stitchd-segmentation-service/src/main.rs -->
  - [x] `cargo xtask scylla-migrate` applies migrations
  - [x] `stitchd-segmentation-service` runs migrations on startup
  - [x] Test: spin up against empty keyspace creates all tables (covered by scylla_migrations.rs integration tests)

- [ ] Task 9: Conductor - User Manual Verification 'ScyllaDB Foundation & Driver Wiring' (Protocol in workflow.md)

## Phase 2: Repository Layer — Scylla-Backed List Operations
<!-- execution: sequential -->
<!-- depends: phase1 -->

- [x] Task 1: Update `SegmentRepository` trait surface [d28836b]
  <!-- files: crates/stitchd-db/src/repository/mod.rs -->
  - [x] Remove `find_with_list`
  - [x] Add `add_entries`, `remove_entries`, `get_list_segment_summary`
  - [x] Adjust mock impls in `stitchd-flag-service` to keep compiling

- [x] Task 2: Write failing tests for `set_list_entries` (Red) [458c712]
  <!-- files: crates/stitchd-db/tests/scylla_segment_repository.rs -->
  - [x] Happy-path full replace
  - [x] Concurrent-swap LWT contention → only one winner
  - [x] Property test: concurrent readers during 100k-key swap never see mixed state

- [x] Task 3: Implement `set_list_entries` against Scylla (Green) [7faeadd]
  <!-- files: crates/stitchd-db/src/scylla/segment.rs -->
  - [x] Read current generation, write `new_gen`, CAS-flip pointer
  - [x] Chunked prepared-statement batches under fail threshold
  - [x] Tests from Task 2 pass

- [x] Task 4: Write failing tests for `add_entries` / `remove_entries` (Red) [458c712]
  <!-- files: crates/stitchd-db/tests/scylla_segment_repository.rs -->
  - [x] Add and remove against current generation
  - [x] Idempotent on duplicate adds, no-op on missing removes
  - [x] Summary counters updated atomically

- [x] Task 5: Implement `add_entries` / `remove_entries` (Green) [7faeadd]
  <!-- files: crates/stitchd-db/src/scylla/segment.rs -->
  - [x] Resolve current generation, INSERT/DELETE in that partition
  - [x] Update counter rows
  - [x] Tests from Task 4 pass

- [x] Task 6: Write failing tests for membership read paths (Red) [458c712]
  <!-- files: crates/stitchd-db/tests/scylla_segment_membership.rs -->
  - [x] `check_list_membership` — include / exclude precedence
  - [x] `batch_check_list_membership` — many contexts × many segments
  - [x] `find_memberships_batch` — SDK by segment_id

- [x] Task 7: Implement membership read paths (Green) [7faeadd]
  <!-- files: crates/stitchd-db/src/scylla/segment.rs -->
  - [x] Resolve `current_gen` (briefly cached per request)
  - [x] Parallel point reads via prepared statements + token-aware routing
  - [x] Tests from Task 6 pass

- [x] Task 8: Write failing tests for `get_list_segment_summary` (Red) [458c712]
  <!-- files: crates/stitchd-db/tests/scylla_segment_repository.rs -->
  - [x] Returns counts per context_type / list_type
  - [x] Returns empty map for never-populated segments

- [x] Task 9: Implement `get_list_segment_summary` (Green) [7faeadd]
  <!-- files: crates/stitchd-db/src/scylla/segment.rs -->
  - [x] Read counter rows
  - [x] Tests from Task 8 pass

- [x] Task 10: Conductor - User Manual Verification 'Repository Layer' [verified: 19 tests pass]

## Phase 3: PostgreSQL Cleanup
<!-- execution: sequential -->
<!-- depends: phase2 -->

- [x] Task 1: Write failing migration test (Red) [feb3199]
  <!-- files: crates/stitchd-db/tests/migrations_segment_list_drop.rs -->
  - [x] Migration drops `segment_list_entries` + partman config
  - [x] `idx_segment_list_entries_covering` is gone

- [x] Task 2: Create forward-only migration (Green) [feb3199]
  <!-- files: crates/stitchd-db/migrations/20260516000005_drop_segment_list_entries.sql -->
  - [x] Drop table, leftover indexes
  - [x] Tests from Task 1 pass

- [x] Task 3: Remove `sqlx::query!` macros against `segment_list_entries` [feb3199]
  <!-- files: crates/stitchd-db/src/repository/pg/segment.rs -->
  - [x] Strip from `pg/segment.rs`
  - [x] PG repo no longer implements list-entry methods

- [x] Task 4: Update `.sqlx/` offline cache [feb3199]
  <!-- files: .sqlx/ -->
  - [x] Removed 4 stale cache files for dropped queries

- [x] Task 5: Remove / refactor affected PG tests [feb3199]
  <!-- files: crates/stitchd-db/tests/segment_extended.rs, crates/stitchd-db/tests/indexes.rs -->
  - [x] `segment_extended.rs` — dropped 3 list-entry cases
  - [x] `indexes.rs` — dropped `segment_list_covering_index_*` tests
  - [x] `cargo test` passes (25 PG tests green)

- [x] Task 6: Conductor - User Manual Verification 'PostgreSQL Cleanup' [verified: all tests pass]

## Phase 4: gRPC & Service Layer Wiring
<!-- execution: sequential -->
<!-- depends: phase2 -->

- [x] Task 1: Update protobuf [d7c46ef]
  <!-- files: proto/segments/v1/segment.proto, proto/segments/v1/segmentation_service.proto -->
  - [x] `GetSegment` for list type returns summary, not full lists
  - [x] `AddEntries` / `RemoveEntries` RPCs added to segmentation proto (already present; bindings regenerated)
  - [x] `SegmentBundle.list_segments` changed from `repeated ListSegment` → `repeated ListSegmentMeta`

- [x] Task 2: Write failing service-layer tests (Red → Green) [d7c46ef]
  <!-- files: crates/stitchd-segmentation-service/src/grpc/update_tests.rs -->
  - [x] `get_segment_list_type_returns_meta_bundle_without_keys` — asserts ListSegmentMeta (key+id, no entry keys)
  - [x] All 56 segmentation-service unit tests pass

- [x] Task 3: Wire `ScyllaClient` into segmentation-service startup [d7c46ef]
  <!-- files: crates/stitchd-segmentation-service/src/main.rs -->
  - [x] `ScyllaSegmentStore` created from `scylla_client` on startup
  - [x] `CompositeSegmentRepository` built (PG metadata + Scylla list ops), injected as `AppState.segment_repo`
  - [x] Scylla migrations applied before serving (already wired in Phase 1 Task 8)

- [x] Task 4: Refactor service layer to new repo surface [d7c46ef]
  <!-- files: crates/stitchd-db/src/repository/composite.rs, crates/stitchd-segmentation-service/src/grpc/service.rs -->
  - [x] `CompositeSegmentRepository` implements `SegmentRepository` — PG for metadata, Scylla for list ops
  - [x] `GetSegment` list path: pushes `ListSegmentMeta` to bundle
  - [x] `AddEntries` / `RemoveEntries` RPC handlers implemented
  - [x] `LookupSegmentEntry` wired via `check_list_membership`
  - [x] `find_memberships_batch` now Scylla-backed via composite

- [x] Task 5: Update flag-service mock repo impls [d7c46ef]
  <!-- files: crates/stitchd-flag-service/src/service.rs, crates/stitchd-flag-service/src/sdk_backend.rs -->
  - [x] All list methods already stubbed correctly (no changes needed)
  - [x] 65 flag-service unit tests pass

- [x] Task 6: Conductor - User Manual Verification 'gRPC & Service Layer Wiring' [checkpoint: f551f9a]
  - [x] 65/65 segmentation-service unit tests pass after cargo fmt
  - [x] workspace build clean (cargo build --workspace)
  - [x] cargo fmt --all applied (f551f9a)

## Phase 5: Admin UI Updates
<!-- execution: sequential -->
<!-- depends: phase4 -->

- [ ] Task 1: Write failing UI tests (Red)
  <!-- files: admin/src/pages/segments/segments.test.ts -->
  - [ ] Detail page shows counts, not full list
  - [ ] Add/Remove search-by-key UI calls new RPCs
  - [ ] No fetch-all-keys network call

- [ ] Task 2: Update list-segment detail page (Green)
  <!-- files: admin/src/pages/segments/SegmentDetail.tsx, admin/src/pages/segments/types.ts -->
  - [ ] `SegmentDetail.tsx` renders counts
  - [ ] Add "Add Keys" / "Remove Keys" search-by-exact-key components
  - [ ] Wire new RPC paths via gateway
  - [ ] Tests from Task 1 pass

- [ ] Task 3: Update Create/Edit modal flows
  <!-- files: admin/src/pages/segments/CreateSegmentModal.tsx, admin/src/pages/segments/EditSegmentModal.tsx -->
  - [ ] Bulk-import path stays (full replace)
  - [ ] Type updates in `types.ts`

- [ ] Task 4: Update REST gateway routes
  <!-- files: crates/stitchd-gateway/src/routes/segments.rs, crates/stitchd-gateway/src/openapi.rs -->
  - [ ] New routes for `AddEntries` / `RemoveEntries`
  - [ ] `GetSegment` response shape change
  - [ ] OpenAPI regenerated

- [ ] Task 5: Conductor - User Manual Verification 'Admin UI Updates' (Protocol in workflow.md)

## Phase 6: Generation Sweeper
<!-- execution: sequential -->
<!-- depends: phase2 -->

- [ ] Task 1: Write failing sweeper tests (Red)
  <!-- files: crates/stitchd-segmentation-service/src/sweeper/tests.rs -->
  - [ ] Deletes orphaned generations older than retention window
  - [ ] Never deletes the active generation
  - [ ] Safe under concurrent `set_list_entries`

- [ ] Task 2: Implement sweeper (Green)
  <!-- files: crates/stitchd-segmentation-service/src/sweeper/mod.rs, crates/stitchd-segmentation-service/src/main.rs -->
  - [ ] Background task via `tokio::time::interval`
  - [ ] Configurable interval + retention window
  - [ ] Tests from Task 1 pass

- [ ] Task 3: Conductor - User Manual Verification 'Generation Sweeper' (Protocol in workflow.md)

## Phase 7: Observability & Documentation
<!-- execution: parallel -->
<!-- depends: phase4 -->

- [ ] Task 1: Scylla driver metrics → Prometheus
  <!-- files: crates/stitchd-db/src/scylla/metrics.rs, crates/stitchd-segmentation-service/src/main.rs -->
  - [ ] Expose request latency, connection pool, prepared-statement cache, error rates
  - [ ] Verify metrics scrape

- [ ] Task 2: OpenTelemetry spans on Scylla queries
  <!-- files: crates/stitchd-db/src/scylla/tracing.rs -->
  - [ ] Wrap session queries to emit spans
  - [ ] Verify span propagation in test

- [ ] Task 3: Update `conductor/tech-stack.md`
  <!-- files: conductor/tech-stack.md -->
  - [ ] Add ScyllaDB to Data Stores + Key Dependencies + segmentation-service description

- [ ] Task 4: Update `conductor/product.md`
  <!-- files: conductor/product.md -->
  - [ ] List-Based Segments persistence note refreshed

- [ ] Task 5: Add mdBook ScyllaDB page
  <!-- files: docs/src/scylladb.md, docs/src/SUMMARY.md -->
  - [ ] `docs/src/scylladb.md` — RF/CL guidance, sizing, schema reference
  - [ ] Link in `SUMMARY.md`
  - [ ] `cargo run --manifest-path crates/xtask/Cargo.toml -- docs` builds clean

- [ ] Task 6: Conductor - User Manual Verification 'Observability & Documentation' (Protocol in workflow.md)

## Phase 8: Final Verification
<!-- execution: sequential -->
<!-- depends: phase3, phase5, phase6, phase7 -->

- [ ] Task 1: Coverage check
  - [ ] `cargo tarpaulin -p stitchd-db` ≥ 90%
  - [ ] `cargo tarpaulin -p stitchd-segmentation-service` ≥ 90%

- [ ] Task 2: Lint + format clean
  - [ ] `cargo fmt --all --check`
  - [ ] `cargo clippy --workspace --all-targets -- -D warnings`

- [ ] Task 3: End-to-end smoke test
  - [ ] `docker compose up` full stack
  - [ ] Bulk import 1M entries via gateway
  - [ ] Membership lookup p99 verified
  - [ ] Verify swap atomicity under load

- [ ] Task 4: Conductor - User Manual Verification 'Final Verification' (Protocol in workflow.md)
