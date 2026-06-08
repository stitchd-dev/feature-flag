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
