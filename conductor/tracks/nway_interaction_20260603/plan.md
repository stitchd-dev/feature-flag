# Plan: N-Way Interaction + Funnel/Ratio + Bayesian

**Track:** nway_interaction_20260603
**Spec:** ./spec.md  |  **Workflow:** TDD (red→green), ≥90% coverage, phase checkpoints

Writer = stitchd-stats-service · Reader/RPC = stitchd-experimentation-service ·
Math = stitchd-core · REST = stitchd-gateway · UI = admin/

Parallel execution: ENABLED (Phase 2 stats modules, Phase 4 consumers, Phase 5 views).

---

## Phase 1: Foundations — Tech-Stack, Unified Schema, N-D Cell Model
<!-- execution: sequential -->
<!-- depends: -->

- [ ] Task 1: Document stats additions in `tech-stack.md` (log-linear hierarchical
      decomposition, multi-factor ANOVA, ratio delta-method, Bayesian interaction
      posteriors) with dated note — REQUIRED before implementation per workflow §7.
  <!-- files: conductor/tech-stack.md -->

- [ ] Task 2: New ClickHouse migration superseding `experiment_interactions`:
      `experiment_ids Array(UUID)`, `interaction_order UInt8`, `term LowCardinality(String)`,
      generalized N-D `cell_stats`, Bayesian columns (`bayes_prob`, `bayes_expected`,
      `bayes_ci_low`, `bayes_ci_high` Float64). Register in `migrations.rs` embed list.
      Tests: migration applies on a clean CH; ORDER BY key updated.
  <!-- files: crates/stitchd-event-writer/migrations/20260603000001_nway_interactions.sql, crates/stitchd-event-writer/src/migrations.rs -->
  <!-- depends: task1 -->

- [ ] Task 3: Generalize the cell aggregate to an N-dimensional, variant-tuple-keyed
      type + `cell_stats` (de)serialization (carries n, successes, value_sum,
      value_sq_sum, plus ratio num/den sums). TDD: serde round-trip + k-D indexing
      fixtures (k=2 and k=3).
  <!-- files: crates/stitchd-stats-service/src/interaction_compute.rs -->
  <!-- depends: task2 -->

- [ ] Task: Conductor - User Manual Verification 'Phase 1: Foundations' (Protocol in workflow.md)

## Phase 2: Core Statistics (TDD, parallel by model/family)
<!-- execution: parallel -->
<!-- depends: phase1 -->

- [ ] Task 1: Scaffold `interaction/` module — split existing `interaction.rs` into
      `interaction/mod.rs`; extend the result type with `term`, `interaction_order`,
      and Bayesian fields; keep legacy 2-way fns intact as the regression baseline.
      (Shared seam — done first, gates the parallel workers.)
  <!-- files: crates/stitchd-core/src/experimentation/stats/interaction/mod.rs -->

- [ ] Task 2: Frequentist **log-linear** hierarchical decomposition for binary metrics
      (R×C×D): emits main-effect, all 2-way, and the 3-way interaction terms with
      chi-square + correct df. TDD vs hand-computed contingency fixtures.
  <!-- files: crates/stitchd-core/src/experimentation/stats/interaction/loglinear.rs -->
  <!-- depends: task1 -->

- [ ] Task 3: Frequentist **multi-factor ANOVA** decomposition for continuous metrics:
      main effects + 2-way + 3-way interaction F-tests. TDD vs hand-computed fixtures.
  <!-- files: crates/stitchd-core/src/experimentation/stats/interaction/anova.rs -->
  <!-- depends: task1 -->

- [ ] Task 4: **Ratio** delta-method interaction contrast (variance via delta method;
      `min_denominator` → insufficient_data). TDD vs fixtures + degenerate-denominator cases.
  <!-- files: crates/stitchd-core/src/experimentation/stats/interaction/ratio.rs -->
  <!-- depends: task1 -->

- [ ] Task 5: **Bayesian** binary/funnel posterior — Beta-Binomial cells → difference-
      in-differences interaction contrast; emits prob(≠0 / ROPE), expected effect,
      credible interval. TDD vs analytic/seeded-MC fixtures.
  <!-- files: crates/stitchd-core/src/experimentation/stats/interaction/bayes_binary.rs -->
  <!-- depends: task1 -->

- [ ] Task 6: **Bayesian** continuous/ratio posterior — Normal-Normal cells →
      interaction contrast; prob/expected/credible-interval. TDD vs fixtures.
  <!-- files: crates/stitchd-core/src/experimentation/stats/interaction/bayes_continuous.rs -->
  <!-- depends: task1 -->

- [ ] Task 7: Regression + retire legacy — assert the generalized order-2 path
      reproduces legacy `binary_2x2`/`binary_rxc`/`continuous_interaction` outputs
      bit-for-bit, then remove the superseded fns. (Shared seam, after parallel workers.)
  <!-- files: crates/stitchd-core/src/experimentation/stats/interaction/mod.rs -->
  <!-- depends: task2, task3, task4, task5, task6 -->

- [ ] Task: Conductor - User Manual Verification 'Phase 2: Core Statistics' (Protocol in workflow.md)

## Phase 3: Sweep Orchestration — Enumeration, k-Way Join, Metric Routing, FDR, Persist
<!-- execution: sequential -->
<!-- depends: phase1, phase2 -->

- [ ] Task 1: Extend candidate enumeration to tuples up to order 3 — triple validity
      (every constituent pair satisfies `can_interact`; exclude triples where any two
      co-share an exclusion group). TDD: enumeration fixtures.
  <!-- files: crates/stitchd-stats-service/src/interaction_pairs.rs -->

- [ ] Task 2: Generalize the self-join query builder to k aliases on
      `(env_id, context_type, context_key)`, ITT bound = `greatest(a…k.assigned_at)`,
      variant-tuple cell aggregation. Add **funnel** (windowFunnel → reached/total per
      cell) and **ratio** (numerator/denominator sums per cell) query variants. TDD:
      generated-SQL snapshot tests for k=2, k=3, funnel, ratio.
  <!-- files: crates/stitchd-stats-service/src/queries/interaction_metric.rs -->
  <!-- depends: task1 -->

- [ ] Task 3: Metric-type routing in the sweep — aggregation/conversion→log-linear,
      funnel→binary(reached), continuous→ANOVA, ratio→delta; invoke Phase-2 stats and
      attach Bayesian posteriors per term.
  <!-- files: crates/stitchd-stats-service/src/interaction_compute.rs -->
  <!-- depends: task2 -->

- [ ] Task 4: Apply one BH-FDR (0.05) across the full Frequentist term family (all
      orders + decomposed terms); persist generalized rows (ids array / order / term /
      Frequentist + Bayesian) to CH; insufficient-data sentinels preserved.
  <!-- files: crates/stitchd-stats-service/src/interaction_compute.rs -->
  <!-- depends: task3 -->

- [ ] Task: Conductor - User Manual Verification 'Phase 3: Sweep Orchestration' (Protocol in workflow.md)

## Phase 4: Transport — Proto, Reader/RPC, Gateway REST, OpenAPI
<!-- execution: parallel -->
<!-- depends: phase2, phase3 -->

- [ ] Task 1: Generalize proto `ExperimentInteraction` — `repeated experiment_ids`,
      `interaction_order`, `term`, Bayesian fields (additive field numbers; keep wire
      back-compat). Regenerate prost types. (Shared seam — gates the two consumers.)
  <!-- files: proto/experiments/v1/experimentation_service.proto -->

- [ ] Task 2: Generalize `interactions_reader.rs` to read new columns + map to proto;
      update the `GetExperimentInteractions` service impl. TDD: reader maps a 3-way row.
  <!-- files: crates/stitchd-experimentation-service/src/interactions_reader.rs, crates/stitchd-experimentation-service/src/service.rs -->
  <!-- depends: task1 -->

- [ ] Task 3: Gateway REST DTO + handler + OpenAPI + router for the generalized shape.
      TDD: REST integration test returns 2-way + 3-way rows with Bayesian fields.
  <!-- files: crates/stitchd-gateway/src/routes/experiments.rs, crates/stitchd-gateway/src/openapi.rs, crates/stitchd-gateway/src/router.rs -->
  <!-- depends: task1 -->

- [ ] Task: Conductor - User Manual Verification 'Phase 4: Transport' (Protocol in workflow.md)

## Phase 5: Admin UI — Interactions Tab + Warning Banner
<!-- execution: parallel -->
<!-- depends: phase4 -->

- [ ] Task 1: Generalize the interactions API client TS types — array of participating
      experiments (ids+names), `interaction_order`, `term`, Frequentist + Bayesian fields.
      (Shared seam — gates the views.)
  <!-- files: admin/src/lib/api/exclusionGroups.ts -->

- [ ] Task 2: Generalize the Interactions tab — render 2-way & 3-way rows (participating
      experiments, order, term, metric, shared count), Frequentist estimate/p-value/badge,
      Bayesian prob/expected/credible-interval columns, funnel/ratio value formatting.
  <!-- files: admin/src/pages/experiments/tabs/Interactions.tsx -->
  <!-- depends: task1 -->

- [ ] Task 3: Results-tab warning banner fires on any significant (Frequentist) or
      high-probability (Bayesian) interaction of any order the experiment participates in.
  <!-- files: admin/src/pages/experiments/ExperimentDetail.tsx -->
  <!-- depends: task1 -->

- [ ] Task 4: Vitest coverage for 2-way + 3-way + funnel/ratio + Bayesian rendering and
      the banner gate.
  <!-- files: admin/src/pages/experiments/tabs/__tests__/Interactions.test.tsx -->
  <!-- depends: task2, task3 -->

- [ ] Task: Conductor - User Manual Verification 'Phase 5: Admin UI' (Protocol in workflow.md)

## Phase 6: Integration, Docs & CI Green
<!-- execution: sequential -->
<!-- depends: phase5 -->

- [ ] Task 1: Live-stack end-to-end — seed 3 overlapping experiments across metric
      kinds (aggregation, funnel, ratio, continuous); run the sweep; assert 3-way rows,
      decomposed terms, Bayesian outputs, and insufficient-data behavior; verify the
      REST→UI surfacing.
  <!-- files: crates/stitchd-stats-service/tests/interaction_compute.rs, crates/stitchd-gateway/tests/exclusion_groups_integration.rs -->

- [ ] Task 2: Full CI gate — `cargo fmt --all --check`, clippy `-D warnings`,
      `cargo test --workspace`, `cargo sqlx prepare … --all-targets --features
      stitchd-sdk-rust/test-util`, `cargo xtask docs` + `git diff --exit-code`,
      contract-check, coverage ≥90% per crate.

- [ ] Task 3: Update `product.md` (interaction module now 3-way + funnel/ratio +
      Bayesian; revise the Future list) and append a `patterns.md` note.
  <!-- files: conductor/product.md, conductor/patterns.md -->

- [ ] Task: Conductor - User Manual Verification 'Phase 6: Integration, Docs & CI Green' (Protocol in workflow.md)
