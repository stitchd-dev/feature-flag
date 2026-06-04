# Plan: Sequential Testing (Always-Valid Inference)

Track ID: seqtest_20260603
Workflow: TDD (failing tests first), ≥90% coverage per crate, per-phase verification checkpoint.
Execution: Parallel (worker-wave). Phase 1 ∥ Phase 2 start immediately; Phase 4 ∥ Phase 3; Phases 2 & 5 fan out at the task level.

Dependency annotation legend:
- `<!-- depends: -->` (empty) → phase can start immediately (parallel-eligible).
- `<!-- depends: phaseN -->` → phase waits on the named phase(s).
- A phase with NO `depends` annotation defaults to sequential (depends on the previous phase).
- `<!-- execution: parallel -->` → tasks in the phase have no intra-phase ordering except where a task carries `<!-- depends: taskN -->`.

---

## Phase 1: Pure Sequential-Stats Core
<!-- depends: -->

Foundation. Pure `stitchd-core` math, no I/O. Tasks share the new `sequential/` module + `SequentialResult` type, so they run sequentially within the phase.

- [x] Task 1.1: Module scaffold + `SequentialResult` / `SequentialConfig` types + mSPRT `always_valid_p` (normal-mixture Λ, running-minimum, seeded at 1.0, clamped to [0,1]).
  <!-- files: crates/stitchd-core/src/experimentation/stats/sequential/mod.rs, crates/stitchd-core/src/experimentation/stats/sequential/msprt.rs, crates/stitchd-core/src/experimentation/stats/mod.rs -->
  - TDD: write Monte-Carlo H₀ test (continuous peeking rejects ≤ α) + H₁ power test FIRST; confirm red; implement to green.

- [x] Task 1.2: `confidence_sequence` — closed-form mSPRT-dual anytime-valid CI from the same N(0, τ²) mixture.
  <!-- files: crates/stitchd-core/src/experimentation/stats/sequential/confidence_seq.rs -->
  <!-- depends: task1 -->
  - TDD: uniform-coverage simulation (≥1−α under continuous peeking) + CI-width-shrinks-with-n tests FIRST.

- [x] Task 1.3: Per-family adapters (conversion/count Bernoulli diff, continuous mean diff, ratio via delta-method, funnel final-step rate diff) → `(δ̂, se)`; multi-variant multiplicity correction.
  <!-- files: crates/stitchd-core/src/experimentation/stats/sequential/adapters.rs -->
  <!-- depends: task1, task2 -->
  - TDD: per-family correctness + multiplicity tests FIRST.

- [x] Task: Conductor - User Manual Verification 'Phase 1: Pure Sequential-Stats Core' (Protocol in workflow.md)

---

## Phase 2: Schema, Config & Proto Contract
<!-- execution: parallel -->
<!-- depends: -->

Migrations + proto + repository plumbing. Tasks 2.1 / 2.2 / 2.3 touch disjoint files → fully parallel. Runs concurrently with Phase 1.

- [x] Task 2.1: Postgres migration — sequential config columns on `experiments` + `experiment_iterations` (`sequential_testing_enabled`, `sequential_alpha`, `sequential_tau_squared`, `sequential_min_sample_size`) with CHECK constraints (α∈(0,1), τ²>0, min_sample≥0); `stitchd-db` repo read/write + snapshot-at-iteration-start; regenerate `.sqlx`.
  <!-- files: crates/stitchd-db/migrations/20260603000001_sequential_config.sql, crates/stitchd-db/src/repositories/experiment.rs, .sqlx/ -->
  - TDD: repo round-trip test (write config → read back; iteration snapshot) FIRST.

- [x] Task 2.2: ClickHouse migration — nullable sequential columns on `experiment_results` (`sequential_p_value`, `sequential_ci_lower`, `sequential_ci_upper`, `sequential_method`, `sequential_crossed`, `sequential_insufficient_data`); update the analytics-service Row struct + insert.
  <!-- files: crates/stitchd-event-writer/migrations/20260603000002_experiment_results_sequential.sql, crates/stitchd-analytics-service/src/results.rs -->
  - TDD: insert/read-back row test FIRST.

- [x] Task 2.3: Additive proto fields — `Experiment`/`ExperimentIteration` config; `WriteExperimentResultsRequest`; `VariantResult`/`ContextTypeResults` sequential result fields; regenerate stubs (no renumbering, all new tags).
  <!-- files: proto/experiments/v1/experimentation_service.proto, proto/analytics/v1/analytics.proto -->
  - TDD: proto-roundtrip / serde test FIRST where applicable.

- [x] Task: Conductor - User Manual Verification 'Phase 2: Schema, Config & Proto Contract' (Protocol in workflow.md)

---

## Phase 3: Compute Integration
<!-- depends: phase1, phase2 -->

Wire the Phase 1 core into `stitchd-stats-service`. Tasks share scheduler/results-writer files → sequential within the phase.

- [x] Task 3.1: Compute sequential result per (metric, variant, context-type) in the 60-min scheduler, reusing the cumulative ITT sufficient statistics already fetched; gate on `sequential_testing_enabled` + `sequential_min_sample_size`; compose with CUPED-adjusted estimates when `pre_period_days > 0`.
  <!-- files: crates/stitchd-stats-service/src/scheduler.rs, crates/stitchd-stats-service/src/dispatch.rs, crates/stitchd-stats-service/src/sequential_compute.rs -->
  - TDD: compute-path test (enabled vs off; min-sample gate → insufficient_data) FIRST.

- [x] Task 3.2: Running-minimum persistence — read the prior tick's `always_valid_p` from `experiment_results`, feed as `prev_p`, write back; extend `results_writer` to emit the new sequential fields.
  <!-- files: crates/stitchd-stats-service/src/results_writer.rs, crates/stitchd-stats-service/src/prior_results.rs -->
  - TDD: monotone-non-increasing-across-ticks test FIRST.

- [x] Task: Conductor - User Manual Verification 'Phase 3: Compute Integration' (Protocol in workflow.md)

---

## Phase 4: Results Read Path & Gateway
<!-- depends: phase2 -->

Read the new ClickHouse columns into the results proto and surface over REST. Independent of Phase 3's write path → runs in parallel with Phase 3.

- [x] Task 4.1: Read new ClickHouse columns into the `ExperimentResults` proto (`VariantResult`/`ContextTypeResults`) in the analytics/experimentation read path.
  <!-- files: crates/stitchd-experimentation-service/src/results.rs, crates/stitchd-analytics-service/src/results_read.rs -->
  - TDD: read-mapping test FIRST.

- [x] Task 4.2: Gateway REST passthrough — surface sequential fields in the results JSON + OpenAPI annotation; keep the contract check green.
  <!-- files: crates/stitchd-gateway/src/routes/experiments.rs -->
  <!-- depends: task1 -->
  - TDD: route/serialization test FIRST.

- [x] Task: Conductor - User Manual Verification 'Phase 4: Results Read Path & Gateway' (Protocol in workflow.md)

---

## Phase 5: Admin UI
<!-- execution: parallel -->
<!-- depends: phase2, phase4 -->

React form + Results tab. Different components → parallel. Shared API-type definitions file is an explicit seam: keep additions small and adjacent in both tasks.

- [x] Task 5.1: Create/Edit experiment form — "Sequential testing" section (opt-in toggle + advanced α / τ² / min-sample-before-first-look knobs), Formik + Yup, off by default.
  <!-- files: admin/src/pages/experiments/ExperimentForm.tsx, admin/src/lib/validation/experiment.ts, admin/src/lib/api/experiments.ts -->
  - TDD: vitest (toggle hides/shows knobs; validation bounds; payload shape) FIRST.

- [x] Task 5.2: Results tab — always-valid p-value + anytime-CI columns and a "safe to stop" decision badge/banner (fires when the boundary is crossed in the goal direction), per context type, alongside the Frequentist/Bayesian view toggle.
  <!-- files: admin/src/pages/experiments/tabs/Results.tsx, admin/src/lib/api/experiments.ts -->
  - TDD: vitest (columns render when on; badge only when crossed in goal direction; hidden when off) FIRST.

- [x] Task: Conductor - User Manual Verification 'Phase 5: Admin UI' (Protocol in workflow.md)

---

## Phase 6: Documentation & Final Verification
<!-- depends: phase3, phase4, phase5 -->

- [x] Task 6.1: Update `product.md` (move sequential testing Future → implemented; describe the model), `tech-stack.md` (sequential module + new columns/migration), mdBook statistics/experimentation pages + crate `//!` preambles; run `cargo xtask docs` and confirm zero diff (idempotent).
  <!-- files: conductor/product.md, conductor/tech-stack.md, docs/src/, crates/stitchd-core/src/experimentation/stats/sequential/mod.rs -->

- [x] Task 6.2: Final CI gate — `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`, `cargo sqlx prepare --workspace --check -- --all-targets --features stitchd-sdk-rust/test-util`, admin `vitest` + `tsc --noEmit` + `npm run lint`, `cargo xtask docs && git diff --exit-code`, OpenAPI contract check. Fix any drift inline.
  <!-- files: (verification only — no owned source files) -->

- [x] Task: Conductor - User Manual Verification 'Phase 6: Documentation & Final Verification' (Protocol in workflow.md)
