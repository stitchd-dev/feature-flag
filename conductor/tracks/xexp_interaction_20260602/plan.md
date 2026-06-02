# Plan: Cross-Experiment Interaction — Exclusion Groups + Interaction Analysis

**Track ID:** `xexp_interaction_20260602`
**Spec:** [./spec.md](./spec.md)

Methodology: TDD (Red → Green → Refactor) per task; ≥90% per-crate coverage; each phase
closes with the Phase Completion Verification & Checkpointing Protocol (workflow.md §85).

Parallelism: Phases 2, 3, and 4 fan out after Phase 1. Phase 5 → Phase 6; Phase 7 needs
Phases 3 + 6; Phase 8 needs Phase 7; Phase 9 needs Phase 8. Shared seams:
`stitchd-experimentation-service/src/service.rs` (Phases 3 & 6) and
`stitchd-core/src/evaluation/` (Phases 1 & 2) — keep additions small + adjacent.

---

## Phase 1: Schema & Domain Model
<!-- execution: parallel -->
<!-- depends: -->

- [ ] Task: Core bucket math — `group_bucket(context_key, salt) -> u16` (0–9999) + range-membership
      helper in `stitchd-core`, reusing the existing Murmur3 hashing. TDD: determinism, distribution,
      boundary (lo inclusive / hi exclusive) tests first.
  <!-- files: crates/stitchd-core/src/evaluation/exclusion.rs, crates/stitchd-core/src/evaluation/mod.rs -->
- [ ] Task: Core domain types — `ExclusionGroup`, `BucketRange`, `experiments` model fields
      (`exclusion_group_id`, `group_bucket_lo/hi`), and an optional `exclusion_gate` on the rule's
      percentage-allocation type (RolloutDistribution) — NOT an experiment binding.
  <!-- files: crates/stitchd-core/src/experiment.rs, crates/stitchd-core/src/rule_engine/types.rs -->
  <!-- depends: task1 -->
- [ ] Task: PostgreSQL migration — `exclusion_groups` table (env-scoped, unique name, immutable salt,
      version/audit/soft-delete) + `experiments` ALTER (group_id + bucket range) + snapshot columns on
      `experiment_iterations`. Add partial soft-delete index.
  <!-- files: crates/stitchd-db/migrations/20260602000001_exclusion_groups.sql -->
- [ ] Task: ClickHouse migration — `experiment_interactions` table keyed on
      `(env_id, experiment_id_a, experiment_id_b, context_type, metric_key)`.
  <!-- files: crates/stitchd-event-writer/migrations/20260602000002_experiment_interactions.sql -->
- [ ] Task: Proto additions — `ExclusionGroup` message + group fields on `Experiment`/`ExperimentIteration`;
      `exclusion_gate` fields on the `PercentageAllocation`/rollout message; group-management RPCs;
      `Interaction*` messages + read RPC. All additive (proto3 optional / new).
  <!-- files: proto/experiments/v1/experimentation_service.proto, proto/flags/v1/*.proto -->
- [ ] Task: Conductor - User Manual Verification 'Schema & Domain Model' (Protocol in workflow.md)

## Phase 2: Exclusion-Group Eval Gating
<!-- execution: parallel -->
<!-- depends: phase1 -->

- [ ] Task: Extend `evaluate_flag` with rule-resident gate — when the matched rule carries an
      `exclusion_gate`, enroll only if `group_bucket(context_key, salt) ∈ [lo, hi)`; else fall through
      to non-experiment outcome. Core stays pure/non-async/experiment-unaware. TDD: in-range enrolls,
      out-of-range holds out, ungrouped unchanged, NO I/O on path.
  <!-- files: crates/stitchd-core/src/evaluation/engine.rs -->
- [ ] Task: Carry the gate on the flag-definition snapshot (PG read → proto → flag-service
      server-streaming sync), reusing the existing `hash_inputs` plumbing.
  <!-- files: crates/stitchd-flag-service/src/service.rs, crates/stitchd-db/src/repository/pg/flag.rs -->
- [ ] Task: Preview-path parity — evaluate-preview honors the gate; trace surfaces "held out by
      exclusion group" reason (in-memory, same path as RolloutDebug).
  <!-- files: crates/stitchd-core/src/evaluation/engine.rs, crates/stitchd-flag-service/src/preview.rs -->
  <!-- depends: task1 -->
- [ ] Task: SDK parity test — `stitchd-sdk-rust::evaluate` produces identical gated outcomes as preview,
      reading only the in-memory `ArcSwap` snapshot (no network during eval).
  <!-- files: sdks/rust/tests/exclusion_gating.rs -->
  <!-- depends: task2 -->
- [ ] Task: Conductor - User Manual Verification 'Exclusion-Group Eval Gating' (Protocol in workflow.md)

## Phase 3: Exclusion-Group Management Service
<!-- execution: parallel -->
<!-- depends: phase1 -->

- [ ] Task: PG repository — exclusion_groups CRUD + range allocator (find disjoint free range sized by
      traffic_allocation; reject on insufficient capacity; free range on stop/delete). TDD: allocation,
      capacity rejection, free-and-reuse, optimistic-concurrency conflict.
  <!-- files: crates/stitchd-db/src/repository/pg/exclusion_group.rs -->
- [ ] Task: experimentation-service RPCs — CreateExclusionGroup / ListExclusionGroups /
      UpdateExclusionGroup / DeleteExclusionGroup + AssignExperimentToGroup / UnassignExperiment;
      assignment **stamps** the `exclusion_gate` onto the bound rule's distribution (clears on unassign).
  <!-- files: crates/stitchd-experimentation-service/src/service.rs -->
  <!-- depends: task1 -->
- [ ] Task: Lifecycle wiring — TransitionExperiment frees the group range + clears the rule gate on stop;
      group membership is a locked attribute while running/paused (extends the whole-flag lock).
  <!-- files: crates/stitchd-experimentation-service/src/service.rs -->
  <!-- depends: task2 -->
- [ ] Task: Conductor - User Manual Verification 'Exclusion-Group Management Service' (Protocol in workflow.md)

## Phase 4: Interaction Detection Query
<!-- execution: parallel -->
<!-- depends: phase1 -->

- [ ] Task: Detection query builder — self-join `experiment_assignments` on
      `(env_id, context_type, context_key)` across distinct experiment_ids with overlapping windows;
      emit shared-context count + per-(Aᵥ × Bᵥ) cell counts. Pure `BuiltQuery` (parameterized).
      TDD against query-builder snapshot + a seeded ClickHouse fixture.
  <!-- files: crates/stitchd-stats-service/src/queries/interaction.rs, crates/stitchd-stats-service/src/queries/mod.rs -->
- [ ] Task: Pair enumerator — list candidate overlapping experiment pairs for an env (distinct flags,
      overlapping active windows, shared metric_ids, NOT same exclusion group).
  <!-- files: crates/stitchd-stats-service/src/interaction_pairs.rs -->
  <!-- depends: task1 -->
- [ ] Task: Conductor - User Manual Verification 'Interaction Detection Query' (Protocol in workflow.md)

## Phase 5: Interaction Significance Math
<!-- execution: parallel -->
<!-- depends: phase4 -->

- [ ] Task: Two-way interaction test for binary metrics (conversion) — main effects + A×B interaction
      term with p-value; insufficient-data → null. TDD: planted interaction flagged, independent effects
      not (no false positive), small-sample null.
  <!-- files: crates/stitchd-stats-service/src/stats/interaction.rs -->
- [ ] Task: Two-way interaction test for continuous metrics (revenue/duration/numeric) — two-way model
      on cell means/variances; interaction significance.
  <!-- files: crates/stitchd-stats-service/src/stats/interaction_continuous.rs -->
- [ ] Task: Conductor - User Manual Verification 'Interaction Significance Math' (Protocol in workflow.md)

## Phase 6: Stats-Service Orchestration + Read RPCs
<!-- execution: sequential -->
<!-- depends: phase5 -->

- [ ] Task: Wire interaction computation into the 60-min schedule + on-demand recompute; write rows to
      `experiment_interactions`; skip same-group (mutually excluded) pairs.
  <!-- files: crates/stitchd-stats-service/src/dispatch.rs, crates/stitchd-stats-service/src/results_writer.rs -->
- [ ] Task: experimentation-service read path — `GetExperimentInteractions` RPC reading
      `experiment_interactions` for an experiment + context type.
  <!-- files: crates/stitchd-experimentation-service/src/service.rs, crates/stitchd-experimentation-service/src/analytics_client.rs -->
- [ ] Task: Conductor - User Manual Verification 'Stats-Service Orchestration + Read RPCs' (Protocol in workflow.md)

## Phase 7: Gateway REST Surface
<!-- execution: parallel -->
<!-- depends: phase3, phase6 -->

- [ ] Task: REST routes — exclusion-group CRUD + assign/unassign + capacity; interaction-results read.
      Pure REST↔gRPC translation (lean-gateway); `#[utoipa::path]` annotations; canonical error mapping.
  <!-- files: crates/stitchd-gateway/src/routes/exclusion_groups.rs, crates/stitchd-gateway/src/routes/experiments.rs, crates/stitchd-gateway/src/router.rs -->
- [ ] Task: Conductor - User Manual Verification 'Gateway REST Surface' (Protocol in workflow.md)

## Phase 8: Admin UI
<!-- execution: parallel -->
<!-- depends: phase7 -->

- [ ] Task: Exclusion-group management UI — list/create/edit groups + capacity view (allocated vs free
      bucket space, member experiments). Formik + Yup; vitest.
  <!-- files: admin/src/pages/experiments/exclusionGroups/, admin/src/lib/api/exclusionGroups.ts -->
- [ ] Task: CreateExperiment group picker — optional group select with remaining-capacity validation.
  <!-- files: admin/src/pages/experiments/CreateExperimentModal.tsx -->
- [ ] Task: ExperimentDetail Interactions tab + Results-tab warning banner — overlaps, per-cell
      breakdown, significance verdict.
  <!-- files: admin/src/pages/experiments/tabs/Interactions.tsx, admin/src/pages/experiments/ExperimentDetail.tsx, admin/src/pages/experiments/tabs/Results.tsx -->
- [ ] Task: Conductor - User Manual Verification 'Admin UI' (Protocol in workflow.md)

## Phase 9: Docs + Final Verification & Sync
<!-- execution: sequential -->
<!-- depends: phase8 -->

- [ ] Task: Update product.md / tech-stack.md (new tables, RPCs, env if any); regenerate
      `cargo xtask docs` and confirm idempotency; refresh patterns.md with any discovered patterns.
  <!-- files: conductor/product.md, conductor/tech-stack.md, conductor/patterns.md -->
- [ ] Task: Full quality gate — workspace tests, clippy, sqlx offline prepare (CI flags), admin vitest,
      docs diff.
  <!-- depends: task1 -->
- [ ] Task: Conductor - User Manual Verification 'Docs + Final Verification & Sync' (Protocol in workflow.md)
