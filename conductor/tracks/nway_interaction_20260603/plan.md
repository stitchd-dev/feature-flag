# Plan: N-Way Interaction + Funnel/Ratio + Bayesian

**Track:** nway_interaction_20260603
**Spec:** ./spec.md  |  **Workflow:** TDD (red→green), ≥90% coverage, phase checkpoints

Writer = stitchd-stats-service · Reader/RPC = stitchd-experimentation-service ·
Math = stitchd-core · REST = stitchd-gateway · UI = admin/

Parallel execution: ENABLED (Phase 2 stats modules, Phase 4 consumers, Phase 5 views).

---

## Phase 1: Foundations — Tech-Stack, Unified Schema, N-D Cell Model [checkpoint: e8f2300]
<!-- execution: sequential -->
<!-- depends: -->

- [x] Task 1: Document stats additions in `tech-stack.md` (log-linear hierarchical
      decomposition, multi-factor ANOVA, ratio delta-method, Bayesian interaction
      posteriors) with dated note — REQUIRED before implementation per workflow §7.
  <!-- files: conductor/tech-stack.md -->

- [x] Task 2: Superseded `experiment_interactions` in place (clean cutover): rewrote
      `20260602000002_experiment_interactions.sql` to the unified schema
      (`experiment_ids Array(UUID)`, `interaction_order UInt8`, `term`, `df`, N-D
      `cell_stats`, Bayesian cols), engine → `ReplacingMergeTree(computed_at)` + 30d TTL;
      removed the separate `…0005` ALTER. Verified DDL on live CH.
  <!-- files: crates/stitchd-event-writer/migrations/20260602000002_experiment_interactions.sql, crates/stitchd-event-writer/src/migrations.rs -->
  <!-- depends: task1 -->

- [x] Task 3: Generalize the cell aggregate to an N-dimensional, variant-tuple-keyed
      type + `cell_stats` (de)serialization (carries n, successes, value_sum,
      value_sq_sum, plus ratio num/den/sq/cov sums). TDD: serde round-trip + k-D indexing
      fixtures (k=2 and k=3).
  <!-- files: crates/stitchd-stats-service/src/interaction_compute.rs -->
  <!-- depends: task2 -->

- [ ] Task: Conductor - User Manual Verification 'Phase 1: Foundations' (Protocol in workflow.md)

## Phase 2: Core Statistics (TDD, parallel by model/family) [checkpoint: c1b6702]
<!-- execution: parallel -->
<!-- depends: phase1 -->

- [x] Task 1: Seam — kept `interaction.rs` with an `interaction/` submodule dir (no
      file move needed); added contract types (TermKind/TermResult/BayesianInteraction
      + NdBinaryCell/NdContinuousCell/NdRatioCell), exposed distribution helpers
      `pub(crate)`, flat-re-exported 5 stub submodules. Legacy fns kept as baseline.
  <!-- files: crates/stitchd-core/src/experimentation/stats/interaction.rs (+ interaction/) -->

- [x] Task 2: Frequentist **log-linear** hierarchical decomposition for binary metrics
      — main + all 2-way + 3-way; 2-way delegates to legacy (regression-exact), Main =
      Pearson χ², 3-way = IPF on outcome-bearing margins + Pearson χ². 15 tests. (90d65ac)
  <!-- files: crates/stitchd-core/src/experimentation/stats/interaction/loglinear.rs -->
  <!-- depends: task1 -->

- [x] Task 3: Frequentist **multi-factor ANOVA** decomposition for continuous metrics:
      common pooled error; main one-way SS, 2-way delegates to legacy, 3-way residual SS.
      14 tests. (85f981b)
  <!-- files: crates/stitchd-core/src/experimentation/stats/interaction/anova.rs -->
  <!-- depends: task1 -->

- [x] Task 4: **Ratio** delta-method interaction — per-cell Var(R), IV-weighted; 2×2/2×2×2
      DiD z-contrasts, general grids weighted residual χ²; Main = Cochran's Q. 19 tests. (582056f)
  <!-- files: crates/stitchd-core/src/experimentation/stats/interaction/ratio.rs -->
  <!-- depends: task1 -->

- [x] Task 5: **Bayesian** binary/funnel posterior — Beta(1,1) cells → Normal-approx of
      the linear contrast; prob=Φ(|E|/sd), expected, 95% CI. Deterministic. 15 tests. (2acb6c7)
  <!-- files: crates/stitchd-core/src/experimentation/stats/interaction/bayes_binary.rs -->
  <!-- depends: task1 -->

- [x] Task 6: **Bayesian** continuous/ratio posterior — Normal-Normal (continuous) +
      delta-method (ratio) cell posteriors → same contrast summary. Deterministic. 17 tests. (fa596eb)
  <!-- files: crates/stitchd-core/src/experimentation/stats/interaction/bayes_continuous.rs -->
  <!-- depends: task1 -->

- [x] Task 7: Integration + regression — order-2 two-way reproduces legacy bit-for-bit
      (public API), 5 modules compose, freq+bayes join by TermKind. 107 interaction tests.
      NOTE: legacy core fns RETAINED (2-way delegates to them); old sweep-path retirement
      moved to Phase 3. (c1b6702)
  <!-- files: crates/stitchd-core/src/experimentation/stats/interaction.rs -->
  <!-- depends: task2, task3, task4, task5, task6 -->

- [x] Task: Conductor - User Manual Verification 'Phase 2: Core Statistics' (automated gate: clippy -D + 107 tests + fmt green)

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
