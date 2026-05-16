# Implementation Plan: Database & Query Optimizations

Track: `db_optim_20260516`

---

## Phase 1: PostgreSQL Index Layer
<!-- execution: sequential -->

- [x] Task 1: SDK key hash composite index migration (fe931df)
  - 20260516000001: `CREATE INDEX IF NOT EXISTS idx_sdk_keys_key_hash_active ON sdk_keys(key_hash, is_active)`
  - Note: migration uses CREATE INDEX (transactional); production deploy should use CONCURRENTLY manually
  - sqlx::test: sdk_key_hash_index_find_active_returns_matching_key + revoked_key_not_returned

- [x] Task 2: Soft-delete partial indexes migration (fe931df)
  - 20260516000002: 6 partial indexes WHERE deleted_at IS NULL for
    feature_flags, segments, projects, environments, event_definitions, experiments
  - Note: variants table has no deleted_at — skipped; CONCURRENTLY for production
  - sqlx::test: soft_delete_partial_index_flags_excludes_deleted + segments_excludes_deleted

- [x] Task 3: Segment list entry covering index migration (fe931df)
  - 20260516000003: drop idx_segment_list_entries_lookup, create idx_segment_list_entries_covering
    (segment_id, context_type, list_type, entry_key)
  - sqlx::test: segment_list_covering_index_member_found + non_member_returns_false

- [x] Task 4: Context registry purge indexes migration (fe931df)
  - 20260516000004: idx_context_type_registry_last_seen + idx_context_param_registry_last_seen
  - sqlx::test: context_registry_last_seen_index_purge_removes_old_types + old_params

- [ ] Task: Conductor - User Manual Verification 'PostgreSQL Index Layer' (Protocol in workflow.md)

---

## Phase 2: Segment Batch Load — N+1 Elimination [checkpoint: 74e347e]
<!-- depends: phase1 -->
<!-- execution: sequential -->

- [x] Task 1: Batch repository methods on PgSegmentRepository (010ac1c)
  - `find_batch_by_ids(ids: &[SegmentId]) -> Vec<Segment>`
    → `WHERE id = ANY($1) AND deleted_at IS NULL`
  - `find_rules_batch(ids: &[SegmentId]) -> HashMap<SegmentId, Vec<SegmentRule>>`
    → `WHERE segment_id = ANY($1) ORDER BY segment_id, rule_index`
  - `find_lists_batch(ids: &[SegmentId]) -> HashMap<SegmentId, Vec<SegmentListEntry>>`
    → `WHERE segment_id = ANY($1)`
  - Add methods to `SegmentRepository` trait; implement on `PgSegmentRepository`
  - sqlx::test for each: seed N segments, verify result count and ordering

- [x] Task 2: Refactor fetch_segment_definitions in stitchd-flag-service
  - Replace per-segment loop in `service.rs` with three bulk calls
  - Mock repository test: 10 segments → each batch method called exactly once
  - Integration test: definition fetch issues ≤ 4 total DB queries regardless of segment count

- [x] Task: Conductor - User Manual Verification 'Segment Batch Load' (Protocol in workflow.md)

---

## Phase 3: SDK Key In-Process Cache [checkpoint: e830042]
<!-- depends: -->
<!-- execution: sequential -->

- [x] Task 1: Add moka dependency + update tech-stack.md
  - `moka = { version = "0.12", features = ["future"] }` in stitchd-auth-service Cargo.toml
  - Document cache pattern under "Caching" section in tech-stack.md

- [x] Task 2: Implement SdkKeyCache
  - `struct SdkKeyCache(Cache<String, SdkKey>)` keyed on `key_hash`, TTL = 60s
  - `async fn get_or_load(&self, hash: &str, loader: F) -> Result<SdkKey, RepositoryError>`
  - Unit tests: hit skips loader, miss invokes loader once, TTL expiry retriggers loader,
    invalidate removes entry, concurrent gets coalesce to single loader call

- [x] Task 3: Wire cache into validate_sdk_key and revocation path
  - Inject `SdkKeyCache` into `AuthServiceImpl` (new field, constructed in main)
  - `validate_sdk_key` uses `cache.get_or_load(hash, || repo.find_active_by_hash(hash))`
  - SDK key revocation: call `cache.invalidate(key_hash)` after DB revoke
  - Unit test: revoked key not returned after invalidation within TTL window

- [x] Task: Conductor - User Manual Verification 'SDK Key Cache' (Protocol in workflow.md) (Protocol in workflow.md)

---

## Phase 4: ClickHouse Overhaul [checkpoint: 4ba06fa]
<!-- depends: -->
<!-- execution: sequential -->

- [x] Task 1: Parameterize eval_stats query — injection fix (314882f)
  - Validate `flag_id` path param as UUID at handler entry → HTTP 400 on parse failure
  - Replace `format!()` SQL in `routes/eval_stats.rs` with clickhouse-rs bind parameters
  - Unit tests: invalid UUID → 400; valid UUID → query executes; no raw interpolation in SQL

- [x] Task 2: Experiment materialized views — schema + migration files (e5504ad)
  - `events_experiment_daily` (AggregatingMergeTree) in 20260516000005
  - MV trigger `events_experiment_daily_mv` auto-populates on events insert
  - 2 integration tests: MV populates on insert; non-experiment events excluded

- [x] Task 3: Backfill migration — populate MVs from raw events (e5504ad)
  - 20260516000006: TRUNCATE + INSERT .. SELECT (idempotent)
  - Integration tests: seed raw events, run backfill, assert MV row counts match

- [x] Task 4: Rewrite experiment_queries.rs to read from MVs (9651898)
  - build_count_metric_sql reads events_experiment_daily with countMerge/uniqMerge
  - query_numeric_metric stays on raw events (MV lacks quantile states)
  - 27 unit tests pass; function signatures preserved (no proto/API changes)

- [x] Task 5: Partition tuning — weekly partitions for flag_evaluation_log and events (4ba06fa)
  - 20260516000007_events_v2: weekly toMonday partitions + INSERT SELECT backfill
  - 0004_flag_evaluation_log_v2.sql: same + TTL preserved; manual rename documented
  - 3 integration tests: inserts queryable, toMonday key verified, row counts match

- [x] Task: Conductor - User Manual Verification 'ClickHouse Overhaul' (Protocol in workflow.md) [checkpoint: 2ad381e]

---

## Phase 5: Offset Pagination
<!-- depends: -->
<!-- execution: sequential -->

- [x] Task 1: Shared pagination types in stitchd-gateway (b53e0ee)
  - `struct PaginationParams { page: u32, per_page: u32 }` — default page=1,
    per_page=50, max cap=200 (enforced in extractor)
  - `struct PaginatedResponse<T> { items: Vec<T>, total: u64, page: u32, per_page: u32 }`
  - Unit tests: defaults applied, per_page capped at 200, page=0 normalised to 1

- [x] Task 2: Paginate flag list endpoint (b53e0ee)
  - Repository: `list_by_project_paginated(project_id, offset, limit) -> (Vec<Flag>, u64)`
    using `COUNT(*) OVER()` window function
  - Route: `GET /v1/projects/:pid/flags?page=N&per_page=N` → `PaginatedResponse<FlagJson>`
  - Backwards-compatible: no params → page 1, per_page 50
  - sqlx::test: page 1 of 2, page 2 of 2, total count accurate

- [x] Task 3: Paginate segment list endpoint (65c63ff)
  - Repository: `list_by_environment_paginated(env_id, offset, limit) -> (Vec<Segment>, u64)`
  - Proto: page/per_page on ListAdminSegmentsRequest, total on ListAdminSegmentsResponse
  - Route: `GET /v1/segments?env_id=...&page=N` → `PaginatedResponse<AdminSegmentJson>`
  - sqlx::test: 4 tests — page slicing, remainder, total count, empty env

- [x] Task 4: Paginate remaining admin endpoints (07e8a40, d010fc7)
  - Experiments: list_by_environment_paginated + proto page/per_page/total + gateway PaginatedResponse
  - Event definitions: stub updated to return PaginatedResponse shape (no proto RPC exists)
  - SDK keys: list_by_environment_paginated + management proto + auth-service handler
  - Org users: list_by_organisation_paginated + management proto + auth-service handler
  - Audit log: skipped (route does not exist yet)
  - sqlx tests: experiment (24 pass), sdk_key_extended (11 pass)

- [x] Task 5: Frontend pagination component + wire-up (485a35a)
  - `admin/src/components/Pagination.tsx`: prev/next buttons, page/total indicator, disabled states
  - `PaginatedResponse<T>` type added to shared types
  - FlagsList: reads ?page from URL, fetches with page/per_page, shows Pagination on table view
  - SegmentsList: same pattern — URL-driven page, paginated fetch, Pagination below table
  - 14 Vitest tests covering totalPages, isFirst/isLast, from/to range, onChange (134 total pass)

## Phase 5: Offset Pagination [checkpoint: 485a35a]

- [x] Task: Conductor - User Manual Verification 'Offset Pagination' (Protocol in workflow.md) [checkpoint: f9e4b61]
