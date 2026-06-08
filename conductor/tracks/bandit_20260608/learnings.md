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

## Phase 2 — Bandit Core Algorithms (w2_core, stitchd-core, pure math)

- **RNG reuse, no new dep:** the bandit allocators sample posteriors via the
  *existing* `bayesian.rs` machinery. Promoted `Lcg`, the `Rng` trait,
  `sample_beta`, `sample_gamma` from private → `pub(crate)`, and added one new
  `pub(crate) fn sample_standard_normal` (Box-Muller, same cosine branch already
  inside `sample_gamma`) so Numeric/Ratio posteriors sample from the same seeded
  stream. `RatioGroupStats::ratio_var()` is already `pub(crate)`; bandit is in the
  same crate so it consumes it directly. No `rand`/extra dependency added.
- **`VariantStats` shape:** the real struct is `{ sample_size, conversions, mean,
  variance, conversion_rate, percentiles }` (NOT the `n/successes/value_sum`
  shape in the task brief). `RewardPosterior` wraps `VariantStats` for
  count/funnel/numeric and `RatioGroupStats` (from `stats::sequential`) for ratio.
- **`GoalDirection { Increase, Decrease }`** lives in `thompson.rs` (first
  allocator) and is re-exported + reused by epsilon/ucb/reward — single
  interpretation of goal direction everywhere. Decrease = argmin (Thompson) /
  negated mean (epsilon/ucb index).
- **Separation of concerns:** allocators return *raw* `Vec<(String, f64)>`
  weights; `allocation::normalize_to_distribution` is the ONLY place that turns
  raw weights → basis points. Largest-remainder (Hamilton) apportionment:
  reserve `floor·n`, distribute `10000−floor·n` proportionally, hand out leftover
  bp by largest fractional remainder (ties → input order) → sum is EXACTLY 10000,
  every arm ≥ floor. Rejects `floor·n > 10000`. A zeroed-weight arm still gets the
  floor; with floor=0 an arm can legitimately get 0 bp (so the normaliser does NOT
  call `RolloutDistribution::validate`, which forbids 0 — that's only valid when
  floor>0).
- **UCB rule = winner-take-all** (documented): raw weight 1.0 to the max-index
  arm, 0.0 to the rest; `n==0 → +inf` index (forced exploration). The exploration
  floor downstream means winner-take-all never starves the other arms.
- **Constrained multi-objective gotcha:** post-hoc `apply_exploitable_mask`
  (zeroing non-exploitable arms' weights) STARVES winner-take-all allocators if
  the sole winner is the excluded arm (all weights become 0 → normaliser splits
  evenly). Added `allocate_exploitable(arms, mask, allocator_closure)` which runs
  the allocator over ONLY eligible arms then merges excluded arms back at 0.0 —
  the robust path for every allocator. Kept `apply_exploitable_mask` for the
  proportional (Thompson) case.
- **Scalarized reward has no joint posterior:** `reward_arms` turns the
  goal-normalised z-score weighted-sum into a point `Numeric` posterior
  (variance 0), so Thompson degenerates to deterministic argmax — intended for a
  scalarized point reward. Scalar/Constrained preserve the real posterior so
  Thompson keeps full uncertainty sampling.
- Phase 2 SHAs: Thompson 4c19ecc, epsilon+UCB be6b2ac, normalization 40292d9,
  reward combiner 40f1733. 61 new unit tests; full stitchd-core lib suite 843
  passed, clippy `-D warnings` clean, fmt clean. No async/I/O/sqlx in the bandit
  module (mirrors the `evaluation` purity contract).
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

## Phase 4 — Stats-Service Static Reallocation Pass (2026-06-08)

### Task 4.1 — bandit.rs reallocation pass (commit 5c37034)
- **ListRunningExperiments did NOT carry experiment_mode/bandit_config** — had to add
  them (proto `RunningExperiment` fields 15/16 as `string experiment_mode` +
  `optional string bandit_config` JSON, mirroring how Experiment/Iteration carry
  bandit_config as a JSON string). Server build: `experiment_mode` from `exp`,
  `bandit_config` from the snapshot on `iter` (`iter.bandit_config`, serde-serialised).
  stats-service `RunningExperiment` struct + `fetch_running_experiments` decode them;
  a malformed `bandit_config` JSON is logged + treated as `None` (→ ineligible, no-op).
- **Reuse the compute.rs VariantStats builders** (`count_variant_stats`,
  `numeric_variant_stats`, `funnel_variant_stats`) + the `CellReader` trait directly —
  do NOT re-query raw. `MetricType` is re-exported from
  `stitchd_core::experimentation::stats` (NOT `crate::compute`). `AggCell` is `pub`.
- **Allocation is a single GLOBAL rule write, but stats are per-context_type** — sum
  sufficient stats (assignment counts + AggCell/RatioGroupStats) across the experiment's
  `unit_context_types` into one posterior set per metric, zero-filling missing variants
  in `variant_keys` order so every arm is represented.
- **Pure decision core split out** (`decide_allocation(config, &[MetricRewards], seed)
  -> AllocationDecision`) so eligibility / seed-determinism / algorithm dispatch /
  skip-on-insufficient-data (min arm n < 30) are exhaustively unit-tested without any
  I/O; PG/gRPC behind thin `AllocationApplier` / `RunRecorder` async traits with fakes.
  Thompson uses the post-hoc `apply_exploitable_mask`; epsilon/UCB use
  `allocate_exploitable` (the robust constrained merge from Phase 2). Contextual algo →
  skip (real-time path, not static).
- **Deterministic seed = FNV-1a over (experiment_id, iteration_id, n_arms,
  tick.timestamp())** — reproducible per tick, varies tick-to-tick, never wall-clock RNG.
- **One row per experiment per tick:** the orchestrator records exactly one
  `bandit_allocation_runs` row (action reallocate|skip, outcome applied|skipped|failed).
  A transport-level RPC error is mapped to a `failed` row (recoverable, tick advances) —
  never propagated as `Err` (only a real CH read / PG insert error propagates).
- **PgRunRecorder uses runtime `sqlx::query`** (no `query!` macro) → zero `.sqlx` delta,
  consistent with the rest of stats-service.

### Task 4.2 — wire tick + TriggerRecompute + live test + CI (commit 20c0146)
- **Tick wiring:** the scheduler per-experiment task already builds the
  `ClickHouseCellReader` + resolves `metrics`; the reallocation call reuses both after
  `write_results`, with a `GrpcAllocationApplier` (shared exp client) + `PgRunRecorder`
  (pool). No-op + silent for non-bandit; logged on the bandit branch.
- **TriggerRecompute was a STUB** (`run_recompute` only managed the job row, never ran
  the stats compute). Added an `ExperimentRecomputer` trait + `with_recomputer` on
  `StatsServiceImpl`; production `PerExperimentRecomputer` fetches running experiments,
  finds the target, resolves metrics, runs the reallocation. Decoupled via the trait so
  the trigger stays unit-testable (added an ok/err recomputer test).
- **Live test needs BOTH CH and PG:** the `bandit_allocation_runs` insert has FK to
  `experiments` (NOT NULL) + `experiment_iterations`. Self-seed the full PG FK chain
  organisation→project→environment→feature_flag→experiment→iteration via raw `sqlx::query`.
  GOTCHA: `experiments` has a `experiments_rule_xor_default` CHECK — must set
  `targets_default_rule = true` (or a `flag_rule_id`), can't leave both null.
  `feature_flags` needs `value_type` ('boolean'). Test cleans up its PG rows at the end
  (CH is append-only / deduped per run).
- **CI:** added `--test bandit_reallocation` to the live-CH `--test` list in the coverage
  job. CRITICAL: that test (unlike the CH-only ones) seeds PG against the base
  DATABASE_URL DB, which the coverage job did NOT migrate — added `Install sqlx-cli` +
  `sqlx migrate run` steps before the live-CH step. Observed: treatment 9500 / control
  500 bp (floor) for a 10%→30% effect.
- **Pre-existing Phase-3 gap fixed (commit 701e6ac):** the Phase-3 ApplyBanditAllocation
  / BanditUpdateAllocation RPCs were never added to the gateway integration-test service
  mocks, so `cargo build --workspace --all-targets` was already broken (E0046) at the
  Phase-4 start commit. Added unimplemented stubs across 6 gateway test files. (Fix gaps
  as discovered — did not baseline around it.)

## Phase 5 — Real-Time Eval Path (snapshot-resident, bandit-aware) (2026-06-08)

### Task 5.1 — RealtimeBanditModel rides the snapshot (commit c932949)
- **Mirrors ExclusionGate 1:1.** New `optional RealtimeBanditModel realtime_bandit = 5`
  on proto `PercentageAllocation` (+ `RealtimeBanditModel`/`VariantPosterior` messages +
  `RewardFamily`/`BanditGoalDirection` enums in `flag_sync.proto`); parallel domain types
  in `rule_engine/types.rs` (`RealtimeBanditModel`, `VariantPosterior`, `RewardFamily`,
  `BanditGoal`) + `realtime_bandit: Option<RealtimeBanditModel>` on `RuleOutput::Percentage`
  with `#[serde(default, skip_serializing_if=...)]` so legacy rules decode + omit it.
- **`VariantPosterior` carries BOTH families' params** (`alpha`/`beta` for Beta,
  `mu`/`sigma2` for Normal) in one shape — general enough for Phase 6 contextual to extend
  without a wire change.
- **A new `RuleOutput::Percentage` field ripples to ~18 struct-literal + pattern sites**
  across core/proto/flag-service/gateway/db-tests/SDK. Fastest path: `perl -0pi` to insert
  `realtime_bandit: None,` after every `exclusion_gate: None,` line, then hand-fix the
  ~3 `exclusion_gate: Some(...)` constructors + the legacy `FlagEvaluator::evaluate`
  destructure (`realtime_bandit: _`). Mapping wired in `mapping.rs`
  (`proto_realtime_bandit_to_domain`/`domain_realtime_bandit_to_proto`) — unspecified
  family→Beta, unspecified goal→Increase.

### Task 5.2 — pure in-memory bandit sampling in evaluate_flag (commit 35ce501)
- **Purity kept by delegating ALL sampling to the bandit module.** Added
  `experimentation/bandit/realtime.rs` (`context_seed` = murmur3_x64_128 of `salt+unit_key`
  folded u128→u64 by XOR of halves; `sample_realtime_variant` = one seeded Beta/Normal draw
  per arm via the shared `Lcg`, goal-directed argmax). engine.rs only calls these pure fns +
  resolves the chosen variant — NO sqlx/tokio/reqwest/warn/error tokens, so the two
  `purity.rs` greps stay green (verified after the change).
- **Branch is gated + insertion-only.** The realtime branch sits at the TOP of the
  `RuleOutput::Percentage` arm in the unified `evaluate_flag`: `if let Some(model) = realtime_bandit
  && let Some(assignment) = sample_realtime_bandit(...) { ...; continue; }`. A rule with
  `realtime_bandit: None` never enters it → static bucket→% path is byte-identical (proven by
  `rule_without_realtime_bandit_uses_static_path_unchanged`, which recomputes the murmur bucket
  independently and asserts equality over 500 contexts). A missing diversion unit returns
  `None` → graceful static fallback.
- **Preview==SDK parity is automatic** because BOTH go through `evaluate_flag`; tested by
  evaluating the same context at `TraceLevel::Full` (preview) vs `TraceLevel::Off` (SDK) →
  same variant over 300 contexts. The SDK's own proto→domain mapper (`sdks/rust/src/client.rs`)
  also wires `realtime_bandit` so the model flows through the snapshot the SDK already fetches.
- **Trace privacy:** the bandit `RolloutDebug.hash_input` names only "real-time bandit
  sampling: <variant>" + surfaces sampled draws as variant_ranges — NEVER posterior params or
  context values (asserted: no `900`/`alice` in the note).

### Task 5.3 — stats-tick refreshes the snapshot model (commit 8c5782c)
- **Chose to EXTEND the existing RPCs (smaller change) over a new sibling RPC.** Added
  `optional RealtimeBanditModel realtime_model` to `ApplyBanditAllocationRequest` (field 4,
  exp-service) + `BanditUpdateAllocationRequest` (field 9, flag-service). exp-service imports
  `flags/v1/flag_sync.proto` for the cross-package type. When present, flag-service's
  `bandit_update_allocation` writes it onto the bound custom rule's `realtime_bandit` field
  (default-rule target + a model → InvalidArgument, since a distribution has no rule output).
  The static `allocations` still ride along as the fallback split.
- **Realtime path is a separate, mutually-exclusive branch in `run_bandit_reallocation`:**
  `is_eligible_realtime` (mode=Bandit + propagation=Realtime) → `run_realtime_refresh`, which
  builds the proto model via the pure `decide_realtime_model` (single scalar objective only for
  now; Beta from count/funnel mean+n, Normal from numeric/ratio mean), dispatches via the new
  `AllocationApplier::apply_realtime`, and records one `reallocate` row. Static path (Phase 4)
  is untouched — regression test asserts a static bandit calls `apply` not `apply_realtime`.
- **`AllocationApplier` trait grew `apply_realtime`;** `GrpcAllocationApplier` shares an inner
  `dispatch(.., model: Option<..>)` for both paths. Two test fakes (lib `FakeApplier`, live-CH
  `CapturingApplier`) updated to count realtime calls + capture the model.
- **Salt = experiment_id, unit_context_type = first configured unit** for the snapshot model;
  even-split fallback distribution (largest-remainder, sum=10000) rides along.
- **No `.sqlx` delta** (PgRunRecorder uses runtime `sqlx::query`, no macros). Satisfied the
  "extend live-CH test OR unit-test the dispatch" with thorough unit tests for
  `decide_realtime_model` + the realtime/static dispatch branches.

### Gates
- purity tests green after every engine change; clippy `-D warnings` clean on all 4 crates;
  core 860 / flag-service 116+ / stats-service 356 / exp-service 130 lib tests pass; full
  `cargo build --workspace --all-targets` clean; `cargo build -p stitchd-sdk-rust` confirms the
  snapshot field flows to the SDK.

## Phase 6 — Contextual Bandit

### Task 6.1 — pure contextual reward model + engine wiring (commit 278cab9)
- **New pure module `experimentation/bandit/contextual.rs`.** Model shape: `FeatureSpec
  { context_type, parameter, encoding }` with `FeatureEncoding::{Numeric, OneHot{categories}}`;
  `VariantCoefficients { variant_key, coeffs: Vec<f64>, a_inv: Option<Vec<f64>> }`;
  `ContextualModel { features, variants }`. Design vector is intercept-first: `[1.0, feat0…,
  feat1…]`. `predict` = dot product. `sample_contextual_variant` = Thompson-on-linear: per
  variant `mean + sd·Z` (Z from the shared `Lcg`; `sd = sqrt(xᵀ A⁻¹ x)` when `a_inv` present,
  else a 0.1 floor) then goal-directed argmax. `fit_ridge` / `fit_design_inverse` solve the
  normal equations `(XᵀX+λI)` via hand-rolled Gauss-Jordan with partial pivoting — **no new
  crate dep** (workspace had no linalg crate; small fixed dims).
- **A model is EITHER non-contextual posteriors OR contextual coeffs.** Added
  `contextual: Option<ContextualModel>` to `RealtimeBanditModel` (domain + proto + flag-service
  mapping + SDK mapping). Adding the field tripped clippy `large_enum_variant` on
  `RuleOutput::Percentage` → **boxed it: `realtime_bandit: Option<Box<RealtimeBanditModel>>`**
  (touches every construction/match site; `model.as_ref()` at the engine call, `.map(Box::new)`
  at mappers). serde `Box<T>` is transparent so legacy JSON still decodes.
- **Engine purity preserved by pre-resolving feature values into an OWNED map.** First attempt
  used a `RefCell`-cached `&str`-returning resolver — unsound (borrow ends when the guard
  drops). Final `BundleFeatureResolver` pre-resolves every `(context_type, parameter)` the
  model names into `HashMap<(String,String),String>` once, then hands back `&str` borrows — no
  `unsafe`. Feature VALUES stay in the resolver + the numeric vector; the trace surfaces feature
  NAMES only (`user.score`), asserted no value/unit-key leaks. Missing unit → static fallback;
  missing feature → encodes 0.0 (graceful, deterministic). `proto FeatureEncoding` is a oneof
  (`Numeric`/`OneHot`), so map via `feature_encoding::Kind`.

### Task 6.2 — contextual fit in stats-service tick + live-CH test (commit f0d13e6)
- **Feature values for the FIT come from `events.properties` (a `Map(String,String)`), NOT the
  assignment row** (assignments carry only context_type/context_key — no params). New
  `build_contextual_reward_rows_query` returns ONE row per assigned unit:
  `(variant_key, feature_value = events.properties[feature], reward = post-exposure value sum)`,
  ITT (non-firing unit → reward 0, feature ''). Escape the feature key into the `properties['…']`
  map-key literal via `clickhouse_escape` (same as `on_field`).
- **`decide_contextual_model` is pure** (encode_features + fit_ridge + fit_design_inverse per
  variant, sorted for determinism, MIN_ARM_SAMPLE floor on the smallest arm). `decide_contextual
  _refresh` (async) reads the rows for a single scalar Aggregation objective and calls it.
  `run_realtime_refresh` branches: `BanditAlgorithm::Contextual` → contextual decision, else the
  Phase-5 non-contextual `decide_realtime_model`. The apply/record path is shared.
- **DEVIATION (scoped):** today the FIRST declared `ContextualConfig.features` entry drives the
  single CH feature column + a Numeric encoding. The model shape + engine already support
  multiple features and one-hot; multi-feature/one-hot FIT-side is a future extension (the CH
  query returns one feature column). `ContextualConfig.features` is `Vec<String>` (names only),
  so encodings are inferred Numeric at fit time.
- **`BanditGoal` ambiguity in stats-service:** the crate aliases `GoalDirection as BanditGoal`
  (bandit core), but `sample_contextual_variant` wants `rule_engine::types::BanditGoal`. In tests
  qualify the rule_engine path explicitly.
- **Live self-seeding test `tests/bandit_contextual.rs` (#[ignore], mirrors bandit_reallocation)**
  seeds reward = f(score), runs the contextual fit through `run_bandit_reallocation`, maps the
  captured proto contextual model back to domain, and asserts high_lover wins high score /
  low_lover wins low score. Added `bandit_contextual` to the CI `--ignored --test` list.
- **No `.sqlx` delta** (runtime `sqlx::query`/`query_as`, no compile-time macros).

### Gates
- purity green; clippy `-D warnings` clean on core/flag-service/stats-service (all-targets);
  core 886 / flag-service 117 lib + 8 prereq-integration (with DATABASE_URL) / stats-service
  full suite pass; `bandit_contextual --ignored` green against live CH+PG; `cargo build
  -p stitchd-sdk-rust` confirms the contextual model flows to the SDK. No new crate deps.

## Phase 7 — Autonomous Lifecycle (2026-06-08)

### Task 7.1 — convergence detector (commit c25943b)
- New pure module `stitchd-core/src/experimentation/bandit/convergence.rs`.
  `probability_best` is a thin intent-named wrapper over `thompson_weights` (the
  Thompson "raw weight" already IS P(arm is goal-directed best)) — convergence and
  allocation share one definition. `detect_convergence(arms, goal, threshold, seed)
  -> Option<ConvergedWinner{variant_key, prob}>` returns Some when the single
  top-probability arm's prob-best >= threshold.
- **Tie handling:** a genuine tie at the top (another arm shares the exact max
  prob) returns None — no single arm dominates. Single/empty arm sets → None (a
  one-armed bandit has nothing to converge against). `DEFAULT_CONVERGENCE_SAMPLES
  = 4000` MC draws. 10 tests: golden-vector, clear-winner-converges,
  tie-does-not-converge, decrease-goal, determinism.

### Task 7.2 — lifecycle executor (commit fa074b3)
- New `stats-service/src/lifecycle.rs`, run AFTER reallocation each tick in
  main.rs (the reallocation's `BanditRunOutcome::Applied { allocation }` is captured
  and passed in as `current_allocation` for the idempotency check).
- **WHERE convergence/lifecycle state is persisted (for Phase 11):** NEW migration
  `20260608000002_bandit_lifecycle.sql` adds nullable `bandit_converged_variant
  TEXT` + `bandit_converged_prob DOUBLE PRECISION` on the `experiments` table.
  Advisory, auto_commit AND auto_rollout all stamp it (via
  `LifecycleTransitioner::record_convergence`, a runtime `UPDATE experiments SET ...`)
  so the badge is readable regardless of policy. Apply to the verify DB with
  `cargo sqlx migrate run --source crates/stitchd-db/migrations` before testing.
- **Commit = single-bucket 100%-to-winner.** `RolloutDistribution::validate`
  rejects 0bp, so commit writes `[{winner, 10000}]` (other arms OMITTED, i.e. 0%)
  — a single full allocation is valid. Reuses the Phase-3 privileged
  `AllocationApplier::apply` (ApplyBanditAllocation), NOT a new RPC.
- **Auto_rollout = commit -> stop, lock released IMPLICITLY.** There is NO explicit
  "promote winner / release lock" RPC. Stopping via `TransitionExperiment`
  (proto `EXPERIMENT_STATUS_CONCLUDED` = core `Stopped`; the request has a
  `reason` field — easy to miss → E0063) makes the experiment no longer the
  flag's active experiment, so the flag-service's lock (derived from
  `find_active_experiment_for_flag`) releases on its own, and the bound rule is
  left at the committed 100%-winner. So "promote winner into standing rule +
  release lock" = "commit on the bound rule, then stop". Documented as the
  simplest model satisfying FR5.
- **Idempotency:** pure `decide_lifecycle(policy, convergence, already_committed)`
  → NoAction/RecordAdvisory/Commit/Rollout/StopOnly. `already_committed =
  is_committed_to(current_allocation, winner)`. Once committed, AutoCommit →
  NoAction (no re-commit row), AutoRollout → StopOnly (commit done, just stop).
  Ordered commit→stop is restart-safe: a crash after commit re-runs next tick as
  StopOnly. A stopped experiment is never listed again (scheduler lists only
  running) → the ultimate idempotency backstop.
- **Test-fakes refactor:** the inline `FakeApplier`/`FakeRecorder`/`FakeReader`/
  `count_metric` in `bandit.rs`'s `mod tests` weren't reusable cross-module
  (field-access API). Added a `#[cfg(test)] pub(crate) mod tests_support` in
  bandit.rs with a RICHER API (`FakeApplier::apply_calls()`/`last_allocation()`,
  `FakeRecorder::count()`/`last_action()`, `FakeReader::with_conversions(...)`,
  `running_bandit_with(...)`) that lifecycle.rs imports. Left the original inline
  fakes untouched (didn't rewrite the 12 existing bandit tests). Also added
  `build_metric_rewards_pub` (pub(crate) wrapper) so lifecycle can build the same
  objective posteriors for convergence.
- 13 lifecycle tests (pure decision incl. policy × already-committed matrix; +
  orchestration with fakes: advisory-no-traffic-change, commit-100%-no-stop,
  commit-idempotent, rollout-commit-then-stop, rollout-stop-only-idempotent,
  no-convergence-no-action, non-bandit-no-action). stats-service lib: 374 pass.
- **No `.sqlx` delta** (record_convergence + recorder use runtime `sqlx::query`).
