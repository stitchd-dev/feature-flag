# Track Learnings: bandit_20260608

Patterns, gotchas, and context discovered during implementation.

## Codebase Patterns (Inherited)

From `conductor/patterns.md` (read before starting):
- **In-process cache primitive:** `moka::future::Cache` with `time_to_live` + `get_or_try_insert_with` (coalesces concurrent loaders). (db_optim_20260516)
- **Pagination total without second query:** `COUNT(*) OVER()`. (db_optim_20260516)
- **ID newtypes:** `macro_rules!` + `sqlx::Type(transparent)`. (domain_20260411)
- **Rust 2024:** `resolver = "3"`; `std::env::set_var` is `unsafe`; nightly-only rustfmt opts no-op on stable.

## Track-Specific Context (from scoping)

- **Reuse, don't rebuild:** bandit is a *mode* on the existing experiment entity — reuses
  flag binding (custom rule OR default-rule), `metric_ids`, `unit_context_types`, exclusion
  groups, whole-flag lock, ITT attribution, and the Bayesian stats core.
- **Bayesian posteriors already exist** in
  `crates/stitchd-core/src/experimentation/stats/bayesian.rs` — `analyze_count`
  (Beta-Binomial), `analyze_numeric` (Normal-Normal), `analyze_percentile`, `analyze_funnel`,
  `analyze_ratio`. Thompson Sampling samples from THESE; the LCG RNG + Gamma/Beta sampling
  are already present (bayesian.rs ~88–155). Do NOT add a new RNG dependency.
- **Allocation representation:** `RolloutDistribution` / `RolloutAllocation { variant_key,
  percentage_bp }` in `crates/stitchd-core/src/rollout.rs`; basis points, sum = 10,000.
- **Sufficient-stats queries** to reuse: `crates/stitchd-stats-service/src/queries/variant_stats.rs`
  (`build_aggregation_cells_query`, `build_ratio_cells_query`, `build_funnel_cells_query`,
  `build_assignment_counts_query`).
- **Compute hook:** `run_stats_compute` in `crates/stitchd-stats-service/src/compute.rs`
  (~line 1451) — bandit reallocation hooks in AFTER the frequentist/bayesian/sequential pass.
- **History table pattern:** mirror `scheduled_change_runs`
  (`crates/stitchd-db/migrations/20260604000001_lifecycle_automation.sql` ~lines 67–76) for
  `bandit_allocation_runs`.
- **Eval-invariant gate pattern:** the real-time path mirrors the in-memory `ExclusionGate`
  (rides on the rule's snapshot, zero DB lookup) — see xexp_interaction_20260602.

## CRITICAL gotchas (from workflow.md / beads memory)

- **CI live-ClickHouse `--test` list:** stats-service self-seeding `#[ignore]`d live-CH tests
  run in a SEPARATE CI Coverage step (`.github/workflows/ci.yml`) that names each `--test`
  target by filename. `cargo llvm-cov` does NOT run them. **When you add `tests/bandit_*.rs`
  files, add them to that explicit `--test` list** or CI goes red on the next push, invisible
  to local `cargo test --workspace`. Current set: aggregation_query, ratio_query, funnel_query,
  preview_query, interaction_compute, compute_pass, cuped_compute, percentile_significance.
- **ClickHouse eval-log table** has `targeting_on Bool` NOT `is_disabled`; queries use
  `toUInt8(NOT targeting_on) AS is_disabled`.
- **sqlx prepare** with the SAME flags CI uses:
  `cargo sqlx prepare --workspace -- --all-targets --features stitchd-sdk-rust/test-util`
  (NOT `-- --tests`). Commit `.sqlx/` additions AND deletions.
- **Beads close in parallel waves:** use plain `bd close <id>` (`--force` if a phantom dep on
  a still-open sibling phase blocks it); `--no-auto` is unreliable. Orchestrator controls wave
  advancement.
- **Whole-flag lock:** human flag/rule mutations 409 while an experiment runs/paused. The
  bandit's reallocation write is the ONE sanctioned exception (system actor) — must bypass the
  lock while concurrent human mutations still 409.

## Architectural decisions (from spec confirmation)

- **Real-time eval path is an accepted invariant departure:** `evaluate_flag` becomes
  bandit-*aware* (samples per-context from the snapshot-resident model) — but stays pure,
  zero-DB, deterministic under a context-seeded RNG, and is GATED to realtime-mode rules only.
  `fixed` and static-bandit flags' eval path must be provably unchanged.
- **"Auto-creation"** = autonomous optimization *campaigns* (auto-spawn successive iterations
  on convergence/drift, bounded by max_iterations/budget) — NOT literal experiment creation
  from no trigger. The campaign opt-in IS the standing authorization.
- **Multi-objective** = scalarization (goal-normalized weighted sum) + constrained (primary +
  guardrail constraints). Full Pareto-front exploration is explicitly out of scope.

<!-- Learnings from implementation will be appended below -->

## Phase 1 — Schema, Domain & Proto Foundation (2026-06-08)

### Task 1.1 — PG migration (commit eb79e87)
- New migration `20260608000001_bandit_foundation.sql`: `experiment_mode TEXT DEFAULT
  'fixed'` (CHECK fixed|bandit) + `bandit_config JSONB` + `bandit_campaign_id UUID FK`
  on `experiments`; `bandit_config JSONB` snapshot on `experiment_iterations`;
  `bandit_campaigns` + `bandit_allocation_runs` tables (the latter mirrors
  `scheduled_change_runs`). All idempotent (IF NOT EXISTS / ADD COLUMN IF NOT EXISTS).
- **Shared dev DB drift gotcha:** the running `stitchd` Postgres DB has a *pre-existing*
  baseline checksum mismatch on `20260525000001_v1_baseline` AND several un-applied
  pending migrations (exclusion_group_unit_context_type, lifecycle_automation, …) — i.e.
  it was NOT migrated to the current branch state. `cargo sqlx migrate run` against it
  aborts on the checksum. Workaround used: create a fresh throwaway DB
  (`bandit_verify_20260608`) and migrate from scratch — the whole chain incl. this
  migration applies green. Use that fresh DB as `DATABASE_URL` for `#[sqlx::test]` and
  `cargo sqlx prepare`. (DATABASE_URL is unset in the env; dev URL is
  `postgres://stitchd:stitchd@localhost:5432/<db>` from docker-compose defaults.)
- Baseline tables use `env_id` (not `environment_id`) on `experiments`/`iterations`, but
  `bandit_campaigns` follows the spec's `environment_id` naming (both conventions exist
  in v1_baseline).

### Task 1.2 — core domain types (commit d9378cf)
- Bandit submodule under `crates/stitchd-core/src/experimentation/bandit/`. Tagged enums
  use `#[serde(tag = "type", rename_all = "snake_case")]` (matches `MetricKind`/segment
  conventions); `ExperimentMode` is `rename_all = "lowercase"`. `Uuid` fields carry
  `#[cfg_attr(feature="openapi", schema(value_type = String, format = Uuid))]`.
- New `Experiment`/`ExperimentIteration` fields use `#[serde(default)]` so legacy JSON/rows
  decode (added a back-compat deserialization test).
- **DB repo is hand-rolled `sqlx::query` + `row.get(...)` mapping (NOT `query!` macros NOR
  FromRow).** Adding a domain field therefore requires touching EVERY SELECT/INSERT/UPDATE
  + the `row_to_experiment`/`row_to_iteration` mappers in
  `repository/pg/experiment.rs`. The status-transition UPDATE…RETURNING and the
  iteration-INSERT both feed the row mappers, so their column lists/binds must include the
  new columns too. Did this in Task 1.2's commit (it's the schema↔domain seam the migration
  + types jointly require).

### Task 1.3 — proto (commit 92f5421)
- Additive only: Experiment fields 25/26, Iteration 17, analytics
  WriteExperimentResultsRequest 14 + ExperimentResult row 15. `bandit_config` rides as a
  JSON *string* (mirrors how no JSON-typed proto exists here; verified sequential_result
  carries the same way). Proto regenerates via a plain `cargo build` (protoc-bin-vendored).
- **A new domain field ripples to ~12 struct-literal sites across the workspace** (services
  + every test fixture/builder that spells out `Experiment {…}`/`ExperimentIteration {…}`).
  Fastest path: `cargo build --workspace --all-targets`, fix the reported `missing field`
  sites one wave at a time. Watch trailing closers: `};` vs `}` vs `}])` need separate
  edits / replace_all groupings.
- **bandit_allocation is accepted at the analytics write boundary but defaulted to None at
  the CH conversion** — the ClickHouse `experiment_results` row has no column yet (single
  baseline CH migration; no incremental ALTER mechanism wired). Full CH persistence is FR7
  surfacing (Phase 11), deliberately deferred.

### Task 1.4 — gateway REST (commit b692fc5)
- Gateway is a pure proto-translation layer; binding validation lives in the service. Added
  `resolve_bandit_fields()` helper for the bandit input rules (mode ∈ {fixed,bandit};
  config only with mode=bandit; config must `validate()`), returning 422 via
  `GatewayError::InvalidBody` (there is NO `Validation` variant — `InvalidBody`→422,
  `BadRequest`→400).
- **Don't insert a fn between a `#[utoipa::path(...)]` attribute and its handler** — the
  macro generates `__path_<fn>` and breaks if the attribute no longer sits directly on the
  handler. Put helpers above the doc-comment/attribute block.
- running/paused immutability for mode/config is enforced by the existing service-side
  update guard (rejects updates while running|paused), not re-implemented at the gateway.

### Gates
- `.sqlx` cache delta = ZERO: the experiment repo uses runtime `sqlx::query`, not `query!`,
  so `cargo sqlx prepare` produced no new entries. Nothing to commit there.
- fmt/clippy `-D warnings`/core+db+gateway tests all green (with DATABASE_URL pointed at the
  fresh verify DB for the `#[sqlx::test]` suites).

## Phase 3 — Privileged Allocation-Write Path (2026-06-08)

### Task 3.1 — flag-service BanditUpdateAllocation RPC (commit e7f8e96)
- **Lock-bypass is owner-scoped, NOT a blanket bypass.** The handler does NOT call
  `ensure_flag_unlocked` (that would reject the bandit's own write). Instead it loads the
  current lock owner via `is_flag_locked` (the same cache/`find_active_experiment_for_flag`
  helper human mutations use) and requires `owner == request.experiment_id`. Unlocked flag OR
  different owner → `FAILED_PRECONDITION`. So an arbitrary caller can't use this RPC to dodge
  the human lock, and concurrent human `MutateFlag`/`SetDefaultRuleDistribution` still 409
  (they keep calling `ensure_flag_unlocked`, unchanged).
- **Two write targets, two repo paths.** Default-rule: set `record.default_rule_distribution`
  + `flag_repo.update()` (bumps version + system-actor audit internally). Custom rule:
  `find_rules` → mutate the matched rule's `RuleOutput::Percentage { weights }` (Vec<(VariantId,
  u32 bp)>) → `upsert_rules`, THEN `flag_repo.update(&record)` because `upsert_rules` does NOT
  bump the flag version (SDKs converge on the flag version bump).
- **Allocation payload reuses `AllocationBucket`** (variant_key + weight_bp basis points) from
  flag_sync.proto — already imported. Validated via `RolloutDistribution::validate` (sum=10000,
  each >0) + variant_key referential-integrity against `variant_repo.find_by_flag`.
- **Audit attribution = scheduler pattern (actor_id=None).** Added an optional
  `Arc<dyn AuditLogger>` to `FlagServiceImpl` (`with_audit_logger`, wired in main.rs from the
  same `audit_raw`) to log a dedicated `bandit_reallocate` action (best-effort: a log failure
  does NOT roll back the committed write). The repo `update`/`upsert_rules` already log a
  system-actor row too.
- **Proto FeatureFlag has NO top-level `default_rule_distribution` field** — the mapping folds
  it into the rules list as a trailing `And:[]` catch-all rule. So tests assert the written
  distribution via the repo record, not the response proto.
- **`StubFlagRepo.update` does NOT bump version** (real PG repo does, internally) — version-bump
  assertions only hold against PG; stub tests assert the write applied + optimistic-conflict on
  a wrong `req.version`.

### Task 3.2 — experimentation-service ApplyBanditAllocation dispatch (commit 364e445)
- **New RPC on experimentation-service** (stats-service already reaches it over gRPC via
  `ExperimentationServiceClient`, like `ListRunningExperiments`/`TransitionExperiment`). Added
  `BanditAllocationBucket` + Apply request/response + `BanditAllocationOutcome` enum
  (applied|skipped|failed) to `proto/experiments/v1/experimentation_service.proto` (additive).
- **Pure `resolve_bandit_dispatch(&Experiment, alloc_count) -> BanditDispatchPlan` helper** isolates
  eligibility + bound-target resolution from the gRPC/flag-client I/O so it's exhaustively
  unit-testable without a live flag-service. Eligibility: bandit mode, status ∈ {Running,Paused},
  non-empty allocations, `flag_key` present, XOR(`flag_rule_id`, `targets_default_rule`).
  Anything else → `Skip(reason)` (recoverable no-op, NOT an error — tick keeps advancing).
- **`FlagClient` is a concrete struct (`Option<FlagClient>` on the service), not a trait** — the
  applied/failed flag-service call can't be mocked without a live service, so the dispatch
  *decision* logic is tested via the pure helper + the RPC's SKIPPED branches (ineligible exp,
  no flag-client wired). Outcome mapping is thin: `tonic::Code::Aborted` → `version_conflict=true`.
- **flag-service `BanditUpdateAllocation` now resolves a flag by `environment_id` when
  `project_id` is empty** (mirrors `get_flag`'s SDK path) — the dispatch wrapper has
  `environment_id` + `flag_key` from the `Experiment`, not the project_id, and the proto
  FeatureFlag carries no project_id.
- **Gate note:** DATABASE_URL is unset by default; the 4 `#[sqlx::test]` start-prerequisite tests
  fail without it. Point DATABASE_URL at the fresh `bandit_verify_20260608` DB (already created in
  Phase 1, reachable via the `stitchd-postgres` container) and all 130 exp-service + 112 flag-service
  lib tests pass. No `.sqlx` cache delta (no `query!` macros added). fmt + clippy `-D warnings` clean.
