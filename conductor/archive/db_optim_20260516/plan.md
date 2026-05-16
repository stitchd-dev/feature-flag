# Implementation Plan: Database & Query Optimizations

Track: `db_optim_20260516`

---

## Phase 1: PostgreSQL Index Layer
<!-- execution: sequential -->

- [ ] Task 1: SDK key hash composite index migration
  - New migration: `CREATE INDEX CONCURRENTLY idx_sdk_keys_key_hash_active ON sdk_keys(key_hash, is_active)`
  - Migration must be non-transactional (CONCURRENTLY cannot run inside a transaction)
  - sqlx::test: verify `find_active_by_hash` executes via the new index path (query plan check)

- [ ] Task 2: Soft-delete partial indexes migration
  - New migration: `CREATE INDEX CONCURRENTLY` with `WHERE deleted_at IS NULL` for:
    `feature_flags(project_id)`, `segments(environment_id)`, `projects(organisation_id)`,
    `environments(project_id)`, `variants(flag_id)`,
    `event_definitions(environment_id)`, `experiments(env_id)`
  - sqlx::test: list queries on each table return correct results post-migration

- [ ] Task 3: Segment list entry covering index migration
  - New migration: drop `idx_segment_list_entries_lookup`, create replacement
    `(segment_id, context_type, list_type, entry_key)` — covering, enables index-only scans
  - sqlx::test: EXISTS membership subquery returns correct results with new index

- [ ] Task 4: Context registry purge indexes migration
  - New migration:
    `CREATE INDEX idx_context_type_registry_last_seen ON context_type_registry(last_seen_at)`
    `CREATE INDEX idx_context_param_registry_last_seen ON context_param_registry(last_seen_at)`
  - sqlx::test: DELETE WHERE last_seen_at < $1 deletes correct rows

- [ ] Task: Conductor - User Manual Verification 'PostgreSQL Index Layer' (Protocol in workflow.md)

---

## Phase 2: Segment Batch Load — N+1 Elimination
<!-- depends: phase1 -->
<!-- execution: sequential -->

- [ ] Task 1: Batch repository methods on PgSegmentRepository
  - `find_batch_by_ids(ids: &[SegmentId]) -> Vec<Segment>`
    → `WHERE id = ANY($1) AND deleted_at IS NULL`
  - `find_rules_batch(ids: &[SegmentId]) -> HashMap<SegmentId, Vec<SegmentRule>>`
    → `WHERE segment_id = ANY($1) ORDER BY segment_id, rule_index`
  - `find_lists_batch(ids: &[SegmentId]) -> HashMap<SegmentId, Vec<SegmentListEntry>>`
    → `WHERE segment_id = ANY($1)`
  - Add methods to `SegmentRepository` trait; implement on `PgSegmentRepository`
  - sqlx::test for each: seed N segments, verify result count and ordering

- [ ] Task 2: Refactor fetch_segment_definitions in stitchd-flag-service
  - Replace per-segment loop in `service.rs` with three bulk calls
  - Mock repository test: 10 segments → each batch method called exactly once
  - Integration test: definition fetch issues ≤ 4 total DB queries regardless of segment count

- [ ] Task: Conductor - User Manual Verification 'Segment Batch Load' (Protocol in workflow.md)

---

## Phase 3: SDK Key In-Process Cache
<!-- depends: -->
<!-- execution: sequential -->

- [ ] Task 1: Add moka dependency + update tech-stack.md
  - `moka = { version = "0.12", features = ["future"] }` in stitchd-auth-service Cargo.toml
  - Document cache pattern under "Caching" section in tech-stack.md

- [ ] Task 2: Implement SdkKeyCache
  - `struct SdkKeyCache(Cache<String, SdkKey>)` keyed on `key_hash`, TTL = 60s
  - `async fn get_or_load(&self, hash: &str, loader: F) -> Result<SdkKey, RepositoryError>`
  - Unit tests: hit skips loader, miss invokes loader once, TTL expiry retriggers loader,
    invalidate removes entry, concurrent gets coalesce to single loader call

- [ ] Task 3: Wire cache into validate_sdk_key and revocation path
  - Inject `SdkKeyCache` into `AuthServiceImpl` (new field, constructed in main)
  - `validate_sdk_key` uses `cache.get_or_load(hash, || repo.find_active_by_hash(hash))`
  - SDK key revocation: call `cache.invalidate(key_hash)` after DB revoke
  - Unit test: revoked key not returned after invalidation within TTL window

- [ ] Task: Conductor - User Manual Verification 'SDK Key Cache' (Protocol in workflow.md)

---

## Phase 4: ClickHouse Overhaul
<!-- depends: -->
<!-- execution: sequential -->

- [ ] Task 1: Parameterize eval_stats query — injection fix
  - Validate `flag_id` path param as UUID at handler entry → HTTP 400 on parse failure
  - Replace `format!()` SQL in `routes/eval_stats.rs` with clickhouse-rs bind parameters
  - Unit tests: invalid UUID → 400; valid UUID → query executes; no raw interpolation in SQL

- [ ] Task 2: Experiment materialized views — schema + migration files
  - Create `migrations/clickhouse/` directory with numbered migration scripts
  - `events_experiment_daily_mv` (AggregatingMergeTree):
    keys: `(env_id, experiment_id, variant_key, metric_key, day Date)`
    columns: `count_state AggregateFunction(count)`,
             `sum_state AggregateFunction(sum, Float64)`,
             `uniq_ctx_state AggregateFunction(uniq, String)`
  - MATERIALIZED VIEW trigger on `events` table to auto-populate on insert
  - Unit test: MV schema creation + insert triggers MV population

- [ ] Task 3: Backfill migration — populate MVs from raw events
  - Script: truncate MV, then `INSERT INTO events_experiment_daily_mv SELECT ...`
    with `initializeAggregation` for state columns
  - Idempotent: safe to re-run
  - Integration test: seed raw events, run backfill, assert MV row counts match

- [ ] Task 4: Rewrite experiment_queries.rs to read from MVs
  - Replace `arrayFirst`/`arrayExists` raw events queries with
    `finalizeAggregation()` reads against `events_experiment_daily_mv`
  - Preserve all existing function signatures (no proto/API changes)
  - Unit tests with seeded MV data; results must match previous test expectations

- [ ] Task 5: Partition tuning — weekly partitions for flag_evaluation_log and events
  - Create `flag_evaluation_log_v2` with `PARTITION BY toMonday(evaluated_at)`
  - Create `events_v2` with `PARTITION BY toMonday(occurred_at)`
  - Copy: `INSERT INTO ..._v2 SELECT * FROM ...`
  - Assert row counts match, then drop originals and rename v2
  - Integration test: post-migration row counts preserved; time-range query hits
    correct weekly partition

- [ ] Task: Conductor - User Manual Verification 'ClickHouse Overhaul' (Protocol in workflow.md)

---

## Phase 5: Offset Pagination
<!-- depends: -->
<!-- execution: sequential -->

- [ ] Task 1: Shared pagination types in stitchd-gateway
  - `struct PaginationParams { page: u32, per_page: u32 }` — default page=1,
    per_page=50, max cap=200 (enforced in extractor)
  - `struct PaginatedResponse<T> { items: Vec<T>, total: u64, page: u32, per_page: u32 }`
  - Unit tests: defaults applied, per_page capped at 200, page=0 normalised to 1

- [ ] Task 2: Paginate flag list endpoint
  - Repository: `list_by_project_paginated(project_id, offset, limit) -> (Vec<Flag>, u64)`
    using `COUNT(*) OVER()` window function
  - Route: `GET /v1/projects/:pid/flags?page=N&per_page=N` → `PaginatedResponse<FlagJson>`
  - Backwards-compatible: no params → page 1, per_page 50
  - sqlx::test: page 1 of 2, page 2 of 2, total count accurate

- [ ] Task 3: Paginate segment list endpoint
  - Repository: `list_by_environment_paginated` + `list_entries_paginated`
  - Routes: `GET /v1/segments?env_id=...&page=N` and
    `GET /v1/segments/:id/entries?page=N`
  - sqlx::test: correct slicing and total count

- [ ] Task 4: Paginate remaining admin endpoints
  - Experiments: `GET /v1/environments/:eid/experiments?page=N`
  - Event definitions: `GET /v1/environments/:eid/event-definitions?page=N`
  - SDK keys: `GET /v1/management/environments/:eid/sdk-keys?page=N`
  - Org users: `GET /v1/admin/orgs/:oid/users?page=N`
  - Audit log: `GET /v1/audit?page=N` (ordered by created_at DESC)
  - Each: paginated repository method + route query param + sqlx::test

- [ ] Task 5: Frontend pagination component + wire-up
  - `admin/src/components/Pagination.tsx`: prev/next + page indicator, disabled states
  - Wire into FlagsList and SegmentsList: read page from URL search param, push on change
  - Vitest: renders correct page state, onChange fires correct page number, disables
    prev on page 1 and next on last page

- [ ] Task: Conductor - User Manual Verification 'Offset Pagination' (Protocol in workflow.md)
