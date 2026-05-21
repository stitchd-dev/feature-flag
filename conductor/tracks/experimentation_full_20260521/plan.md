# Implementation Plan: Experimentation — Complete UI + Backend with Eval-Log-Based Attribution

## Phase 1: Data Model Foundations

- [x] Task 1: PostgreSQL migration — extend `experiments` + `feature_flags` schema [4d043ac + 0e50929]
  - [x] Sub-task 1.1: 9 schema tests in `crates/stitchd-db/tests/experiment_attribution_schema.rs`, all passing.
  - [x] Sub-task 1.2: Migration `20260521000001_experiment_attribution_fields.sql` — add columns + XOR CHECK constraint (`flag_rule_id` XOR `targets_default_rule`); also adds `flag_id NOT NULL`, replaces per-rule unique index with per-flag, uses `cardinality()` for non-empty check.
  - [x] Sub-task 1.3: Migration `20260521000002_flag_default_rule_distribution.sql` — add `default_rule_distribution Jsonb` to `feature_flags`.
  - [x] Sub-task 1.4: Migration `20260521000003_experiment_iterations_snapshot.sql` — snapshot new fields + `flag_id` + `default_rule_distribution` into `experiment_iterations`.
  - [x] Sub-task 1.5: `.sqlx/` offline cache regenerated; PG migrations applied to live DB.
- [x] Task 2: ClickHouse migration — `flag_evaluation_log` schema bump [0e50929]
  - [x] Sub-task 2.1: Schema-assert covered by `crates/stitchd-db/tests/eval_log.rs` (full-roundtrip test using new column shape).
  - [x] Sub-task 2.2: Migration `20260521000001_flag_eval_log_matched_rule.sql` — adds `targeting_on Bool` (renamed from `is_disabled` with inverted semantics per user direction) + `matched_rule_id Nullable(UUID)` to `flag_evaluation_log`. CH MATERIALIZE + MODIFY-DEFAULT used to break dependency before DROP.
- [x] Task 3: Domain model — `stitchd-core` struct updates [0e50929]
  - [x] Sub-task 3.1: `Experiment` adds `flag_id: FlagId`, `flag_rule_id: Option<RuleId>`, `targets_default_rule: bool`, `guardrail_metric_ids: Vec<MetricId>`, `pre_period_days: u32`, `unit_context_types: Vec<String>`. `ExperimentIteration` snapshots all of these + `default_rule_distribution`.
  - [x] Sub-task 3.2: `FlagRecord.default_rule_distribution: Option<RolloutDistribution>` added. New `RolloutDistribution` + `RolloutAllocation` types in `stitchd-core::rollout` with 11-test validator (non-empty, percentages in (0, 100], unique variant_keys, sum == 100 ± 0.01).
  - [x] Sub-task 3.3: Proto schema updates deferred to Phase 3 (Gateway API Surface) where the wire fields actually surface. Repo layer + domain model already carry the new fields; consumers use placeholders at the proto boundary for now.
  - [x] Sub-task 3.4: Domain↔repo mapping updated end-to-end.
- [x] Task 4: Repository layer — sqlx queries for new fields [0e50929]
  - [x] Sub-task 4.1: All experiment repo queries (`find_by_id`, `list_by_environment`, `list_by_environment_paginated`, `create`, `update`, `apply_transition`, iteration queries) updated. Per-flag uniqueness replaces per-rule.
  - [x] Sub-task 4.2: Flag repo SELECT/INSERT/UPDATE updated for `default_rule_distribution`; serializes via serde_json. Per-flag CRUD endpoint dedicated to default-rule-distribution will land in Phase 3 Task 5.
- [ ] Task: Conductor — User Manual Verification 'Data Model Foundations' (Protocol in workflow.md)

## Phase 2: Flag Service — Eval Log Enhancement
<!-- depends: phase1 -->

- [x] Task 1: Eval log writer emits `matched_rule_id` [0f0516c]
  - [x] Sub-task 1.1: TDD `eval_log_writer.rs` — assert `matched_rule_id` = rule UUID when custom rule matched, `None` when default-rule path, row skipped when disabled
  - [x] Sub-task 1.2: Wire matched rule ID through `EvalLogRow`
  - [x] Sub-task 1.3: Update flag-service evaluation hook to pass matched rule ID downstream
- [x] Task 2: Default-rule percentage-distribution evaluation [9d02142]
  - [x] Sub-task 2.1: TDD flag evaluation with `default_rule_distribution` returns a hashed variant (not the single `default_variant_id`) when distribution is set + flag enabled + no rule matched
  - [x] Sub-task 2.2: Implement distribution-based fallthrough in `crates/stitchd-core/src/evaluation/`
  - [x] Sub-task 2.3: Backwards-compat: when `default_rule_distribution` is None, fall through to `default_variant_id` (today's behavior)
- [x] Task 3: Integration test — eval log rows have correct `matched_rule_id` [9313c75]
  - [x] Sub-task 3.1: TDD against in-process flag-service: eval with rule match → row has `matched_rule_id = rule_id`; eval falling through → `matched_rule_id IS NULL`; disabled flag → no row written
- [x] Task: Conductor — User Manual Verification 'Flag Service Eval Log' (Protocol in workflow.md)

## Phase 3: Flag Lock Enforcement
<!-- depends: phase1 -->

- [ ] Task 1: Flag-lock derivation helper in `stitchd-flag-service`
  - [ ] Sub-task 1.1: TDD `is_flag_locked(flag_id) -> Option<ExperimentId>` queries experiments where `flag_id = ? AND status IN ('running','paused')`
  - [ ] Sub-task 1.2: Cache invalidation on experiment transitions (`moka` TTL = 30s)
- [ ] Task 2: Mutation guards on flag/variant/rule endpoints
  - [ ] Sub-task 2.1: TDD gateway `PATCH /flags/{key}` returns 409 `FLAG_LOCKED_BY_EXPERIMENT` with experiment ID in body
  - [ ] Sub-task 2.2: Same for variant CRUD, rule CRUD, default-rule-distribution endpoint, flag enable/disable, flag archive
  - [ ] Sub-task 2.3: Remove today's per-rule `frozen` field path (replaced by derived flag-level lock); migration to drop column if any
- [ ] Task 3: Experiment binding validator
  - [ ] Sub-task 3.1: TDD experiment create with non-percentage rule → 422 `INVALID_RULE_KIND`
  - [ ] Sub-task 3.2: TDD experiment create with `targets_default_rule=true` but flag has no `default_rule_distribution` → 422 `INVALID_DEFAULT_RULE_KIND`
  - [ ] Sub-task 3.3: TDD `unit_context_types` validates against `context_type_registry`; empty array → 422 `EMPTY_UNIT_CONTEXT_TYPES`; unknown type → 422 `UNKNOWN_CONTEXT_TYPE`
- [ ] Task: Conductor — User Manual Verification 'Flag Lock Enforcement' (Protocol in workflow.md)

## Phase 4: Attribution Pipeline — MV + Backfill
<!-- depends: phase1, phase2 -->

- [ ] Task 1: `experiment_iterations_active` ClickHouse dictionary
  - [ ] Sub-task 1.1: TDD dictionary refresh on iteration start/stop via PG-backed CH dictionary source
  - [ ] Sub-task 1.2: Migration `0006_experiment_iterations_active_dict.sql` — `CREATE DICTIONARY` keyed on `(env_id, flag_id, matched_rule_id, context_type)` → iteration_id + variant allow-list
  - [ ] Sub-task 1.3: Refresh hook in `stitchd-experimentation-service` on `apply_transition`
- [ ] Task 2: `experiment_assignments_mv` materialized view
  - [ ] Sub-task 2.1: TDD against fixture eval-log rows — assert first-exposure rows land in `experiment_assignments` with correct `(experiment_id, iteration_id, context_type, context_key, variant_key, assigned_at, matched_rule_id)`
  - [ ] Sub-task 2.2: Migration `0007_experiment_assignments_mv.sql` — `CREATE MATERIALIZED VIEW ... TO experiment_assignments AS SELECT ... FROM flag_evaluation_log_v2 JOIN dictGet('experiment_iterations_active', ...) WHERE matched_rule_id matches + context_type IN unit_context_types`
  - [ ] Sub-task 2.3: TDD re-exposures with different variants do NOT reassign (ReplacingMergeTree on `assigned_at` minimum)
- [ ] Task 3: Backfill migration
  - [ ] Sub-task 3.1: TDD backfill fixture — 90 days of eval log, varying rules, fixture experiments → backfill produces expected assignment rows
  - [ ] Sub-task 3.2: Migration `0008_backfill_experiment_assignments.sql` — bounded `INSERT INTO experiment_assignments SELECT ...` from existing eval log
- [ ] Task 4: Rule-scoping + context-type-scoping correctness tests
  - [ ] Sub-task 4.1: TDD evals matching a different rule on the same flag do NOT create assignments
  - [ ] Sub-task 4.2: TDD evals for `context_type ∉ unit_context_types` do NOT create assignments
  - [ ] Sub-task 4.3: TDD default-rule-bound experiment: evals with `matched_rule_id IS NULL` create assignments
- [ ] Task: Conductor — User Manual Verification 'Attribution Pipeline MV + Backfill' (Protocol in workflow.md)

## Phase 5: Stats Service — Query Cutover
<!-- depends: phase4 -->
<!-- execution: parallel -->

- [ ] Task 1: Aggregation query — JOIN events ↔ assignments
  <!-- files: crates/stitchd-stats-service/src/queries/aggregation.rs, crates/stitchd-stats-service/tests/aggregation_query.rs -->
  - [ ] Sub-task 1.1: TDD `build_aggregation_query` no longer references `arrayExists(t -> t.1 = 'experiment'...)` etc.
  - [ ] Sub-task 1.2: Rewrite `crates/stitchd-stats-service/src/queries/aggregation.rs` — `JOIN experiment_assignments a USING (env_id, context_type, context_key)`, filter `e.occurred_at >= a.assigned_at AND < iteration_end`, GROUP BY `(a.context_type, a.variant_key)`
  - [ ] Sub-task 1.3: TDD pre-exposure events excluded
- [ ] Task 2: Ratio query cutover
  <!-- files: crates/stitchd-stats-service/src/queries/ratio.rs, crates/stitchd-stats-service/tests/ratio_query.rs -->
  - [ ] Sub-task 2.1: TDD ratio query joins both numerator and denominator metrics against the same assignment scope
  - [ ] Sub-task 2.2: Rewrite `queries/ratio.rs`
- [ ] Task 3: Funnel query cutover
  <!-- files: crates/stitchd-stats-service/src/queries/funnel.rs, crates/stitchd-stats-service/tests/funnel_query.rs -->
  - [ ] Sub-task 3.1: TDD funnel `windowFunnel` runs per context type, scoped to assigned contexts only, with `dedup_key = (context_type, context_key)`
  - [ ] Sub-task 3.2: Rewrite `queries/funnel.rs` — beware of bind-order gotcha (patterns.md: `clickhouse-rs` binds `?` by SQL position)
- [ ] Task 4: Preview query cutover
  <!-- files: crates/stitchd-stats-service/src/queries/preview.rs, crates/stitchd-stats-service/tests/preview_query.rs -->
  - [ ] Sub-task 4.1: TDD preview daily series scoped per context type and per variant
  - [ ] Sub-task 4.2: Rewrite `queries/preview.rs`
- [ ] Task 5: Per-context-type result builder
  <!-- files: crates/stitchd-stats-service/src/results_writer.rs, crates/stitchd-db/clickhouse-migrations/0009_experiment_results_context_type.sql -->
  <!-- depends: task1, task2, task3, task4 -->
  - [ ] Sub-task 5.1: TDD `compute_iteration_results` produces a map `context_type -> Vec<VariantResult>` instead of a flat list
  - [ ] Sub-task 5.2: Update `results_writer.rs` to persist per-context-type rows in `experiment_results` (add `context_type LowCardinality(String)` to the CH table — new migration `0009_experiment_results_context_type.sql`)
- [ ] Task: Conductor — User Manual Verification 'Stats Query Cutover' (Protocol in workflow.md)

## Phase 6: Stats Math — Frequentist + Bayesian + CUPED + SRM + Guardrails
<!-- depends: phase4 -->
<!-- execution: parallel -->

- [ ] Task 1: SRM chi-square test
  <!-- files: crates/stitchd-core/src/experimentation/stats/srm.rs -->
  - [ ] Sub-task 1.1: TDD chi-square test for variant allocation imbalance per context type; red pill when p < 0.001
  - [ ] Sub-task 1.2: Implement in `crates/stitchd-core/src/experimentation/stats/srm.rs`; surface via `SrmResult { observed, expected, deviation_pct, chi_sq_p, health }`
- [ ] Task 2: Frequentist analyzer audit + multi-comparison correction
  <!-- files: crates/stitchd-core/src/experimentation/stats/frequentist.rs -->
  - [ ] Sub-task 2.1: TDD Welch's t-test + two-prop z-test pass against canonical fixtures
  - [ ] Sub-task 2.2: Bonferroni correction applied when variants > 2
- [ ] Task 3: Bayesian analyzer
  <!-- files: crates/stitchd-core/src/experimentation/stats/bayesian.rs -->
  - [ ] Sub-task 3.1: TDD Beta-Binomial posterior + PtB for proportion metrics
  - [ ] Sub-task 3.2: TDD Normal-Normal posterior + PtB for continuous metrics
  - [ ] Sub-task 3.3: 95% credible interval, expected lift
- [ ] Task 4: CUPED variance reduction
  <!-- files: crates/stitchd-core/src/experimentation/stats/cuped.rs, crates/stitchd-stats-service/src/cuped_fetch.rs -->
  - [ ] Sub-task 4.1: TDD CUPED reduces CI width by ≥ 15% on a seeded fixture with strong pre-period covariate
  - [ ] Sub-task 4.2: Implement in `stitchd-core/src/experimentation/stats/cuped.rs` — `θ = cov(Y, X_pre) / var(X_pre)`, adjusted metric `Y' = Y - θ(X_pre - mean(X_pre))`
  - [ ] Sub-task 4.3: Stats service fetches pre-period metric per context from `events_v2` (filtered by `unit_context_types`)
- [ ] Task 5: Guardrails computation
  <!-- files: crates/stitchd-stats-service/src/grpc/results.rs -->
  <!-- depends: task2, task3 -->
  - [ ] Sub-task 5.1: TDD guardrail metrics computed identically to primaries; surfaced as separate `guardrails: VariantResultJson[]` in results
  - [ ] Sub-task 5.2: Direction violation flag computed against `metric_definitions.goal_direction`
- [ ] Task: Conductor — User Manual Verification 'Stats Math' (Protocol in workflow.md)

## Phase 7: Gateway API Surface
<!-- depends: phase5, phase6 -->

- [ ] Task 1: Extended `GET /results` response shape
  - [ ] Sub-task 1.1: TDD response includes `results_by_context_type`, `bound_target`, `pre_period_days`
  - [ ] Sub-task 1.2: Update `experiment_to_json` + `variant_result_to_json` to handle per-context-type breakdown
  - [ ] Sub-task 1.3: utoipa annotations updated; OpenAPI schema test passes
- [ ] Task 2: `GET /exposures` endpoint
  - [ ] Sub-task 2.1: TDD paginated list of assignments (context_type, context_key, variant, assigned_at, matched_rule_id); `context_type` query param required
  - [ ] Sub-task 2.2: Backend gRPC `ListExposures` in `stitchd-experimentation-service`
  - [ ] Sub-task 2.3: Gateway route + utoipa
- [ ] Task 3: `GET /timeseries` endpoint
  - [ ] Sub-task 3.1: TDD per-metric per-variant daily series, scoped per `context_type` query param (required)
  - [ ] Sub-task 3.2: Stats service exposes `GetTimeseries` RPC; gateway proxies
- [ ] Task 4: `POST /recompute` + `GET /recompute/{job_id}`
  - [ ] Sub-task 4.1: TDD recompute endpoint enqueues `TriggerRecompute` gRPC, returns job_id
  - [ ] Sub-task 4.2: Job status endpoint polls stats-service `GetJobStatus`
- [ ] Task 5: `POST /flags/{key}/default-rule-distribution`
  - [ ] Sub-task 5.1: TDD set/update; 409 when flag locked
  - [ ] Sub-task 5.2: Audit log entry written
- [ ] Task 6: OpenAPI contract-check + mdBook docs draft
  - [ ] Sub-task 6.1: Verify `scripts/check_openapi_contract.py` passes
- [ ] Task: Conductor — User Manual Verification 'Gateway API Surface' (Protocol in workflow.md)

## Phase 8: Admin UI — Foundation + API Wiring
<!-- depends: phase7 -->

- [ ] Task 1: API client helpers
  - [ ] Sub-task 1.1: Add typed wrappers in `admin/src/lib/api/experiments.ts` for `getResults`, `listExposures`, `getTimeseries`, `recompute`, `getRecomputeStatus`
  - [ ] Sub-task 1.2: Add `admin/src/lib/api/flags.ts` `setDefaultRuleDistribution`
  - [ ] Sub-task 1.3: Vitest unit tests with mocked axios
- [ ] Task 2: Remove `EXPERIMENTS` mock dependency from `ExperimentDetail.tsx`
  - [ ] Sub-task 2.1: TDD page renders against API stub (`vi.mock`)
  - [ ] Sub-task 2.2: Replace `EXPERIMENTS` import + `Experiment` mock type with API types
  - [ ] Sub-task 2.3: Loading + error states
- [ ] Task 3: Context-type switcher primitive
  - [ ] Sub-task 3.1: TDD `ContextTypeTabs` primitive — renders one tab per `unit_context_types`, persists active tab in localStorage scoped by experiment ID
  - [ ] Sub-task 3.2: Lift active context type into a context provider so all sub-tabs read from it
- [ ] Task: Conductor — User Manual Verification 'UI Foundation' (Protocol in workflow.md)

## Phase 9: Admin UI — Results, SRM, Timeseries, Iterations Tabs
<!-- depends: phase8 -->
<!-- execution: parallel -->

- [ ] Task 1: Results tab (real data)
  <!-- files: admin/src/pages/experiments/tabs/Results.tsx, admin/src/pages/experiments/tabs/Results.test.tsx -->
  - [ ] Sub-task 1.1: TDD Frequentist viz reads `results_by_context_type[active].variants` and renders mean ± CI, p-value, PtB
  - [ ] Sub-task 1.2: Bayesian viz toggle (localStorage-persisted)
  - [ ] Sub-task 1.3: Multi-variant matrix when variants > 2
  - [ ] Sub-task 1.4: Winner highlighting via `goal_direction`
- [ ] Task 2: Exposure/SRM panel
  <!-- files: admin/src/pages/experiments/tabs/Exposures.tsx, admin/src/pages/experiments/tabs/Exposures.test.tsx -->
  - [ ] Sub-task 2.1: TDD per-variant assigned/expected/deviation/chi-sq pill
  - [ ] Sub-task 2.2: Red pill when `chi_sq_p < 0.001`
  - [ ] Sub-task 2.3: "Bound to: <rule name> | Default rule" badge
- [ ] Task 3: Time-series tab
  <!-- files: admin/src/pages/experiments/tabs/Timeseries.tsx, admin/src/pages/experiments/tabs/Timeseries.test.tsx -->
  - [ ] Sub-task 3.1: TDD daily series sparklines + expanded chart on metric click
  - [ ] Sub-task 3.2: Scoped to active context type
- [ ] Task 4: Iteration history tab + manual recompute
  <!-- files: admin/src/pages/experiments/tabs/Iterations.tsx, admin/src/pages/experiments/tabs/Iterations.test.tsx -->
  - [ ] Sub-task 4.1: TDD past-iteration list with snapshot summary (incl. `unit_context_types`)
  - [ ] Sub-task 4.2: "Recompute now" button → `POST /recompute`, polls status, toast on completion
  - [ ] Sub-task 4.3: Click iteration row → past-iteration results (read-only)
- [ ] Task: Conductor — User Manual Verification 'UI Detail Tabs' (Protocol in workflow.md)

## Phase 10: Admin UI — Create/Edit Modal, Flag Editor, List
<!-- depends: phase8 -->
<!-- execution: parallel -->

- [ ] Task 1: Create/Edit experiment modal
  <!-- files: admin/src/pages/experiments/CreateExperimentModal.tsx, admin/src/pages/experiments/CreateExperimentModal.test.ts, admin/src/lib/validation/experiment.ts -->
  - [ ] Sub-task 1.1: TDD rule picker filters to percentage-distribution rules + "Default rule" entry when flag has `default_rule_distribution`
  - [ ] Sub-task 1.2: TDD non-eligible rules disabled with tooltip
  - [ ] Sub-task 1.3: `unit_context_types` multi-select (sourced from `context_type_registry`); default `[user]`
  - [ ] Sub-task 1.4: Primary metric_ids + guardrail metric_ids FieldArrays
  - [ ] Sub-task 1.5: Optional `pre_period_days` input (CUPED)
  - [ ] Sub-task 1.6: Yup schema in `admin/src/lib/validation/experiment.ts`
- [ ] Task 2: Flag editor — default-rule distribution section
  <!-- files: admin/src/pages/flags/EditFlagDefaultRule.tsx, admin/src/pages/flags/EditFlagDefaultRule.test.tsx -->
  - [ ] Sub-task 2.1: TDD UI lets user pick single-variant (default) OR percentage distribution
  - [ ] Sub-task 2.2: Disabled with lock badge when any experiment running on this flag
  - [ ] Sub-task 2.3: Validation: percentages sum to 100, each variant covered
- [ ] Task 3: ExperimentsList enhancements
  <!-- files: admin/src/pages/experiments/ExperimentsList.tsx, admin/src/pages/experiments/ExperimentsList.test.tsx -->
  - [ ] Sub-task 3.1: TDD exposed-context count badge (live from `/exposures?per_page=1` for total)
  - [ ] Sub-task 3.2: Status + flag filters
  - [ ] Sub-task 3.3: Days remaining
- [ ] Task: Conductor — User Manual Verification 'UI Create + Flag Editor + List' (Protocol in workflow.md)

## Phase 11: Documentation, E2E, Coverage, Cleanup
<!-- depends: phase9, phase10 -->

- [ ] Task 1: mdBook documentation
  - [ ] Sub-task 1.1: New `docs/src/experimentation/attribution.md` — explains eval-log-based first-exposure model, rule-scoping, context-type-scoping
  - [ ] Sub-task 1.2: New `docs/src/experimentation/default-rule-experiments.md`
  - [ ] Sub-task 1.3: Update `docs/src/experimentation/index.md` with full lifecycle + UI tour
  - [ ] Sub-task 1.4: Rebuild docs via `cargo run --manifest-path crates/xtask/Cargo.toml -- docs`
- [ ] Task 2: E2E test — full lifecycle
  - [ ] Sub-task 2.1: TDD `tests/e2e/experiment_lifecycle.rs` — create flag with default-rule distribution → create experiment bound to default rule → emit evals + events for two context types → transition to running → verify per-context-type assignments → verify per-context-type results → stop → verify flag unlocked
- [ ] Task 3: Coverage validation
  - [ ] Sub-task 3.1: Run `cargo tarpaulin -p <crate>` for each crate touched; verify ≥90%
  - [ ] Sub-task 3.2: Run admin `npm test` with coverage; verify ≥90% on new components
- [ ] Task 4: Pattern + tech-stack updates
  - [ ] Sub-task 4.1: Add new patterns to `conductor/patterns.md` (CH dictionary refresh, per-context-type result shape, ITT attribution model)
  - [ ] Sub-task 4.2: Update `conductor/tech-stack.md` ClickHouse Schema section with `experiment_assignments_mv` and `experiment_iterations_active` dictionary
  - [ ] Sub-task 4.3: Update `conductor/product.md` Implementation Status table
- [ ] Task 5: Final cleanup
  - [ ] Sub-task 5.1: Remove any `EXPERIMENTS` mockData references workspace-wide
  - [ ] Sub-task 5.2: Remove SDK-tagged event-context-attribution code paths in `stitchd-event-writer` and `stitchd-analytics-service` (if any remain after Phase 5)
  - [ ] Sub-task 5.3: Update `conductor/tracks.md` archive note
- [ ] Task: Conductor — User Manual Verification 'Docs + E2E + Cleanup' (Protocol in workflow.md)
