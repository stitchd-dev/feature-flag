# Track Learnings: experimentation_full_20260521

Patterns, gotchas, and context discovered during implementation.

## Codebase Patterns (Inherited)

The following patterns from `conductor/patterns.md` are load-bearing for this track. Re-read before starting each phase.

### ClickHouse (critical for Phases 4-5)
- **AggregatingMergeTree insert/read combiners** — use `*State` on insert, `*Merge` on read; never `finalizeAggregation` in aggregated context.
- **`sumState(Nullable(Float64))` type mismatch** — wrap with `ifNull(expr, 0.0)`.
- **Weekly partition key** — `toMonday(event_date)` + `TTL ... + INTERVAL 52 WEEK`.
- **`toFloat64OrNull` only accepts String** — never wrap numeric columns; use it ONLY for `properties['<key>']` String → Float64 coercion.
- **`clickhouse-rs` binds `?` placeholders by SQL position, not vec index** — push binds in SQL-appearance order or values bind to the wrong placeholders (cryptic `Cannot parse uuid <step-event-key>` errors). Critical for funnel + assignment JOIN queries.
- **`CREATE INDEX CONCURRENTLY` inside a transaction** — sqlx migrations wrap each file in a transaction; split into separate files for production.

### Architecture
- **Gateway lean principle** — gateway holds only gRPC clients; never direct DB. New endpoints (`/exposures`, `/timeseries`, `/recompute`, `/default-rule-distribution`) must call existing or new gRPC RPCs.
- **Fire-and-forget gRPC for analytics** — `tokio::spawn` for non-critical telemetry calls; log errors, don't block.
- **Admin vs SDK response shape** — separate `AdminFooJson` (full data) from SDK-facing `FooJson` (minimal). Don't bloat SDK responses for UI needs.
- **Bool-type flag invariant** — boolean flags always have exactly 2 variants (`true`/`false`). The `default_rule_distribution` for a bool flag must respect this.
- **gRPC service registration gotcha** — implementing `impl XxxService for Impl` is not enough; must also `.add_service(XxxServiceServer::new(impl_))` in `main.rs`.
- **Stale worktree binary on shared port** — when restarting services for testing, `ps -o comm=` to verify the binary serving a port is from the current worktree.

### Events + Metrics (from `events_metrics_20260519`)
- **Discriminated-union serde + protobuf oneof alignment** — tagged-union Rust enums with `#[serde(tag = "kind", rename_all = "snake_case")]` + `#[serde(flatten)]` match protobuf `oneof` wire format exactly.
- **Adding tonic RPCs breaks every impl — fix MockServices too** — grep `impl <Service> for` and add `Unimplemented` stubs to MockServices when adding RPC methods.
- **Proto field deprecation needs `reserved`** — `reserved <N>; reserved "<name>";` for any deleted tag.
- **`Count` metric_type is an occurrence marker** — ingestion accepts missing `value`; SDKs omit it.

### Admin UI (Frontend)
- **Formik + Yup is the only form pattern** — never ad-hoc `useState` for form state. Primitives in `admin/src/components/form/`; schemas in `admin/src/lib/validation/`.
- **`validateOnChange={false}` for async Yup validators** — prevents per-keystroke API calls.
- **`enableReinitialize` for async-loaded edit forms** — needed when `initialValues` depend on async data.
- **`key={mode}` for mode-switching Formik forms** — forces remount + validation reset.
- **`verbatimModuleSyntax`** — use `import type` for type-only imports.
- **TypeScript CLI** — `node_modules/.bin/tsc --noEmit -p tsconfig.app.json`; never `npx tsc`.
- **Admin UI modal primitives** — reuse `Modal`, `Dropdown`, `EmptyState`, `LockOverlay` from `admin/src/components/`.
- **RBAC UI gating pattern** — `disabled` + `opacity: 0.35` (never `display:none`) for permission-gated actions.

### Auth + Gateway
- **RBAC permissions expanded from role in `crates/stitchd-auth-service/src/rbac.rs`** — explicit role→permission expansion required.
- **`require_non_system_org` middleware** — management routes (including new experiment + flag endpoints) sit behind both JWT auth and this middleware.
- **Shared `require_permission` in `routes/mod.rs`** — all sub-modules use `super::require_permission`.

### Cargo + Worktree
- **Cargo must run from the worktree root** — `cd .worktrees/experimentation_full_20260521/` or `cargo -C <worktree_path>`.
- **`bd close --no-auto` is mandatory for parallel waves** — prevents beads from claiming downstream tasks before orchestrator verifies the milestone.
- **Fix gaps as discovered** — inline-fix in-scope bugs; file beads bug (`bd create --priority 2`) for out-of-scope issues.

### Rust 2024
- **`std::env::set_var` is `unsafe`** — wrap in `unsafe {}` with `// SAFETY:` comment.
- **`gen` is reserved in Rust 2024** — use `active_gen`, `cur_gen`, `generation`.
- **Recursive types** — use `Box<T>` for recursive variants (e.g., distribution AST if any).

### Dep Constraints
- **`major.minor` is workspace canon** — pin to `major.minor`, not bare-major or patch.
- **Rust toolchain stays on `stable`** — `rust-toolchain.toml` + `dtolnay/rust-toolchain@stable`; MSRV in `[workspace.package].rust-version` is the sole enforcement.
- **`tonic 0.14` codec split** — `tonic-prost` (runtime) + `tonic-prost-build` (build); `tonic_prost_build::configure()`.
- **`clickhouse 0.15` async insert** — `client.insert::<Row>("table").await?`; explicit type annotation required.

---

<!-- Learnings from implementation will be appended below -->

## [2026-05-21] - Phase 1 Task 1: PG schema migrations for experiment attribution

- **Implemented:** Three PG migrations (`20260521000001_experiment_attribution_fields`, `20260521000002_flag_default_rule_distribution`, `20260521000003_experiment_iterations_snapshot`) + a 9-test sqlx schema-level test suite (`crates/stitchd-db/tests/experiment_attribution_schema.rs`).
- **Files created:** 3 migrations + 1 test file.
- **Learnings:**
  - **Gotcha — `array_length(arr, 1)` returns NULL for empty arrays in PostgreSQL, not 0.** A `CHECK (array_length(...) >= 1)` therefore evaluates to NULL on `{}` input, and CHECK treats NULL as passing. **Use `cardinality(arr) > 0`** for non-empty enforcement.
  - **Sequencing constraint:** `sqlx::query_as!` macros in `crates/stitchd-db/src/repository/pg/experiment.rs` query the live DB schema at compile time. Making `flag_rule_id` nullable in the DB (without also updating the `Experiment` struct to `Option<RuleId>`) breaks workspace compilation. → Phase 1 Tasks 1, 3, 4 must land as one atomic commit. The current commit lands the migration FILES + schema tests only; the live DB has been rolled back and the migrations remain unapplied until Tasks 3 + 4 land.
  - **Pattern:** Use raw `sqlx::query(r"...")` strings (not the macro) in tests targeting new schema (per `boundaries_20260518` pattern).
- **Schema design decisions:**
  - Added `flag_id UUID NOT NULL` on `experiments` to support whole-flag uniqueness without rule joins on the hot path.
  - Replaced `idx_experiments_one_active_per_rule` with `idx_experiments_one_active_per_flag` — matches whole-flag-lock semantics.
  - Used `targets_default_rule BOOLEAN` + XOR CHECK rather than a sentinel `flag_rule_id` UUID.
---

## [2026-05-21] - Phase 1 Tasks 2/3/4/1.5: domain model + repo + sqlx cache + CH migration (atomic landing)

- **Implemented:** Full Phase 1 codified — CH migration with `targeting_on` (renamed from `is_disabled` per user direction) + `matched_rule_id`; new `RolloutDistribution` type with full validation; `Experiment` struct gains 6 new fields; `FlagRecord.default_rule_distribution`; all repo queries updated; `.sqlx` cache regenerated; PG + CH migrations applied to live DBs.
- **Files changed:** 62 total — ~28 source files + new `crates/stitchd-core/src/rollout.rs` + new CH migration file + `.sqlx/` cache regen.
- **Commit:** `0e50929`
- **Verification:** 141 tests pass across `stitchd-db`, `stitchd-core`, `stitchd-flag-service`. `cargo fmt` clean. `cargo clippy --workspace --all-targets -- -D warnings` clean.
- **Learnings:**
  - **Gotcha — ClickHouse `ALTER ... DROP COLUMN` blocked by DEFAULT-expression dependency:** When you add a column with `DEFAULT (NOT is_disabled)`, then later try to `DROP COLUMN is_disabled`, CH refuses with `Cannot drop column ... because column ... depends on it`. Even after `MATERIALIZE COLUMN` persists the values, the DEFAULT expression still references the old column. Fix: `ALTER ... MODIFY COLUMN target DEFAULT <plain value>` between the MATERIALIZE and the DROP to break the dependency. `mutations_sync = 2` on the DROP ensures it returns only after the column is gone.
  - **Gotcha — sqlx `ColumnNotFound` traces back to outdated SELECT lists, not the macro layer:** When a repo method uses raw `sqlx::query(r"SELECT ...")` (not the macro), forgetting to add a new column to the SELECT silently makes `row.get("new_col")` return `ColumnNotFound`. Audit every SELECT in the file after adding a column — not just the macro-based ones. Found 3 such sites in `crates/stitchd-db/src/repository/pg/flag.rs` (`list_by_project_paginated`, `list_by_environment`, the UPDATE...RETURNING).
  - **Pattern — Atomic schema-domain-repo commits:** `sqlx::query_as!` macros bind to the live DB schema at compile time. Splitting schema migration → struct change → repo query update across separate commits breaks the workspace build at each midpoint. Treat them as one atomic landing: write migrations, apply, change structs + repo queries, regenerate `.sqlx`, then commit. (Plan-wise: keep Phase 1 Tasks 1+3+4 as a single phase even though they look discrete — the sqlx coupling forces atomicity.)
  - **Pattern — `cardinality(arr) > 0` over `array_length(arr, 1) >= 1` for PG CHECK constraints (re-confirmed from Phase 1 Task 1).**
  - **Pattern — Per-flag uniqueness replaces per-rule:** When the lock model shifts from "one experiment per rule" to "one experiment per flag" (whole-flag-lock semantics), the partial unique index must move with it: `idx_experiments_one_active_per_flag` on `(flag_id) WHERE status IN ('running','paused')`. The `UniqueViolation { field }` returned by sqlx then reports `flag_id` rather than `flag_rule_id` — test assertions on the field name must update.
  - **Gotcha — Partial-completion sub-agent state leaks across rejections:** When an Agent invocation is rejected mid-execution, the file edits already committed to disk persist (no transactional rollback). On resume, audit `git status --short` carefully — there were ~10 file edits + 1 new file (`rollout.rs`) sitting uncommitted from the rejected Phase-1 Agent in the previous session. Most were useful (cleanly written domain model + proto-adjacent changes); a few needed completion (e.g. `assemble_flag` callers weren't updated to pass the new arg). Always inspect orphaned changes before continuing — they may save substantial work.
---

## [2026-05-21] - Phase 3 Tasks 1-3: Flag Lock Enforcement (worker_p3)

- **Implemented:** Whole-flag lock enforcement end-to-end.
  - Task 3.1: New repo method `ExperimentRepository::find_active_experiment_for_flag` + PG impl; new module `crates/stitchd-flag-service/src/flag_lock.rs` owns `FlagLockCache` (moka `time_to_live(30s)`) + `is_flag_locked(repo, cache, flag_id)`. `FlagServiceImpl::new(...).with_experiment_repo(...)` wires the cache + repo; private `ensure_flag_unlocked` guards every admin mutation path (`mutate_flag` Update/Delete/Archive + `update_flag_hashing`).
  - Task 3.2: Gateway `GatewayError::FlagLockedByExperiment { experiment_id }` decodes a `flag_locked_by_experiment:<uuid>` sentinel prefix from `tonic::Status::failed_precondition` into HTTP 409 with structured body. Also adds `GatewayError::ExperimentBindingInvalid { code, message }` mapped to HTTP 422 (consumed by Task 3.3). Integration test in `crates/stitchd-gateway/tests/flag_lock_integration.rs` spins up an in-process tonic mock FlagService and asserts PUT/DELETE/variants → 409 + valid body, while the success path still returns 200.
  - Task 3.3: `validate_experiment_binding(...)` in `routes/experiments.rs` enforces the four binding invariants from spec §2 — INVALID_RULE_KIND (incl. XOR violations), INVALID_DEFAULT_RULE_KIND (partial: XOR-only for Phase 3; full default_rule_distribution lookup is Phase 7), EMPTY_UNIT_CONTEXT_TYPES, UNKNOWN_CONTEXT_TYPE (via existing `AnalyticsService.ListContextTypes` RPC).
- **Files:** 14 changed (~1.6 kLOC added)
  - new: `crates/stitchd-flag-service/src/flag_lock.rs`, `crates/stitchd-gateway/tests/flag_lock_integration.rs`
  - modified: `crates/stitchd-db/src/repository/{mod,pg/experiment,pg/flag}.rs`, `crates/stitchd-flag-service/src/{error,lib,main,service}.rs`, `crates/stitchd-flag-service/Cargo.toml`, `crates/stitchd-gateway/src/{error,routes/experiments}.rs`, `crates/stitchd-experimentation-service/src/service.rs`, `crates/stitchd-analytics-service/src/grpc/metric.rs`, `crates/stitchd-stats-service/src/recompute_trigger.rs`
- **Commits:** `088538a` (3.1), `8d95b54` (3.2), `8cd1c3a` (3.3)
- **Verification:** `cargo fmt --all --check` clean. `cargo clippy --workspace --all-targets -- -D warnings` clean. `cargo test -p stitchd-flag-service -p stitchd-gateway` — 294 tests pass (78 flag-service unit + 211 gateway unit + 4 gateway integration + 1 doctest scaffold).
- **Learnings:**
  - **Pattern — Sentinel-prefix message for cross-service typed errors over a raw `tonic::Status`:** Adding a structured 409 to the gateway without touching protobuf requires encoding the typed information (here: the locking `experiment_id`) into the `Status::failed_precondition` message via a stable prefix (`flag_locked_by_experiment:<uuid>`). The gateway's `From<tonic::Status>` checks the prefix BEFORE the generic `FailedPrecondition → Conflict` mapping kicks in. Cleaner than `tonic::Status::with_details` (Any) for one-off cases and keeps the gateway gRPC-typing surface minimal.
  - **Pattern — Closure-injected lookups in route validators:** `validate_experiment_binding<F, FFut, G, GFut>(...)` takes the `flag_lookup` + `context_types_lookup` as generic async closures so unit tests stub them with plain `|_, _| async { Ok(...) }` instead of spinning up gRPC mocks. Production callers pass a thin wrapper that clones the `GatewayState` `Arc` into the closure and awaits the real RPC.
  - **Gotcha — Cross-binary cache invalidation:** The prompt asks for `flag_lock_cache.invalidate(flag_id)` to be called from `experimentation_service.apply_transition`. In production those services are separate binaries — the cache is in flag-service process memory. Without a new gRPC RPC (`InvalidateFlagLockCache`), the experimentation-service cannot reach across to invalidate. The 30s TTL bounds the staleness window; an annotation in `transition_experiment` documents the deferral. A future cleanup task should add the RPC.
  - **Gotcha — `routes::flags::test_router` is `#[cfg(test)]` and invisible to integration tests:** Integration tests live in `crates/<crate>/tests/` and can only call `pub` items not gated on `#[cfg(test)]`. Either build a focused router inline in the integration test (chosen), expose the test_router non-conditionally (pollutes API), or move the test next to the route module (loses end-to-end framing). The inline rebuild is ~15 lines and stays in the integration test file.
  - **Gotcha — Pre-existing clippy::too_many_lines on `PgFlagRepository::update`:** The function's 87-line body (SQL + result mapping + version-conflict probe + audit log) exceeds the default 80-line limit. Added a targeted `#[allow]` with justification rather than refactor — the function is still a single linear step sequence and splitting it hurts readability more than it helps. The lint must be resolved (or the limit raised) before any other change in `flag.rs` builds under `-D warnings`.
  - **Gotcha — Adding a method to `ExperimentRepository` cascades to four impls:** PgExperimentRepository + three test-only mocks in `crates/stitchd-{analytics,experimentation,stats}-service/src/*`. All four needed `find_active_experiment_for_flag` stubs returning `Ok(None)` to keep the workspace compiling. This mirrors the patterns.md "Adding tonic RPCs breaks every impl" gotcha — same problem class for non-tonic traits.
  - **Out-of-scope bug filed:** `feature-flag-1dk` — 12 analytics-service tests fail with a CH SYNTAX_ERROR applying the Phase 1 migration `20260521000001_flag_eval_log_matched_rule`. Pre-existing on the worker branch baseline (`df04174`); does not affect Phase 3 verification scope.
---
