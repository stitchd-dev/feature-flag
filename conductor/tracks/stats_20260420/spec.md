# Spec: Experiment Statistical Analysis

## Overview

Implement statistical analysis for experiments. Reads ClickHouse aggregations
produced by the events layer and computes per-iteration, per-variant results for
each registered metric. Supports Frequentist and Bayesian analysis models,
selectable per experiment. Exposes a results query API consumed by the Admin UI
and external tooling.

## Functional Requirements

### Analysis Model Selection
- `analysis_type` field added to `experiments` table: `frequentist | bayesian`
  (default: `frequentist`)
- Mutable only in `draft`, `paused`, or `stopped` states (same mutation guard as
  other experiment fields)

### Metric Types Supported
All four metric types are derived from ClickHouse materialized views:

- **Count / Conversion** — binary event presence per context; proportion test
- **Numeric (sum/avg)** — continuous metric (e.g. revenue, latency); mean test
- **Percentile** — p50/p95/p99 via bootstrap confidence interval
- **Funnel** — multi-step conversion: each step is a registered event key;
  overall funnel conversion rate per variant

### Frequentist Analysis
- **Count/Conversion:** Two-proportion z-test; p-value, 95% CI on lift
- **Numeric:** Welch's t-test; p-value, 95% CI on mean difference
- **Percentile:** Bootstrap percentile CI (1000 resamples); no p-value
- **Funnel:** Z-test on final-step conversion rate
- Output per metric: `p_value`, `confidence_interval: {lower, upper}`,
  `significant: bool` (α = 0.05)

### Bayesian Analysis
- **Count/Conversion:** Beta-Binomial posterior; P(variant > control)
- **Numeric:** Normal-Normal conjugate; P(variant > control), credible interval
- **Percentile:** Bootstrapped posterior approximation
- **Funnel:** Beta-Binomial on final-step conversion
- Output per metric: `prob_best: f64`, `credible_interval: {lower, upper}`,
  `expected_loss: f64`

### Per-Variant Summary Stats (all models)
For each variant and each metric:
- `sample_size: i64`
- `conversions: i64` (count metrics only)
- `mean: f64`, `variance: f64` (numeric / percentile metrics)
- `conversion_rate: f64` (count / funnel metrics)
- `percentiles: {p50, p95, p99}` (percentile metrics only)

### Recommendation Field
Computed after analysis per metric:
- `"variant_X_wins"` — single variant is statistically better (Frequentist: p <
  0.05; Bayesian: P(best) > 0.95)
- `"inconclusive"` — no variant meets the threshold
- `"needs_more_data"` — sample size below `min_sample_size` guardrail (if set)
- `"control_wins"` — control variant is the statistically best

### Per-Iteration Results
Results are computed and stored per experiment iteration (not just latest). The
API returns results for each iteration separately, enabling comparison across
restarts.

### Results Storage
- Computed results persisted to PostgreSQL (`experiment_results` table) to avoid
  recomputing on every API request
- Background job (or on-demand trigger via API) recomputes results from ClickHouse
- Results stamped with `computed_at`; stale if `computed_at` < iteration
  `started_at` + configurable window

### API Endpoints
- `GET  /v1/environments/{env_id}/experiments/{id}/results`
  — latest iteration results for all metrics
- `GET  /v1/environments/{env_id}/experiments/{id}/iterations/{iter}/results`
  — results for a specific iteration
- `POST /v1/environments/{env_id}/experiments/{id}/results/recompute`
  — trigger recomputation (async; returns 202)
- Auth: JWT (human) for all endpoints

## Non-Functional Requirements
- ClickHouse queries use existing `events_count_mv` and `events_numeric_mv`
  materialized views; no new ClickHouse schema changes required
- Statistical computation in-process (pure Rust, no Python subprocess)
- OpenTelemetry spans on analysis compute path
- utoipa annotations for OpenAPI generation
- Coverage ≥ 90% on new code

## Acceptance Criteria
- [ ] `analysis_type` field on experiment; mutation guard enforced
- [ ] Frequentist results computed correctly for all four metric types
- [ ] Bayesian results computed correctly for all four metric types
- [ ] Per-variant summary stats present for every result
- [ ] Recommendation field populated on all results
- [ ] Per-iteration results stored and queryable independently
- [ ] Recompute endpoint triggers async re-fetch from ClickHouse
- [ ] Integration tests: frequentist significance, bayesian P(best),
      inconclusive case, needs_more_data guardrail, per-iteration isolation
- [ ] Coverage ≥ 90% on new code

## Out of Scope
- CUPED variance reduction (deferred to follow-up track)
- Sequential testing / always-valid p-values
- Multi-metric correction (Bonferroni etc.)
- Warehouse-backed event ingestion
- Admin UI
- SDK direct event submission
