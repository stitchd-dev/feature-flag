# Plan: Multi-Armed Bandit (Adaptive & Autonomous Experiment Allocation)

Methodology: TDD (Red→Green→Refactor), ≥90% coverage, worker-wave parallelism enabled.
Foundation (Phase 1) is sequential-first; algorithm/transport/compute fan out in waves.

Phase-level wave summary:
- Wave A: Phase 1 (foundation)
- Wave B: Phase 2 (core math) ∥ Phase 3 (write path)
- Wave C: Phase 4 (static reallocation) → then Phase 5 (real-time eval) ∥ Phase 9
  (multi-objective) ∥ Phase 10 (interaction)
- Wave D: Phase 6 (contextual, after 5) ∥ Phase 7 (lifecycle, after 4) → Phase 8 (campaigns)
- Wave E: Phase 11 (REST surfacing) → Phase 12 (Admin UI) → Phase 13 (integration/docs/CI)

## Phase 1: Schema, Domain & Proto Foundation [checkpoint: d2e0d46]
<!-- execution: parallel -->
<!-- depends: -->

- [x] Task 1: PG migration [eb79e87] — `experiment_mode` + `bandit_config` JSONB on `experiments` +
      `experiment_iterations`; `bandit_allocation_runs` table (mirrors `scheduled_change_runs`:
      experiment_id, iteration_id, fired_at, old/new_allocation, action, outcome, detail);
      `bandit_campaigns` table (campaign config, max_iterations, drift thresholds). sqlx-prepare.
      <!-- files: crates/stitchd-db/migrations/*, .sqlx/* -->
- [x] Task 2: stitchd-core domain types [d9378cf] — `ExperimentMode` enum, `BanditConfig`,
      `BanditAlgorithm`, `PropagationMode`, `LifecyclePolicy`, `RewardObjective`
      (scalar | scalarized | constrained), `BanditCampaignConfig`; serde + invariants + tests.
      <!-- files: crates/stitchd-core/src/experimentation/mod.rs, crates/stitchd-core/src/experimentation/bandit/types.rs -->
- [x] Task 3: Proto additions [92f5421] — `experiment_mode` + `bandit_config` (+ campaign) on
      `Experiment`/`ExperimentIteration`; `bandit_allocation` + per-objective posteriors on
      `WriteExperimentResultsRequest`/`VariantResult`; regenerate. Backward-compatible (new fields).
      <!-- files: proto/experiments/v1/experimentation_service.proto, proto/analytics/v1/analytics.proto -->
      <!-- depends: task2 -->
- [x] Task 4: Gateway REST input wiring [b692fc5] — `experiment_mode`/`bandit_config`/campaign into
      CreateExperimentBody + UpdateExperimentBody; validation; immutability-while-running guard.
      <!-- files: crates/stitchd-gateway/src/routes/experiments.rs -->
      <!-- depends: task3 -->
- [x] Task: Conductor - User Manual Verification 'Phase 1' [autonomous] (Protocol in workflow.md)

## Phase 2: Bandit Core Algorithms (stitchd-core, pure) [checkpoint: 5db4a5f]
<!-- execution: parallel -->
<!-- depends: phase1 -->

- [x] Task 1: Thompson Sampling [4c19ecc] — Monte-Carlo probability-best over existing
      `bayesian::analyze_*` posteriors → weight vector; golden-vector + Monte-Carlo tests.
      <!-- files: crates/stitchd-core/src/experimentation/bandit/thompson.rs -->
- [x] Task 2: Epsilon-greedy + UCB [be6b2ac] allocators from sufficient stats; golden-vector tests.
      <!-- files: crates/stitchd-core/src/experimentation/bandit/epsilon.rs, crates/stitchd-core/src/experimentation/bandit/ucb.rs -->
- [x] Task 3: Weight normalization [40292d9] + `min_exploration_bp` floor enforcement (sum=10000,
      every arm ≥ floor); property tests (floor preserved, sums exact).
      <!-- files: crates/stitchd-core/src/experimentation/bandit/allocation.rs -->
- [x] Task 4: Reward combiner [40f1733] — scalarization (goal-normalized weighted sum) + constrained
      (primary + guardrail-constraint down-weighting); per-objective passthrough; tests.
      <!-- files: crates/stitchd-core/src/experimentation/bandit/reward.rs -->
      <!-- depends: task1 -->
- [x] Task: Conductor - User Manual Verification 'Phase 2' [autonomous] (Protocol in workflow.md)

## Phase 3: Privileged Allocation-Write Path [checkpoint: 5db4a5f]
<!-- execution: parallel -->
<!-- depends: phase1 -->

- [x] Task 1: System-actor allocation-update RPC [e7f8e96] on flag-service — writes bound rule
      allocation / `default_rule_distribution`, version-bumped, audit as bandit/system actor,
      BYPASSES the whole-flag human lock; tests incl. concurrent human-mutation-still-409.
      <!-- files: crates/stitchd-flag-service/src/*, proto/flags/v1/flag_service.proto -->
- [x] Task 2: experimentation-service hook [364e445] exposing the bound-rule target + lock-aware
      dispatch wrapper for the bandit writer; tests.
      <!-- files: crates/stitchd-experimentation-service/src/* -->
      <!-- depends: task1 -->
- [x] Task: Conductor - User Manual Verification 'Phase 3' [autonomous] (Protocol in workflow.md)

## Phase 4: Stats-Service Static Reallocation Pass [checkpoint: 17f077c]
<!-- execution: sequential -->
<!-- depends: phase2, phase3 -->

- [ ] Task 1: `bandit.rs` in stats-service — per-tick: read sufficient stats (reuse
      `queries/variant_stats.rs`), compute weights (Phase 2), call allocation-write (Phase 3),
      record `bandit_allocation_runs`; skip on insufficient data / external lock; idempotent.
      <!-- files: crates/stitchd-stats-service/src/bandit.rs, crates/stitchd-stats-service/src/compute.rs -->
- [ ] Task 2: Wire into `run_stats_compute` tick + `TriggerRecompute`; live-ClickHouse
      integration test (self-seeding) `tests/bandit_reallocation.rs`; **update CI live-CH
      `--test` list in ci.yml**.
      <!-- files: crates/stitchd-stats-service/src/compute.rs, crates/stitchd-stats-service/tests/bandit_reallocation.rs, .github/workflows/ci.yml -->
      <!-- depends: task1 -->
- [x] Task: Conductor - User Manual Verification 'Phase 4' [autonomous] (Protocol in workflow.md)

## Phase 5: Real-Time Eval Path (snapshot-resident, bandit-aware) [checkpoint: 33fac68]
<!-- execution: sequential -->
<!-- depends: phase2, phase3 -->

- [ ] Task 1: Snapshot carries real-time bandit model params on the bound rule (mirrors
      `ExclusionGate`); proto/snapshot plumbing; SDK snapshot fetch covers it.
      <!-- files: crates/stitchd-core/src/evaluation/*, proto/flags/v1/flag_service.proto -->
- [ ] Task 2: `evaluate_flag` bandit-aware sampling step — gated to realtime-mode rules,
      zero-DB, deterministic under context-seeded RNG; identical in preview + Rust SDK;
      trace names variant + weights only (no privateParameters). Tests: invariant untouched
      for fixed/static rules; preview==SDK parity.
      <!-- files: crates/stitchd-core/src/evaluation/evaluate_flag.rs -->
      <!-- depends: task1 -->
- [ ] Task 3: Stats-tick refreshes real-time model params in snapshot (instead of static %).
      <!-- files: crates/stitchd-stats-service/src/bandit.rs -->
      <!-- depends: task2 -->
- [x] Task: Conductor - User Manual Verification 'Phase 5' [autonomous] (Protocol in workflow.md)

## Phase 6: Contextual Bandit [checkpoint: f2dec88]
<!-- execution: sequential -->
<!-- depends: phase5 -->

- [ ] Task 1: Per-context linear/logistic reward model (LinUCB / Thompson-on-linear) in
      stitchd-core; fit from sufficient stats over context features; golden-vector tests.
      <!-- files: crates/stitchd-core/src/experimentation/bandit/contextual.rs -->
- [ ] Task 2: Model fit in stats-service tick → params onto snapshot; eval-time per-context
      sampling via Phase 5 path; live-CH integration test.
      <!-- files: crates/stitchd-stats-service/src/bandit.rs, crates/stitchd-stats-service/tests/bandit_contextual.rs, .github/workflows/ci.yml -->
      <!-- depends: task1 -->
- [x] Task: Conductor - User Manual Verification 'Phase 6' [autonomous] (Protocol in workflow.md)

## Phase 7: Autonomous Lifecycle (convergence / commit / rollout) [checkpoint: b8667fd]
<!-- execution: sequential -->
<!-- depends: phase4 -->

- [x] Task 1: Convergence detector [c25943b] (posterior prob-best threshold) in stitchd-core; tests.
      <!-- files: crates/stitchd-core/src/experimentation/bandit/convergence.rs -->
- [x] Task 2: Lifecycle executor [fa074b3] in experimentation/stats-service — `advisory` badge,
      `auto_commit` (lock to winner), `auto_rollout` (commit + auto-stop + promote winner to
      standing rule/default + release lock); idempotent; audited in `bandit_allocation_runs`.
      <!-- files: crates/stitchd-stats-service/src/bandit.rs, crates/stitchd-experimentation-service/src/* -->
      <!-- depends: task1 -->
- [x] Task: Conductor - User Manual Verification 'Phase 7' [autonomous] (Protocol in workflow.md)

## Phase 8: Autonomous Optimization Campaigns [checkpoint: b8667fd]
<!-- execution: sequential -->
<!-- depends: phase7 -->

- [x] Task 1: Campaign entity [aebebdc] + RPCs; on-convergence spawn next iteration (winner=new
      control + new variants); on-reward-drift reopen exploration; `max_iterations`/budget caps;
      `action=spawn_iteration` audit. Tests incl. cap enforcement + idempotent spawn.
      <!-- files: crates/stitchd-experimentation-service/src/*, crates/stitchd-stats-service/src/bandit.rs -->
- [x] Task: Conductor - User Manual Verification 'Phase 8' [autonomous] (Protocol in workflow.md)

## Phase 9: Multi-Objective Wiring & Constrained Guardrails [checkpoint: pending]
<!-- execution: sequential -->
<!-- depends: phase4 -->

- [x] Task 1: End-to-end scalarization config [bbae68b] (objective weights) + constrained guardrail
      down-weighting in the stats-compute reward path (uses Phase 2 combiner); per-objective
      posteriors persisted for surfacing; live-CH integration test.
      <!-- files: crates/stitchd-stats-service/src/bandit.rs, crates/stitchd-stats-service/src/compute.rs -->
- [x] Task: Conductor - User Manual Verification 'Phase 9' [autonomous] (Protocol in workflow.md)

## Phase 10: Bandit-Aware Interaction Analysis + Order 4+
<!-- execution: sequential -->
<!-- depends: phase4 -->

- [ ] Task 1: Time-varying-allocation correctness in interaction sweep + SRM (no spurious
      flags when a participant is a bandit); tests over shifting-allocation fixtures.
      <!-- files: crates/stitchd-stats-service/src/interaction_pairs.rs, crates/stitchd-stats-service/src/compute.rs -->
- [ ] Task 2: Generalize interaction order cap 3 → operator-bounded 4+; extend hierarchical
      decomposition + BH-FDR; live-CH integration test; **update CI `--test` list**.
      <!-- files: crates/stitchd-stats-service/src/*, .github/workflows/ci.yml -->
      <!-- depends: task1 -->
- [ ] Task: Conductor - User Manual Verification 'Phase 10' (Protocol in workflow.md)

## Phase 11: REST/Proto Output Surfacing
<!-- execution: sequential -->
<!-- depends: phase4, phase7, phase8, phase9 -->

- [ ] Task 1: Surface `bandit_allocation` + per-objective posteriors + convergence/commit
      state + campaign status + allocation history through WriteExperimentResults →
      experiment_results → VariantResult → REST; new bandit history/timeline read endpoint.
      <!-- files: crates/stitchd-gateway/src/routes/experiments.rs, crates/stitchd-analytics-service/src/*, crates/stitchd-experimentation-service/src/* -->
- [ ] Task: Conductor - User Manual Verification 'Phase 11' (Protocol in workflow.md)

## Phase 12: Admin UI
<!-- execution: parallel -->
<!-- depends: phase11 -->

- [ ] Task 1: Create/Edit form — mode picker, algorithm, propagation, lifecycle policy,
      campaign config, multi-objective weights/constraints; vitest.
      <!-- files: admin/src/pages/experiments/CreateExperimentModal.tsx -->
- [ ] Task 2: Bandit Results view — live allocation-over-time chart, per-arm weights +
      reward posteriors (per objective), convergence/commit badge, lifecycle + campaign
      timeline; vitest.
      <!-- files: admin/src/pages/experiments/tabs/Results.tsx, admin/src/pages/experiments/tabs/Bandit.tsx -->
      <!-- depends: task1 -->
- [ ] Task 3: Interaction tab — order 4+ + bandit notes surfacing; vitest.
      <!-- files: admin/src/pages/experiments/tabs/Interactions.tsx -->
- [ ] Task: Conductor - User Manual Verification 'Phase 12' (Protocol in workflow.md)

## Phase 13: Integration, Docs & CI Hardening
<!-- execution: sequential -->
<!-- depends: phase12 -->

- [ ] Task 1: End-to-end bandit lifecycle integration test (create→reallocate→converge→
      rollout); contextual + campaign + multi-objective E2E; verify eval invariant intact for
      non-bandit flags.
      <!-- files: crates/stitchd-stats-service/tests/*, tests/e2e/* -->
- [ ] Task 2: Docs — env vars, proto pages, crate READMEs, product.md status row; `cargo
      xtask docs` idempotent; OpenAPI contract; final CI green sweep.
      <!-- files: docs/*, conductor/product.md, crates/*/README.md -->
      <!-- depends: task1 -->
- [ ] Task: Conductor - User Manual Verification 'Phase 13' (Protocol in workflow.md)
