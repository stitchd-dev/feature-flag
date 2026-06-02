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

## Phase 1: Schema & Domain Model  [checkpoint: 24a103a]
<!-- execution: parallel -->
<!-- depends: -->

- [x] Task: Core bucket math — `group_bucket(context_key, salt) -> u16` (0–9999) + range-membership
      helper in `stitchd-core`, reusing the existing Murmur3 hashing. TDD: determinism, distribution,
      boundary (lo inclusive / hi exclusive) tests first.
  <!-- files: crates/stitchd-core/src/evaluation/exclusion.rs, crates/stitchd-core/src/evaluation/mod.rs -->
- [x] Task: Core domain types — `ExclusionGroup`, `BucketRange`, `experiments` model fields  [24a103a]
      (`exclusion_group_id`, `group_bucket_lo/hi`), and an optional `exclusion_gate` on the rule's
      percentage-allocation type (RolloutDistribution) — NOT an experiment binding.
  <!-- files: crates/stitchd-core/src/experiment.rs, crates/stitchd-core/src/rule_engine/types.rs -->
  <!-- depends: task1 -->
- [x] Task: PostgreSQL migration — `exclusion_groups` table (env-scoped, unique name, immutable salt,  [e601a78]
      version/audit/soft-delete) + `experiments` ALTER (group_id + bucket range) + snapshot columns on
      `experiment_iterations`. Add partial soft-delete index.
  <!-- files: crates/stitchd-db/migrations/20260602000001_exclusion_groups.sql -->
- [x] Task: ClickHouse migration — `experiment_interactions` table keyed on  [553f9f3]
      `(env_id, experiment_id_a, experiment_id_b, context_type, metric_key)`.
  <!-- files: crates/stitchd-event-writer/migrations/20260602000002_experiment_interactions.sql -->
- [x] Task: Proto additions — `ExclusionGroup` message + group fields on `Experiment`/`ExperimentIteration`;  [e99e1c1]
      `exclusion_gate` fields on the `PercentageAllocation`/rollout message; group-management RPCs;
      `Interaction*` messages + read RPC. All additive (proto3 optional / new).
  <!-- files: proto/experiments/v1/experimentation_service.proto, proto/flags/v1/*.proto -->
- [x] Task: Conductor - User Manual Verification 'Schema & Domain Model' (Protocol in workflow.md)  [24a103a]

## Phase 2: Exclusion-Group Eval Gating  [checkpoint: 847f220]
<!-- execution: parallel -->
<!-- depends: phase1 -->

- [x] Task: Extend `evaluate_flag` with rule-resident gate — when the matched rule carries an  [a999b0e]
      `exclusion_gate`, enroll only if `group_bucket(context_key, salt) ∈ [lo, hi)`; else fall through
      to non-experiment outcome. Core stays pure/non-async/experiment-unaware. TDD: in-range enrolls,
      out-of-range holds out, ungrouped unchanged, NO I/O on path.
  <!-- files: crates/stitchd-core/src/evaluation/engine.rs -->
- [x] Task: Carry the gate on the flag-definition snapshot (PG read → proto → flag-service  [a999b0e]
      server-streaming sync), reusing the existing `hash_inputs` plumbing.
  <!-- files: crates/stitchd-flag-service/src/service.rs, crates/stitchd-db/src/repository/pg/flag.rs -->
- [x] Task: Preview-path parity — evaluate-preview honors the gate; trace surfaces "held out by  [a999b0e]
      exclusion group" reason (in-memory, same path as RolloutDebug).
  <!-- files: crates/stitchd-core/src/evaluation/engine.rs, crates/stitchd-flag-service/src/preview.rs -->
  <!-- depends: task1 -->
- [x] Task: SDK parity test — `stitchd-sdk-rust::evaluate` produces identical gated outcomes as preview,  [a999b0e]
      reading only the in-memory `ArcSwap` snapshot (no network during eval).
  <!-- files: sdks/rust/tests/exclusion_gating.rs -->
  <!-- depends: task2 -->
- [x] Task: Conductor - User Manual Verification 'Exclusion-Group Eval Gating' (Protocol in workflow.md)  [a999b0e]

## Phase 3: Exclusion-Group Management Service  [checkpoint: 847f220]
<!-- execution: parallel -->
<!-- depends: phase1 -->

- [x] Task: PG repository — exclusion_groups CRUD + range allocator (find disjoint free range sized by  [e4ea2cf]
      traffic_allocation; reject on insufficient capacity; free range on stop/delete). TDD: allocation,
      capacity rejection, free-and-reuse, optimistic-concurrency conflict.
  <!-- files: crates/stitchd-db/src/repository/pg/exclusion_group.rs -->
- [x] Task: experimentation-service RPCs — CreateExclusionGroup / ListExclusionGroups /  [e4ea2cf]
      UpdateExclusionGroup / DeleteExclusionGroup + AssignExperimentToGroup / UnassignExperiment;
      assignment **stamps** the `exclusion_gate` onto the bound rule's distribution (clears on unassign).
  <!-- files: crates/stitchd-experimentation-service/src/service.rs -->
  <!-- depends: task1 -->
- [x] Task: Lifecycle wiring — TransitionExperiment frees the group range + clears the rule gate on stop;  [e4ea2cf]
      group membership is a locked attribute while running/paused (extends the whole-flag lock).
  <!-- files: crates/stitchd-experimentation-service/src/service.rs -->
  <!-- depends: task2 -->
- [x] Task: Conductor - User Manual Verification 'Exclusion-Group Management Service' (Protocol in workflow.md)  [e4ea2cf]

## Phase 4: Interaction Detection Query  [checkpoint: 847f220]
<!-- execution: parallel -->
<!-- depends: phase1 -->

- [x] Task: Detection query builder — self-join `experiment_assignments` on  [dddb240]
      `(env_id, context_type, context_key)` across distinct experiment_ids with overlapping windows;
      emit shared-context count + per-(Aᵥ × Bᵥ) cell counts. Pure `BuiltQuery` (parameterized).
      TDD against query-builder snapshot + a seeded ClickHouse fixture.
  <!-- files: crates/stitchd-stats-service/src/queries/interaction.rs, crates/stitchd-stats-service/src/queries/mod.rs -->
- [x] Task: Pair enumerator — list candidate overlapping experiment pairs for an env (distinct flags,  [dddb240]
      overlapping active windows, shared metric_ids, NOT same exclusion group).
  <!-- files: crates/stitchd-stats-service/src/interaction_pairs.rs -->
  <!-- depends: task1 -->
- [x] Task: Conductor - User Manual Verification 'Interaction Detection Query' (Protocol in workflow.md)  [dddb240]

## Phase 5: Interaction Significance Math  [checkpoint: ce6e97f]
<!-- execution: parallel -->
<!-- depends: phase4 -->

- [x] Task: Two-way interaction test for binary metrics (conversion) — main effects + A×B interaction  [ce6e97f]
      term with p-value; insufficient-data → null. TDD: planted interaction flagged, independent effects
      not (no false positive), small-sample null.
  <!-- files: crates/stitchd-stats-service/src/stats/interaction.rs -->
- [x] Task: Two-way interaction test for continuous metrics (revenue/duration/numeric) — two-way model  [ce6e97f]
      on cell means/variances; interaction significance.
  <!-- files: crates/stitchd-stats-service/src/stats/interaction_continuous.rs -->
- [x] Task: Conductor - User Manual Verification 'Interaction Significance Math' (Protocol in workflow.md)  [ce6e97f]

## Phase 6: Stats-Service Orchestration + Read RPCs  [checkpoint: 13df229]
<!-- execution: sequential -->
<!-- depends: phase5 -->

- [x] Task: Wire interaction computation into the 60-min schedule + on-demand recompute; write rows to  [13df229]
      `experiment_interactions`; skip same-group (mutually excluded) pairs.
  <!-- files: crates/stitchd-stats-service/src/dispatch.rs, crates/stitchd-stats-service/src/results_writer.rs -->
- [x] Task: experimentation-service read path — `GetExperimentInteractions` RPC reading  [13df229]
      `experiment_interactions` for an experiment + context type.
  <!-- files: crates/stitchd-experimentation-service/src/service.rs, crates/stitchd-experimentation-service/src/analytics_client.rs -->
- [x] Task: Conductor - User Manual Verification 'Stats-Service Orchestration + Read RPCs' (Protocol in workflow.md)  [13df229]

## Phase 7: Gateway REST Surface  [checkpoint: dbff080]
<!-- execution: parallel -->
<!-- depends: phase3, phase6 -->

- [x] Task: REST routes — exclusion-group CRUD + assign/unassign + capacity; interaction-results read.  [dbff080]
      Pure REST↔gRPC translation (lean-gateway); `#[utoipa::path]` annotations; canonical error mapping.
  <!-- files: crates/stitchd-gateway/src/routes/exclusion_groups.rs, crates/stitchd-gateway/src/routes/experiments.rs, crates/stitchd-gateway/src/router.rs -->
- [x] Task: Conductor - User Manual Verification 'Gateway REST Surface' (Protocol in workflow.md)  [dbff080]

## Phase 8: Admin UI  [checkpoint: 229053c]
<!-- execution: parallel -->
<!-- depends: phase7 -->

- [x] Task: Exclusion-group management UI — list/create/edit groups + capacity view (allocated vs free  [229053c]
      bucket space, member experiments). Formik + Yup; vitest.
  <!-- files: admin/src/pages/experiments/exclusionGroups/, admin/src/lib/api/exclusionGroups.ts -->
- [x] Task: CreateExperiment group picker — optional group select with remaining-capacity validation.  [229053c]
  <!-- files: admin/src/pages/experiments/CreateExperimentModal.tsx -->
- [x] Task: ExperimentDetail Interactions tab + Results-tab warning banner — overlaps, per-cell  [229053c]
      breakdown, significance verdict.
  <!-- files: admin/src/pages/experiments/tabs/Interactions.tsx, admin/src/pages/experiments/ExperimentDetail.tsx, admin/src/pages/experiments/tabs/Results.tsx -->
- [x] Task: Conductor - User Manual Verification 'Admin UI' (Protocol in workflow.md)  [229053c]

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
