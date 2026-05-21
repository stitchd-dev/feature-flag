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

## [2026-05-22] - Phase 5 Tasks 5.1-5.5: Stats Service Query Cutover (worker_p5)

- **Implemented:** Five commits cutting the experiment-stats query family over from SDK-tagged event-context-tuple attribution to the Phase 4 `experiment_assignments` JOIN model. Per-context-type result builder + ClickHouse schema extension for `experiment_results.context_type` close out the phase.
  - Task 5.1 (`e83e637`): `build_aggregation_query` now JOINs `events_v2 e` against `experiment_assignments a` on `(env_id, context_type, context_key)` via `arrayExists(t -> t.1 = a.context_type AND t.2 = a.context_key, e.contexts)`. Enforces strict ITT (`e.occurred_at >= a.assigned_at AND e.occurred_at < iteration_end`). GROUP BY `(a.context_type, a.variant_key)`. Adds `iteration_end: DateTime<Utc>` parameter (bound as milliseconds via `fromUnixTimestamp64Milli`).
  - Task 5.2 (`40ec90a`): `build_ratio_query` keeps the three-CTE shape (numerator + denominator + denominator_count) but joins them per `(context_type, variant_key)` so multi-context-type experiments don't mis-pair numerator and denominator rows across context types.
  - Task 5.3 (`d8e3ab0`): `build_funnel_query` runs `windowFunnel` over assigned-context events with dedup key `(a.context_type, a.context_key)`. Per-step UNION-ALL outer SELECT preserved. New `funnel_binds_are_in_sql_appearance_order_three_step` test scans the SQL string left-to-right and asserts {pN} placeholder numbers are monotonically increasing — pins the `clickhouse-rs` SQL-position binding contract.
  - Task 5.4 (`e1e13da`): NEW `build_experiment_preview_aggregation_query` alongside the existing standalone preview builders. Day-buckets by `toStartOfDay(e.occurred_at, 'UTC')` and groups by `(day_ts, a.context_type, a.variant_key)`.
  - Task 5.5 (`d274b03`): `experiment_results` CH table gains `context_type LowCardinality(String) DEFAULT 'user'`. Proto `WriteExperimentResultsRequest` + `ExperimentResult` carry context_type. New `build_metric_summaries(points_per_metric, freq_per_pair, bayes_per_pair, recs_per_pair) -> Vec<MetricSummary>` groups per-(metric_key, context_type). Stats-service `results_writer` forwards context_type on every RPC.
- **Files changed:** ~17 across stats-service queries (4) + tests (4) + dispatch + results_writer; analytics-service repo + grpc handler + experiment_results.rs; experimentation-service test fixture; event-writer migrations + runner; proto/analytics.proto. Plus 4 new `tests/<query>_query.rs` integration files (all `#[ignore]`'d since they need a running CH).
- **Verification gate (autonomous):**
  - `cargo fmt --all --check` → OK.
  - `cargo clippy --workspace --all-targets -- -D warnings` → OK (after adding `#[allow(clippy::too_many_arguments)]` to `build_ratio_query` with justification — 8 args is the cleanest expression of "ratio = num+den+den_count under shared experiment scope + iteration_end").
  - `cargo test -p stitchd-stats-service -p stitchd-db --lib --tests` → 138 stats-service lib tests pass (was 126 before Phase 5).
  - Manual grep `arrayExists.*'experiment'` / `'iteration'` / `'variant'` in `crates/stitchd-stats-service/src/queries/` production code (lines before `#[cfg(test)]`) → zero matches across all four query files.
- **Learnings:**
  - **events_v2 alias `e` shared between aggregation and preview.** The `render_aggregator()` helper is `pub(super)`-shared, and after Task 5.1 emits `e.value_double` / `e.properties[...]` (qualified to disambiguate from the JOINed `experiment_assignments AS a`). Preview's standalone `build_preview_*_query` had to alias `events_v2 AS e` too even though there's no JOIN — otherwise CH errors on the qualified column refs. Test assertions in preview updated to `e.timestamp` / `coalesce(e.value_double` accordingly.
  - **`iteration_end` cannot be derived in ClickHouse.** Spec calls for `e.occurred_at < COALESCE(iteration.ended_at, now())`. The iteration table is in PG, CH can't JOIN to PG inline. Caller passes `iteration_end: DateTime<Utc>` (either `iteration.ended_at` or `Utc::now()`) as a bound `i64` via `fromUnixTimestamp64Milli`. Threading this through five signatures (aggregation + ratio + funnel + preview + dispatch_metric_query) was the largest mechanical change.
  - **Funnel bind-order discipline pinned by test.** Step predicates push to the bind vec FIRST (they appear in the CTE SELECT list, before the WHERE clause). The new `funnel_binds_are_in_sql_appearance_order_three_step` test scans the generated SQL left-to-right and asserts `{p0}, {p1}, …` appear in monotonically increasing order. This catches accidental reorderings that would manifest as the cryptic `"Cannot parse uuid <step-event-key>"` runtime error from `conductor/learnings.md`.
  - **Per-context-type JOIN keys in ratio.** Outer JOIN was `numerator.variant_key = denominator.variant_key`; now also includes `numerator.context_type = denominator.context_type`. Without this, a multi-context-type experiment would pair `(user, treatment)` numerator with `(account, treatment)` denominator and emit nonsense per-context ratios. Pinned by `ratio_joins_on_context_type_and_variant_key` test.
  - **Migration registry path deviation.** The plan called the new migration `crates/stitchd-db/clickhouse-migrations/0009_experiment_results_context_type.sql` but the prompt used `crates/stitchd-event-writer/migrations/20260521000005_experiment_results_context_type.sql`. Chose the prompt path because `stitchd-event-writer::migrations` IS the live CH migration registry; `stitchd-db/clickhouse-migrations/` does not exist. The original `crates/stitchd-analytics-service/clickhouse-migrations/0001_experiment_results.sql` is a reference doc not wired to any embedded runner — the new migration includes `CREATE TABLE IF NOT EXISTS experiment_results` (idempotent) before the `ALTER TABLE ADD COLUMN IF NOT EXISTS context_type` so future fresh deploys boot the table cleanly via the standard runner.
  - **Preview path: two parallel builder families, not a rewrite.** The prompt phrased Task 5.4 as "rewrite preview.rs" but the standalone preview surface (`POST /v1/metrics/{id}/preview`) is consumed by analytics-service and has no experiment scope to apply. Added NEW `build_experiment_preview_aggregation_query` alongside the existing standalone builders. Module doc now distinguishes the two families clearly. Phase 7 Task 3 (`GET /timeseries`) will be the consumer of the new experiment-scoped builder.
  - **Proto field additions cascade to test fixtures.** Adding `context_type` to `WriteExperimentResultsRequest` + `ExperimentResult` required updating 4 struct-literal fixtures across analytics-service + experimentation-service tests. None of them needed a real value — `"user"` (the CH default) works everywhere.
  - **`clippy::too_many_arguments` on `build_ratio_query` (8/7).** Genuine — three configs + four scope params + iteration_end. Targeted `#[allow]` with justification rather than folding into a builder struct (one consumer; long arg list is the simpler shape).
---

## [2026-05-22] - Phase 9 Tasks 9.1-9.5: Admin UI Detail Tabs (worker_p9)

- **Implemented:** Five commits — Results tab (5b59f73), Exposures+SRM panel (c6228ca), Time-series tab (1540e90), Iterations tab + recompute polling + page-level integration (9ee48f0), and the verification checkpoint (this commit). Four new components in `admin/src/pages/experiments/tabs/` are mounted on `ExperimentDetail.tsx` via a 7-tab strip (Results · Exposures · Time-series · Iterations + the existing Configuration · Metrics · Events).
- **Files added:** 8 (`Results.tsx`, `Results.test.tsx`, `Exposures.tsx`, `Exposures.test.tsx`, `Timeseries.tsx`, `Timeseries.test.tsx`, `Iterations.tsx`, `Iterations.test.tsx`). Modified: `admin/src/pages/experiments/ExperimentDetail.tsx` (removed ~310 lines of mock-flavoured viz components, added page-level fetch lifecycles for exposures + timeseries + recompute).
- **Verification gate (autonomous):**
  - `node_modules/.bin/tsc --noEmit -p tsconfig.app.json` → OK.
  - `npm run lint` → 0 errors, 51 warnings (16 above Phase 8's 35-warning baseline). All new warnings are the documented `react-refresh/only-export-components` category — the new tab files co-locate pure helpers + component exports (e.g. `viewToggleStorageKey` next to `<Results>`). Splitting helpers into separate files purely to satisfy Fast Refresh would obscure the testable seam; keeping them co-located matches the export pattern of `ContextTypeContext.tsx` (Phase 8.3).
  - `npm test -- --run` → 39 test files, 580 tests pass (was 500 before Phase 9 — 80 new tests across 4 new test files).
- **Learnings:**
  - **`react-dom/server.renderToString` inserts `<!-- -->` comments between adjacent text nodes.** Pinning regex assertions on a literal pair like `"14d"` fails because React emits `14<!-- -->d` to mark the text-node boundary. Anchor on a stable attribute (`data-day-preset="14"`) or the element's `aria-pressed` instead.
  - **SSR fires `useState` initializer but skips `useEffect`.** That means localStorage writes don't fire under `renderToString` — but the initial-read path still runs. Expose the read helper (`readPersistedView`, `srmHealthClass`, etc.) as the unit-testable seam, leaving the round-trip to be exercised in jsdom/playwright when those land.
  - **Off-diagonal pairwise frequentist p-values are not derivable from per-variant vs-control stats alone.** The pairwise matrix renders `—` in those cells with an inline disclaimer; direct pairwise math is deferred to a future gateway extension. Bayesian off-diagonal uses the marginal approximation `P(row > col) ≈ P(row > ctrl) × (1 - P(col > ctrl))`. Documented inline so a reader doesn't mistake the approximation for exact math.
  - **`data-winner="true"` attribute hook lets winner-row tests run without depending on CSS.** Using `style.background` would require parsing CSS variables out of the SSR string; the attribute hook is regex-stable across version updates.
  - **`results.bound_target.kind === 'default_rule'` differs subtly from `rule`.** When `kind === 'default_rule'`, the rule_id is null and the badge is decorated with a marker glyph + the verbatim label (the gateway emits "Default rule (fallthrough)"). Re-derive `srm.health` client-side from `overall_chi_sq_p` rather than trusting the gateway's `health` field — this is the spec's hard threshold (red when chi_sq_p < 0.001) and avoids stale-snapshot drift.
  - **Recompute polling uses `useRef<AbortController>` + `setTimeout(2000)` inside an async while-loop.** The cleanup function returns `cancelled = true` + `ctrl.abort()` so the loop exits cleanly on unmount. `shouldKeepPolling()` centralizes the terminal-state check so the header button, the iterations-tab button, and the polling effect all share the same predicate.
  - **The iterations history endpoint is intentionally a noop in Phase 9.** Phase 7 didn't surface `listExperimentIterations()` — Phase 11 (or a follow-up gateway track) will add it. The IterationsTab component is fully wired against an `IterationSummary[]` shape so dropping in the API wrapper later is a one-line change. The recompute button is fully functional via the existing `/recompute` + `/recompute/{job_id}` endpoints (Phase 7 Task 4).
  - **Removing legacy `FrequentistViz`/`BayesianViz`/`VariantBreakdown`/`MultiVariantMatrix`/`VizAdapter` from `ExperimentDetail.tsx` is the right cleanup pass.** These were mock-flavoured Phase 8 placeholders. Keeping them would have created two parallel viz layers (one mock, one real) and forced future maintainers to grep for which is live. The page diff dropped by 358 lines (delete) + 1054 lines (add) — net +696 lines after factoring in the 4 fetch lifecycles, but each tab is now testable in isolation.
---
