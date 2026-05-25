# Plan: schema_cutover_20260525

## Phase 1: Rollout Precision — Basis Points Migration

**Scope:** Change `calculate_allocation` and all rollout types to use `u32` basis
points (1 = 0.01%) instead of `f64` percentage. This is a pure Rust change;
no DB migration yet. Must land first — later phases and the SQL baseline depend
on the final type shape.

- [x] Task 1.1: Tests (Red) — write failing unit tests for the new `u32` basis-point
  contract in `crates/stitchd-core/src/rollout.rs`:
  - `validate_accepts_50_50_bp` (5000 + 5000 = 10000)
  - `validate_rejects_sum_not_10000`
  - `assign_variant_key_takes_u32`
  - `calculate_allocation_returns_u32` in `hashing.rs` tests
  - Basis-point boundary cases: 1 (0.01%), 10000 (100%)

- [x] Task 1.2: Implement — `crates/stitchd-core/src/hashing.rs` (fec3bf8)
  - Change `compute_raw_percentage`: `(hash % 100_000) as f64 / 1000.0`
    → `(hash % 10_000) as u32`
  - Change `calculate_allocation` return type `f64` → `u32`

- [x] Task 1.3: Implement — `crates/stitchd-core/src/rollout.rs` (fec3bf8)
  - `RolloutAllocation.percentage: f64` → `percentage_bp: u32`
  - `assign_variant_key(&self, percentage: f64)` → `assign_variant_key(&self, percentage_bp: u32)`
  - `validate()`: range check `(0, 10_000]`, sum check `== 10_000` (exact)
  - Remove `SUM_TOLERANCE` constant
  - Update all `#[error(...)]` messages to use basis points
  - Update all existing tests: multiply old `f64` values × 100 (50.0 → 5000)

- [x] Task 1.4: Implement — `crates/stitchd-core/src/evaluation/engine.rs` (6f4783a)
  - Update 4 `calculate_allocation` call sites (return value is now `u32`)
  - Update 2 `assign_variant_key` call sites (argument is now `u32`)

- [x] Task 1.5: Implement — `crates/xtask/src/verify_hash_cutover.rs` (6f4783a)
  - Update `calculate_allocation` call and any percentage comparisons

- [x] Task 1.6: Green — `cargo test -p stitchd-core` passes (544 tests, 0 failures)

- [x] Task 1.7: Conductor - User Manual Verification 'Phase 1: Rollout Precision' (Protocol in workflow.md) — 544 tests pass, pure Rust change, no UI surface

---

## Phase 2: Legacy Code Removal

**Scope:** Four independent Rust-only removals. No DB changes. Can be done in
any order within this phase.

- [x] Task 2.1: `flag_evaluation_log_v2` fix (C3) — simplest change, do first (d1ba671)
  - `crates/stitchd-stats-service/src/context_refresher.rs:125`:
    `flag_evaluation_log_v2` → `flag_evaluation_log`
  - Confirm no other references to `_v2` remain in Rust source

- [x] Task 2.2: `event_definitions.name` nullable removal (D1) (d1ba671)
  - `crates/stitchd-db/src/repository/pg/event_definition.rs:82`:
    remove `name.unwrap_or_else(|| key.clone())`; field becomes `String`
  - Adjust `SELECT_COLS` / struct mapping if `name` was `Option<String>`
  - Run `cargo test -p stitchd-db` to confirm no breakage

- [x] Task 2.3: Tests (Red) — `hash_inputs` cutover (C1) (d1ba671)
  - Write test asserting `context_hash_specs` is empty on the proto output
    for a percentage rule (verifies the dual-write is removed)
  - Write test asserting fallback branch is unreachable (e.g., a rule with
    empty `hash_inputs` and non-empty `context_hash_specs` panics or returns error)

- [x] Task 2.4: Implement — `hash_inputs` cutover (C1) (d1ba671)
  - `crates/stitchd-flag-service/src/mapping.rs`:
    - Remove `context_hash_specs` population block (lines 349–370)
    - Remove `context_hash_specs` fallback read branch (lines 103–113)
    - Remove dual-write test assertions (lines 705, 758–759 test bodies)
  - `crates/stitchd-gateway/src/routes/flags.rs`:
    - Remove `context_hash_specs` fallback reconstruction block
  - `cargo test -p stitchd-flag-service -p stitchd-gateway` passes

- [x] Task 2.5: Tests (Red) — `segment_rules` retirement (C2) (d1ba671)
  - In `crates/stitchd-db/tests/`, write integration test asserting:
    - `find_with_rules` reads exclusively from `condition_expr`
    - `upsert_rules` method no longer exists on the trait (compile-time check)

- [x] Task 2.6: Implement — `segment_rules` retirement (C2) (d1ba671)
  - `crates/stitchd-db/src/repository/pg/segment.rs`:
    - Remove `find_with_rules` `segment_rules`-first path (lines 389–423);
      body becomes a direct `get_condition_expr` call
    - Remove `find_rules_batch` `segment_rules` path (lines 583–650);
      body reads `condition_expr` for all IDs in one query
    - Remove `upsert_rules()` method from trait and impl
  - `crates/stitchd-segmentation-service/src/grpc/service.rs`:
    - Remove `upsert_rules` call sites (lines 866, 917)
  - Remove `SegmentRule` domain struct if unreferenced
  - `cargo test -p stitchd-db -p stitchd-segmentation-service` passes

- [x] Task 2.7: Conductor - User Manual Verification 'Phase 2: Legacy Code Removal' — workspace compiles clean, 15 files changed, 421 deletions (d1ba671)

---

## Phase 3: V1 Baseline SQL/CQL Files

**Scope:** Synthesise the final schema for each database into a single file.
Delete all intermediate migration files. This is the largest manual-work phase —
each baseline must be derived from the final state of all prior migrations.

- [ ] Task 3.1: Write Postgres V1 baseline
  - Read all 44 files in `crates/stitchd-db/migrations/` in timestamp order
  - Apply every CREATE / ALTER / DROP mentally to derive the final table shapes
  - Write `crates/stitchd-db/migrations/20260525000001_v1_baseline.sql`:
    - Preamble: composite type declarations (`hash_input_kind_t`,
      `hash_input_selector_t`, `rollout_allocation_t`)
    - `rollout_sum_valid` function
    - All `CREATE TABLE IF NOT EXISTS` statements in dependency order
      (parents before children — e.g., `organisations` before `environments`)
    - All `CREATE INDEX IF NOT EXISTS` statements
    - All `CREATE VIEW IF NOT EXISTS` / `CREATE FUNCTION` statements
    - `segment_rules` table is NOT included
    - `event_definitions.name` is `TEXT NOT NULL`
    - `hash_inputs` column type is `hash_input_selector_t[]`
    - `default_rule_distribution` columns type is `rollout_allocation_t[]`
      with CHECK constraint via `rollout_sum_valid`
  - Delete all 44 prior numbered `.sql` files

- [ ] Task 3.2: Write ClickHouse V1 baseline
  - Read all 13 files in `crates/stitchd-event-writer/migrations/` in order
  - Read `crates/stitchd-analytics-service/clickhouse-migrations/0001_experiment_results.sql`
  - Write `crates/stitchd-event-writer/migrations/20260525000001_v1_baseline.sql`:
    - `CREATE TABLE IF NOT EXISTS events` (ReplicatedMergeTree)
    - `CREATE TABLE IF NOT EXISTS events_v2` (final shape with properties)
    - `CREATE TABLE IF NOT EXISTS metric_definitions`
    - `CREATE MATERIALIZED VIEW IF NOT EXISTS events_count_mv`
    - `CREATE MATERIALIZED VIEW IF NOT EXISTS events_numeric_mv`
    - `CREATE MATERIALIZED VIEW IF NOT EXISTS events_experiment_daily_mv`
    - `CREATE TABLE IF NOT EXISTS flag_evaluation_log` (toMonday partitions,
      9-column final shape: env_id, flag_id, flag_key, variant_key, targeting_on,
      evaluated_at, context_type, context_key, params_json, matched_rule_id,
      evaluation_id — NOT `is_disabled`, NOT `_v2`)
    - `CREATE DICTIONARY IF NOT EXISTS experiment_iterations_active`
    - `CREATE TABLE IF NOT EXISTS experiment_assignments` (ReplacingMergeTree)
    - `CREATE MATERIALIZED VIEW IF NOT EXISTS experiment_assignments_mv`
    - `CREATE TABLE IF NOT EXISTS experiment_results` (from analytics-service migration)
  - Delete all 13 prior numbered files
  - Delete `crates/stitchd-db/clickhouse-migrations/` directory
  - Delete `crates/stitchd-analytics-service/clickhouse-migrations/` directory

- [ ] Task 3.3: Write ScyllaDB V1 baseline
  - Read all 5 files in `crates/stitchd-db/scylla-migrations/`
  - Write `crates/stitchd-db/scylla-migrations/0001_v1_baseline.cql`:
    - Keyspace DDL (from `0001_keyspace.cql`)
    - `segment_list_entries` table (from `0002`)
    - `segment_list_generations` table (from `0003`)
    - `segment_list_summary` table (from `0004`)
    - Orphaned generations handling (from `0005`)
  - Delete the 5 prior numbered `.cql` files

- [ ] Task 3.4: Conductor - User Manual Verification 'Phase 3: V1 Baseline SQL/CQL Files' (Protocol in workflow.md)

---

## Phase 4: sqlx Types + Cache Regeneration

**Scope:** Wire the new Postgres composite types into sqlx, regenerate the
offline cache, and run the full test suite.

- [ ] Task 4.1: Tests (Red) — write sqlx encode/decode round-trip tests for
  `hash_input_selector_t` and `rollout_allocation_t` in
  `crates/stitchd-db/tests/`

- [ ] Task 4.2: Implement sqlx composite type support
  - `crates/stitchd-db/src/` (or `stitchd-core` if shared): implement
    `sqlx::Type`, `sqlx::Encode`, `sqlx::Decode` for:
    - `hash_input_kind_t` (PgEnum)
    - `hash_input_selector_t` (PgRecord composite)
    - `rollout_allocation_t` (PgRecord composite)
  - Update flag repository read/write for `hash_inputs` to use the new type
    (remove `serde_json::Value` intermediary)
  - Update flag and experiment repositories for `default_rule_distribution`
    to use `rollout_allocation_t[]` (remove `serde_json::to/from_value`)

- [ ] Task 4.3: Green — `cargo test -p stitchd-db -p stitchd-flag-service -p stitchd-core` passes

- [ ] Task 4.4: Regenerate sqlx offline cache
  ```bash
  docker compose up postgres -d --wait
  cargo sqlx migrate run --source crates/stitchd-db/migrations
  SQLX_OFFLINE=false cargo sqlx prepare --workspace -- --tests
  ```
  Commit the updated `.sqlx/` directory.

- [ ] Task 4.5: Full workspace verification
  ```bash
  cargo fmt --all --check
  cargo clippy --workspace --all-targets -- -D warnings
  cargo test --workspace
  SQLX_OFFLINE=true cargo check --workspace
  ```

- [ ] Task 4.6: Conductor - User Manual Verification 'Phase 4: sqlx Types + Cache Regeneration' (Protocol in workflow.md)
