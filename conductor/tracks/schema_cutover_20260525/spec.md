# Spec: Schema Hard Cutover — V1 Baseline + Legacy Cleanup

## Overview

All customers are on the current schema. This track treats deployment as
fresh-start: one canonical V1 baseline file per database defines the complete
final schema. No backward-compatibility migrations, no DROP cleanup scripts.
Existing deployments manage their own cleanup.

Alongside the schema consolidation, three legacy dual-read/dual-write patterns
in Rust are retired, two JSONB columns move to structured Postgres composite
types, one percentage field gains 0.01% integer precision, and one nullable
column is tightened.

## Functional Requirements

### A. Migration Collapse

**A1.** Write `crates/stitchd-db/migrations/20260525000001_v1_baseline.sql` —
the complete final Postgres schema (all tables, indexes, views, functions, with
every ALTER/DROP applied into the final shape). Delete all 44 prior numbered
files.

**A2.** Write `crates/stitchd-event-writer/migrations/20260525000001_v1_baseline.sql`
— the complete final ClickHouse schema: `events`, `events_v2`,
`metric_definitions` and their MVs, `experiment_iterations_active` dictionary,
`experiment_assignments` + MV, `experiment_results`, and `flag_evaluation_log`
(weekly `toMonday()` partitions, 9-column final shape including `evaluation_id`).
Delete all 13 prior numbered files in that directory.

**A3.** Delete `crates/stitchd-db/clickhouse-migrations/` entirely (0001–0006
were reference docs, never wired to a runner).

**A4.** Delete `crates/stitchd-analytics-service/clickhouse-migrations/` —
`experiment_results` is absorbed into the event-writer baseline (A2).

**A5.** Write `crates/stitchd-db/scylla-migrations/0001_v1_baseline.cql` — the
complete final ScyllaDB keyspace + table schema. Delete the 5 prior numbered
CQL files.

The V1 baselines use `CREATE TABLE IF NOT EXISTS` / `CREATE INDEX IF NOT EXISTS`
throughout — idempotent on a fresh keyspace, no-ops on a warmed one.

---

### B. JSONB → Postgres Composite Types + Precision

**B1. `feature_flag_rules.hash_inputs`** → `hash_input_selector_t[]`

The V1 baseline declares the types before the table:

```sql
CREATE TYPE hash_input_kind_t AS ENUM ('context_key', 'context_parameter');
CREATE TYPE hash_input_selector_t AS (
    kind         hash_input_kind_t,
    context_type TEXT,
    parameter    TEXT   -- NULL when kind = 'context_key'
);
```

`feature_flag_rules.hash_inputs` column type is `hash_input_selector_t[]`
(nullable — NULL for rows that carry no selectors).

Rust: implement `sqlx::Type + Encode + Decode` for `hash_input_selector_t`.
Remove the `serde_json::Value` round-trip in the flag repository read/write path.

**B2. `feature_flags.default_rule_distribution`
  and `experiment_iterations.default_rule_distribution`** → `rollout_allocation_t[]`
  with integer basis-point precision (1 = 0.01%, 10000 = 100%)

```sql
CREATE TYPE rollout_allocation_t AS (
    variant_key   TEXT,
    percentage_bp INTEGER  -- basis points: 1 = 0.01%, range (0, 10000]
);

CREATE FUNCTION rollout_sum_valid(allocs rollout_allocation_t[])
    RETURNS BOOLEAN LANGUAGE SQL IMMUTABLE AS $$
        SELECT SUM(a.percentage_bp) = 10000 FROM unnest(allocs) a $$;
```

Both columns typed as `rollout_allocation_t[]` with a CHECK constraint:
```sql
CHECK (default_rule_distribution IS NULL
       OR rollout_sum_valid(default_rule_distribution))
```

No floating-point tolerance needed — the integer sum is either exactly 10000 or it isn't.

Rust changes across `stitchd-core`:
- `hashing.rs`: `compute_raw_percentage` changes from
  `(hash % 100_000) as f64 / 1000.0` to `(hash % 10_000) as u32`.
  `calculate_allocation` return type `f64` → `u32`.
- `rollout.rs`: `RolloutAllocation.percentage: f64` → `percentage_bp: u32`.
  `assign_variant_key` input type `f64` → `u32`. Validation range `(0, 10_000]`,
  sum check `== 10_000` exactly. Remove `SUM_TOLERANCE` constant.
- `evaluation/engine.rs`: update 4 `calculate_allocation` call sites and
  2 `assign_variant_key` call sites.
- `xtask/src/verify_hash_cutover.rs`: update `calculate_allocation` call.
- Implement `sqlx::Type + Encode + Decode` for `rollout_allocation_t`.
- Remove `serde_json::to_value` / `serde_json::from_value` round-trips in flag
  and experiment repositories.
- All rollout tests: values in basis points (50% → 5000, 100% → 10000, etc.).

---

### C. Legacy Fallback Removal

**C1. Complete the `hash_inputs` cutover (flag_eval_unify_20260522 Phase 5/6)**

- `crates/stitchd-flag-service/src/mapping.rs`: remove dual-write of
  `context_hash_specs` (lines 349–370). Remove the fallback read branch at
  lines 103–113. The read path unconditionally uses `hash_inputs`.
- `crates/stitchd-gateway/src/routes/flags.rs`: remove the parallel
  `context_hash_specs` fallback reconstruction block.
- Remove test assertions that verify the dual-write contract
  (`mapping.rs` tests at lines 758–759, `705` test body).
- No DB migration needed: `hash_inputs` is already populated for all rows.
  The `context_hash_specs` field inside `rule_def` JSONB becomes inert dead data.
- Leave the `context_hash_specs` proto field declared but stop writing to it
  (proto field removal is a separate breaking-change track).

**C2. Retire `segment_rules` table**

The V1 Postgres baseline does not include `CREATE TABLE segment_rules`.
On a fresh deployment the table never exists.

Rust code changes:
- `crates/stitchd-db/src/repository/pg/segment.rs`: remove the
  `segment_rules`-first read path in `find_with_rules` (lines 389–423) and
  `find_rules_batch` (lines 583–650). The sole read path becomes
  `get_condition_expr`.
- Remove `upsert_rules()` method from the repository trait and implementation.
- `crates/stitchd-segmentation-service/src/grpc/service.rs`: remove
  `upsert_rules` call sites at lines 866 and 917.
- Remove the `SegmentRule` domain struct if it has no remaining references.

**C3. Fix dead `flag_evaluation_log_v2` reference**

`crates/stitchd-stats-service/src/context_refresher.rs:125`: change query
from `flag_evaluation_log_v2` to `flag_evaluation_log`.

---

### D. Nullable Column Tightening

**D1. `event_definitions.name` — NOT NULL in baseline**

The V1 Postgres baseline declares `name TEXT NOT NULL`. On a fresh deployment
no NULL rows exist.

Rust: remove `name.unwrap_or_else(|| key.clone())` at
`crates/stitchd-db/src/repository/pg/event_definition.rs:82`.
Change the field from `Option<String>` to `String`.

---

## Non-Functional Requirements

- `cargo test --workspace` passes against a fresh docker compose up running
  only the V1 baseline files.
- `SQLX_OFFLINE=true cargo check --workspace` passes (sqlx offline cache
  regenerated after all Postgres schema changes).
- `cargo clippy --workspace --all-targets -- -D warnings` passes.
- No new columns, indexes, or API surface beyond what is specified.

## Acceptance Criteria

- [ ] One migration file per database; all prior numbered files deleted.
- [ ] `flag_evaluation_log_v2` absent from all code and migrations.
- [ ] `segment_rules` absent from V1 Postgres baseline and all Rust source.
- [ ] `context_hash_specs` not written by `mapping.rs` or `routes/flags.rs`;
      fallback read path removed.
- [ ] `feature_flag_rules.hash_inputs` is `hash_input_selector_t[]` in the
      baseline and in Rust sqlx types.
- [ ] `feature_flags.default_rule_distribution` and
      `experiment_iterations.default_rule_distribution` are
      `rollout_allocation_t[]` with sum CHECK constraints.
- [ ] `RolloutAllocation.percentage_bp` is `u32`; `calculate_allocation`
      returns `u32`; `SUM_TOLERANCE` is gone.
- [ ] `event_definitions.name` is `TEXT NOT NULL` in the baseline; no Rust
      fallback to `key`.
- [ ] `cargo test --workspace` passes.
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` passes.
- [ ] `SQLX_OFFLINE=true cargo check --workspace` passes.

## Out of Scope

- Backward-compatibility migrations for existing deployments.
- DROP TABLE / DROP COLUMN cleanup scripts.
- Proto field removal for `context_hash_specs`.
- Any new columns, indexes, API surface, or admin UI changes.
