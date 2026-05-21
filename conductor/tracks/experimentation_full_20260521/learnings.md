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

## [2026-05-21] - Phase 2 Tasks 2.1 + 2.2 + 2.3: matched_rule_id wiring + default-rule distribution + E2E test

- **Implemented:** Three sequential commits — (0f0516c) plumb matched_rule_id through eval_log_writer + sdk_backend::event_to_row + evaluation::preview::ContextPreviewResult.fired_rule_id; (9d02142) default-rule percentage distribution evaluation in FlagEvaluator + evaluate_preview, with rollout_debug populated; (9313c75) E2E integration test driving FlagSdkBackendServiceImpl in-process against real CH.
- **Files changed:** 11 source files + 1 new test file. 24 new unit/integration tests added across rollout / engine / preview / eval_log_writer / sdk_backend / e2e.
- **Verification gate:** cargo fmt --all --check clean. cargo clippy --workspace --all-targets -- -D warnings clean. `cargo test -p stitchd-flag-service -p stitchd-core` → 443 + 77 + 1 e2e + 1 eval_preview_clickhouse all pass. CH inspection confirms the (targeting_on, matched_rule_id) shape per scenario.
- **Learnings:**
  - **Architectural finding — eval_log_writer.rs is a forward-looking helper, not the live path.** The production eval-log write path is `sdk_backend::event_to_row` (driven by SDK IngestSdkEvalLog). `service.rs::evaluate_preview` intentionally does NOT write to flag_evaluation_log (documented inline). `eval_log_writer.rs::{spawn_eval_log_write, build_eval_log_rows}` exists for a future server-side evaluation path. Phase 2 still wires the helper signature so any later caller carries the matched_rule_id contract.
  - **Gotcha — `cargo clippy::too_many_lines` on `async_trait`-expanded impl blocks.** Adding `default_rule_distribution Jsonb` in Phase 1 bloated 7+ SELECT-to-FlagRecord mapping sites in `crates/stitchd-db/src/repository/pg/flag.rs`. The `async_trait` macro expands the entire impl into a single function, so its line count crossed the lint threshold (81/80). Fix: extract a small `assemble_flag_from_row(&PgRow) -> Result<FlagRecord>` helper and replace each verbose mapping call. Keeps the public API identical.
  - **Default-rule distribution hash convention.** Reuses `calculate_allocation(flag_key, env_id_str, [ctx.key for each context])` — same primitive as percentage rollout rules — so cohort assignment is shared across both code paths. PercentageTarget-style configurability (param-based hashing, multi-target ordering) is NOT exposed on the default-rule distribution (spec doesn't include it); the canonical convention is "all present-context keys, in iteration order".
  - **Distribution → unknown variant_key behaviour.** When `default_rule_distribution.allocations[i].variant_key` doesn't exist on the flag's variants list, evaluation falls back to `default_variant_id` with `tracing::warn!` (NOT an error). Keeps the eval path resilient to drift between the distribution config and the variants table.
  - **`stitchd-core` now depends on workspace `tracing`.** Was previously missing despite being used by other crates. Needed for the unknown-variant warning emit. Added under existing workspace dep.
  - **Defensive invariant in eval_log_writer.** When `targeting_on = false`, `build_eval_log_rows` and `event_to_row` both force `matched_rule_id` to `None` — even if a misbehaving caller / SDK includes a non-empty value. This is load-bearing for the Phase 4 `experiment_assignments_mv` invariant: disabled-flag evals must never produce experiment exposures.
  - **Per-row tuple shape on `EvalContextRow`.** Changed `build_eval_log_rows` signature from `&[(EvaluationContext, String)]` to `&[(EvaluationContext, String, Option<Uuid>)]` so the matched_rule_id can vary per-context within a single batch (e.g. one batch where alice matched rule A and bob fell through to default-rule). Type alias `EvalContextRow` exposed on the module for callers.
  - **In-process tonic-impl tests > full tonic-stream tests for single-RPC scenarios.** Phase 2 Task 2.3 calls `FlagSdkBackendServiceImpl::ingest_sdk_eval_log` directly (no listener / no incoming-stream setup). Faster, simpler, no port conflicts. The boundaries_20260518 pattern (TcpListener::bind 0 + serve_with_incoming) remains for tests that need to exercise the wire path (e.g. metadata handling, status codes, codec).
---
