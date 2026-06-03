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

## Phase 3: Sweep Orchestration — Enumeration, k-Way Join, Metric Routing, FDR, Persist [checkpoint: 513f848]
<!-- execution: sequential -->
<!-- depends: phase1, phase2 -->

- [x] Task 1: `candidate_triples` enumeration (94e2e5f) — triple valid iff all 3 pairs
      `can_interact` + common metric across all 3 (Helly: pairwise window-overlap ⇒ common). 7 tests.
  <!-- files: crates/stitchd-stats-service/src/interaction_pairs.rs -->

- [x] Task 2: k-way cell query (88fc51f) — chained k aliases, `array(variant_keys)`,
      `greatest()` ITT bound; agg/conversion+continuous, **funnel** (`windowFunnel`==steps),
      **ratio** (2nd-moment sums) builders. 24 query-shape tests.
  <!-- files: crates/stitchd-stats-service/src/queries/interaction_metric.rs -->
  <!-- depends: task1 -->

- [x] Task 3: Metric-type routing (88fc51f) — pairs+triples × metric × context; classify →
      fetch grid → NdCell → Frequentist `*_terms` + matching `*_bayes`, merged by `TermKind`;
      one row per term with `main:`/`2way:`/`3way:<uuids>` term strings.
  <!-- files: crates/stitchd-stats-service/src/interaction_compute.rs -->
  <!-- depends: task2 -->

- [x] Task 4: Single BH-FDR (0.05) over the whole sweep's Frequentist family; persist
      generalized rows (experiment_ids Array(UUID) via custom uuid_vec serde / order / term /
      df / Frequentist + Bayesian); retired old CellAggregate/compute_result/a-b shape.
  <!-- files: crates/stitchd-stats-service/src/interaction_compute.rs -->
  <!-- depends: task3 -->

- [x] Task: Conductor - User Manual Verification 'Phase 3' (build + clippy + fmt + 78 stats interaction tests green)

## Phase 4: Transport — Proto, Reader/RPC, Gateway REST, OpenAPI [checkpoint: f35bda5]
<!-- execution: parallel -->
<!-- depends: phase2, phase3 -->

- [x] Task 1: proto `ExperimentInteraction` generalized (9836f61) — `repeated experiment_ids`
      + `experiment_names`, `interaction_order`, `term`, `df`, `bayes_*`. Renumbered (not live).
  <!-- files: proto/experiments/v1/experimentation_service.proto -->

- [x] Task 2: `interactions_reader.rs` + service (9836f61) — `FINAL` + `has(experiment_ids, ?)`,
      custom `Array(UUID)` deserialize matching the writer; per-id name resolution. 86 tests.
  <!-- files: crates/stitchd-experimentation-service/src/interactions_reader.rs, crates/stitchd-experimentation-service/src/service.rs -->
  <!-- depends: task1 -->

- [x] Task 3: Gateway `ExperimentInteractionJson` DTO + mapping (9836f61); utoipa
      auto-regenerates the OpenAPI schema. 246 gateway tests; integration asserts a 3-way row.
  <!-- files: crates/stitchd-gateway/src/routes/experiments.rs, crates/stitchd-gateway/src/openapi.rs, crates/stitchd-gateway/src/router.rs -->
  <!-- depends: task1 -->

- [x] Task: Conductor - User Manual Verification 'Phase 4' (workspace build + clippy + fmt green; live round-trip in Phase 6)

## Phase 5: Admin UI — Interactions Tab + Warning Banner [checkpoint: 167571c]
<!-- execution: parallel -->
<!-- depends: phase4 -->

- [x] Task 1: interactions API TS types generalized (699b86f) — N-way `ExperimentInteractionJson`
      shape + `hasSignificantInteraction` (freq || bayes_prob>0.95).
  <!-- files: admin/src/lib/api/exclusionGroups.ts -->

- [x] Task 2: Interactions tab generalized (699b86f) — order badge, humanized term, metric,
      shared count, Frequentist estimate/p/badge, Bayesian prob/expected/CI; sorted by order+metric.
  <!-- files: admin/src/pages/experiments/tabs/Interactions.tsx -->
  <!-- depends: task1 -->

- [x] Task 3: Results banner (699b86f) fires on any significant OR high-probability
      interaction of any order; copy de-pairwise-d.
  <!-- files: admin/src/pages/experiments/ExperimentDetail.tsx -->
  <!-- depends: task1 -->

- [x] Task 4: Vitest (699b86f) — 2-way + 3-way + insufficient rows + banner gate; 824 admin tests green.
  <!-- files: admin/src/pages/experiments/tabs/__tests__/Interactions.test.tsx -->
  <!-- depends: task2, task3 -->

- [x] Task: Conductor - User Manual Verification 'Phase 5' (tsc + lint + 824 vitest green)

## Phase 6: Integration, Docs & CI Green [checkpoint: c3fca5b]
<!-- execution: sequential -->
<!-- depends: phase5 -->

- [x] Task 1: Live-CH writer↔reader round-trip (960fe51) — 5 `--ignored` integration tests
      green against real ClickHouse; caught + fixed a `NO_COMMON_TYPE` alias-collision read bug.
  <!-- files: crates/stitchd-stats-service/tests/interaction_compute.rs -->

- [x] Task 2: Full CI gate GREEN — `cargo fmt --all --check` ✓, `clippy --workspace -D warnings` ✓,
      `cargo test --workspace` ✓ (exit 0, 0 failed), `sqlx prepare --check` ✓, docs idempotent
      (tracked) ✓, contract-check ✓ (23/23 routes covered).

- [x] Task 3: product.md (N-way + funnel/ratio + Bayesian; status row; trimmed Future) +
      patterns.md (7 N-way/parallel-stats patterns). (c3fca5b)
  <!-- files: conductor/product.md, conductor/patterns.md -->

- [x] Task: Conductor - User Manual Verification 'Phase 6' (full CI gate green; live round-trip verified)
