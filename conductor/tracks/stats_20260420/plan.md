# Plan: Experiment Statistical Analysis
Track: stats_20260420

## Phase 1: Database Schema & Domain Types [checkpoint: 58d8af8]
<!-- execution: parallel -->

- [x] Task 1: PostgreSQL migrations <!-- 97c41d3 -->
  <!-- files: crates/stitchd-db/migrations/ -->
  - [x] Sub-task: Add `analysis_type` column to `experiments` table
        (`frequentist | bayesian`, default `frequentist`)
  - [x] Sub-task: Create `experiment_results` table
        (id, experiment_id, iteration_id, metric_key, metric_type,
        variant_stats JSONB, frequentist_result JSONB, bayesian_result JSONB,
        recommendation, computed_at, created_at)

- [x] Task 2: Domain types in `stitchd-core` <!-- 002b7f2 -->
  <!-- files: crates/stitchd-core/src/experimentation/stats/ -->
  - [x] Sub-task: Write failing tests for AnalysisType enum parsing,
        Recommendation enum ordering
  - [x] Sub-task: `AnalysisType` enum (`Frequentist | Bayesian`)
  - [x] Sub-task: `MetricType` enum (`Count | Numeric | Percentile | Funnel`)
  - [x] Sub-task: `VariantStats` struct (sample_size, conversions, mean,
        variance, conversion_rate, percentiles)
  - [x] Sub-task: `FrequentistResult` (p_value, confidence_interval, significant)
  - [x] Sub-task: `BayesianResult` (prob_best, credible_interval, expected_loss)
  - [x] Sub-task: `MetricResult` (metric_key, metric_type, per-variant stats,
        frequentist/bayesian result, recommendation)
  - [x] Sub-task: `Recommendation` enum with ordering logic
  - [x] Sub-task: `IterationResults` struct (experiment_id, iteration_id,
        iteration_number, computed_at, metrics: Vec<MetricResult>)
  - [x] Sub-task: Pass all tests

- [~] Task: Conductor - User Manual Verification 'Phase 1: Database Schema & Domain Types' (Protocol in workflow.md)

## Phase 2: ClickHouse Query Layer
<!-- execution: parallel -->
<!-- depends: phase1 -->

- [ ] Task 1: ClickHouse aggregation queries
  <!-- files: crates/stitchd-db/src/clickhouse/experiment_queries.rs -->
  - [ ] Sub-task: Write failing unit tests for query builders (mock results)
  - [ ] Sub-task: Query `events_count_mv` for count/conversion metrics
        per (env_id, experiment_id, metric_key, variant, date range)
  - [ ] Sub-task: Query `events_numeric_mv` for numeric sum/avg/percentile metrics
  - [ ] Sub-task: Funnel query: per-step conversion counts, chained via context keys
  - [ ] Sub-task: Pass tests

- [ ] Task 2: Bootstrap sampling for percentile CI
  <!-- files: crates/stitchd-core/src/experimentation/stats/bootstrap.rs -->
  - [ ] Sub-task: Write failing tests for bootstrap CI correctness
        (known distribution → expected interval bounds)
  - [ ] Sub-task: Implement 1000-resample bootstrap on raw percentile data
  - [ ] Sub-task: Pass tests

- [ ] Task: Conductor - User Manual Verification 'Phase 2: ClickHouse Query Layer' (Protocol in workflow.md)

## Phase 3: Statistical Engine
<!-- execution: parallel -->
<!-- depends: phase1 -->

- [ ] Task 1: Frequentist analysis
  <!-- files: crates/stitchd-core/src/experimentation/stats/frequentist.rs -->
  - [ ] Sub-task: Write failing tests for z-test (proportion), Welch t-test (numeric),
        bootstrap CI (percentile), funnel z-test — test against known values
  - [ ] Sub-task: Two-proportion z-test: p-value, 95% CI on lift
  - [ ] Sub-task: Welch's t-test: p-value, 95% CI on mean difference
  - [ ] Sub-task: Percentile CI via bootstrap (delegates to bootstrap module)
  - [ ] Sub-task: Funnel final-step z-test
  - [ ] Sub-task: `significant: bool` threshold at α = 0.05
  - [ ] Sub-task: Pass all tests

- [ ] Task 2: Bayesian analysis
  <!-- files: crates/stitchd-core/src/experimentation/stats/bayesian.rs -->
  - [ ] Sub-task: Write failing tests for Beta-Binomial P(best), Normal-Normal
        credible interval, expected_loss — test against known posteriors
  - [ ] Sub-task: Beta-Binomial posterior for count/conversion metrics
  - [ ] Sub-task: Normal-Normal conjugate for numeric metrics
  - [ ] Sub-task: Bootstrap posterior approximation for percentile metrics
  - [ ] Sub-task: Beta-Binomial for funnel final-step
  - [ ] Sub-task: `prob_best`, `credible_interval`, `expected_loss` per variant
  - [ ] Sub-task: Pass all tests

- [ ] Task 3: Recommendation engine
  <!-- files: crates/stitchd-core/src/experimentation/stats/recommendation.rs -->
  - [ ] Sub-task: Write failing tests for all four outcomes
        (variant_wins, control_wins, inconclusive, needs_more_data)
  - [ ] Sub-task: Frequentist rule: p < 0.05 → winner; else inconclusive
  - [ ] Sub-task: Bayesian rule: P(best) > 0.95 → winner; else inconclusive
  - [ ] Sub-task: `needs_more_data` guard: sample_size < min_sample_size
  - [ ] Sub-task: Pass all tests

- [ ] Task: Conductor - User Manual Verification 'Phase 3: Statistical Engine' (Protocol in workflow.md)

## Phase 4: Results Repository & Compute Pipeline
<!-- depends: phase2, phase3 -->

- [ ] Task 1: Results repository
  <!-- files: crates/stitchd-db/src/experiment_results.rs -->
  - [ ] Sub-task: Write failing tests (sqlx::test) for upsert and fetch by
        experiment_id + iteration_id
  - [ ] Sub-task: `ExperimentResultsRepository` trait + sqlx implementation
  - [ ] Sub-task: Upsert result rows (one per metric per iteration)
  - [ ] Sub-task: Fetch all results for latest iteration
  - [ ] Sub-task: Fetch results for specific iteration
  - [ ] Sub-task: Staleness check: `computed_at` vs iteration `started_at`
  - [ ] Sub-task: Pass tests

- [ ] Task 2: Compute pipeline orchestration
  <!-- files: crates/stitchd-server/src/experimentation/compute.rs -->
  <!-- depends: task1 -->
  - [ ] Sub-task: Write failing tests for orchestrator (mock CH + repo)
  - [ ] Sub-task: `ComputeResultsJob`: fetch CH aggregations → run stats engine
        → persist to results repository
  - [ ] Sub-task: Wire `analysis_type` to select Frequentist or Bayesian engine
  - [ ] Sub-task: Async tokio task; returns immediately (fire-and-forget)
  - [ ] Sub-task: Pass tests

- [ ] Task: Conductor - User Manual Verification 'Phase 4: Results Repository & Compute Pipeline' (Protocol in workflow.md)

## Phase 5: REST API Layer
<!-- depends: phase4 -->

- [ ] Task 1: Request/Response types & utoipa schemas
  <!-- files: crates/stitchd-server/src/experimentation/results_api.rs -->
  - [ ] Sub-task: Write failing tests for response serialization
  - [ ] Sub-task: `ExperimentResultsResponse`, `IterationResultsResponse`,
        `MetricResultResponse` with utoipa `#[utoipa::path]` annotations
  - [ ] Sub-task: Pass tests

- [ ] Task 2: Route handlers
  <!-- files: crates/stitchd-server/src/experimentation/results_api.rs -->
  <!-- depends: task1 -->
  - [ ] Sub-task: Write failing integration tests (tower::oneshot) for:
        GET /experiments/{id}/results,
        GET /experiments/{id}/iterations/{iter}/results,
        POST /experiments/{id}/results/recompute
  - [ ] Sub-task: Implement GET latest results handler (staleness check,
        trigger recompute if stale, return cached)
  - [ ] Sub-task: Implement GET iteration-specific results handler
  - [ ] Sub-task: Implement POST recompute handler (enqueue async job, 202)
  - [ ] Sub-task: Wire OpenTelemetry spans on each handler
  - [ ] Sub-task: Register routes in stitchd-server router
  - [ ] Sub-task: Pass all integration tests

- [ ] Task 3: `analysis_type` mutation guard
  <!-- files: crates/stitchd-server/src/experimentation/ -->
  - [ ] Sub-task: Write failing test: PATCH analysis_type while running → 409
  - [ ] Sub-task: Add `analysis_type` to experiment PATCH handler mutation guard
  - [ ] Sub-task: Pass test

- [ ] Task: Conductor - User Manual Verification 'Phase 5: REST API Layer' (Protocol in workflow.md)

## Phase 6: Coverage & Quality Gate
<!-- depends: phase5 -->

- [ ] Task 1: Full lifecycle integration tests
  <!-- files: crates/stitchd-server/tests/ -->
  - [ ] Sub-task: Frequentist — controlled data set → significant result confirmed
  - [ ] Sub-task: Bayesian — controlled data set → P(best) > 0.95 for clear winner
  - [ ] Sub-task: Inconclusive case: close metrics → recommendation = inconclusive
  - [ ] Sub-task: `needs_more_data`: sample_size < min_sample_size guardrail
  - [ ] Sub-task: Per-iteration isolation: iteration 1 results ≠ iteration 2 results
  - [ ] Sub-task: Recompute endpoint → 202; subsequent GET returns fresh data
  - [ ] Sub-task: Funnel metric: correct chained conversion rate per variant

- [ ] Task 2: Coverage verification
  - [ ] Sub-task: Run cargo-tarpaulin on stitchd-core (stats modules),
        stitchd-db (results repo), stitchd-server (results API handlers)
  - [ ] Sub-task: Achieve ≥ 90% coverage on new code; add missing tests until met

- [ ] Task: Conductor - User Manual Verification 'Phase 6: Coverage & Quality Gate' (Protocol in workflow.md)
