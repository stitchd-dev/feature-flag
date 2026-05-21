# Spec: Experimentation — Complete UI + Backend with Eval-Log-Based Attribution

## Overview

Deliver the experimentation surface end-to-end: full Admin UI (Results, SRM, time-series, iteration history, manual recompute) wired to real backend data, and shift metric attribution to **first-exposure (intent-to-treat), rule-scoped, from `flag_evaluation_log_v2`** so the SDK no longer needs to know about experiments. Add hard guardrails so a running experiment locks its bound flag, restrict experiments to percentage-distribution rule paths only (including a percentage-distribution default-rule path), and compute results at the **context-type level** so an experiment can target one or more context types and the analysis is broken down per context type.

This track closes the loop opened by `experimentation_20260419` (CRUD), `stats_20260420` (Frequentist/Bayesian/CUPED math), and `events_metrics_20260519` (metric primitives) — moving from SDK-tagged event attribution to server-side derivation.

## Functional Requirements

### 1. Attribution Pipeline (Eval-Log → Assignment) — Rule-Scoped, Context-Type-Scoped, First-Exposure

- **Source of truth:** `flag_evaluation_log_v2`. Only evaluations where the experiment's bound rule actually matched AND whose `context_type` is in the experiment's `unit_context_types` count as exposure. The first such eval for `(env_id, experiment_id, iteration_id, context_type, context_key)` IS that context's assignment for the iteration.
- **Eval-log schema extension:** Add `matched_rule_id Nullable(UUID)` to `flag_evaluation_log_v2` (new CH migration). Populated by `stitchd-flag-service::eval_log_writer`:
  - Set to the rule UUID when a custom rule matched.
  - `NULL` when the flag fell through to the default-rule path (flag enabled, targeting on, no custom rule matched).
  - Omitted (and not written) when flag was disabled — disabled-flag evals never produce experiment exposures.
- **Default-rule support:** Experiments may bind to either:
  - A specific `flag_rule_id` (existing case), or
  - The flag's default-rule path, encoded as `flag_rule_id = NULL` + a new boolean `targets_default_rule = true` on the experiment row.

  When `targets_default_rule = true`, the flag's default-rule path must be a percentage distribution (see §2). Persisted as a JSONB `default_rule_distribution Jsonb` on `feature_flags` (new column; default `NULL` preserves today's single-variant fallthrough behavior for non-experiment flags).
- **Materialized view:** New CH MV `experiment_assignments_mv` watches `flag_evaluation_log_v2` and writes into the existing `experiment_assignments` table (`ReplacingMergeTree` keyed on `(experiment_id, context_type, context_key)`).
  - MV row shape: `(experiment_id, iteration_id, flag_id, env_id, context_type, context_key, variant_key, assigned_at, matched_rule_id)`.
  - MV body JOINs the eval row against an `experiment_iterations_active` CH dictionary (refreshed from PG on iteration start/stop) keyed on `(env_id, flag_id, matched_rule_id, context_type)` — `NULL` rule_id maps to the default-rule-bound experiment for that flag; `context_type` is filtered against `unit_context_types`.
  - Uses `argMin(variant_key, evaluated_at)` semantics — first matching eval wins per `(experiment_id, iteration_id, context_type, context_key)`.
- **Backfill migration:** One-shot `INSERT INTO experiment_assignments SELECT ... FROM flag_evaluation_log_v2 JOIN experiment_iterations_active ...` to populate from the last 90 days of history. Backfill is skipped for rows where `matched_rule_id` is absent (pre-migration rows) — those contexts get attributed only from go-forward evals.
- **Stats query cutover:** `crates/stitchd-stats-service/src/queries/{aggregation,ratio,funnel,preview}.rs` STOP filtering on event-side `experiment`/`iteration`/`variant` context tuples. They now `JOIN events_v2 e ON experiment_assignments a USING (env_id, context_type, context_key)` (joining `a.context_type, a.context_key` against `arrayExists(t -> t.1 = a.context_type AND t.2 = a.context_key, e.contexts)`) and filter on `a.experiment_id = ? AND a.iteration_id = ? AND e.occurred_at >= a.assigned_at AND e.occurred_at < COALESCE(iteration.ended_at, now())`. Results GROUP BY `(a.context_type, a.variant_key)`.
- **Pre-exposure events excluded:** `e.occurred_at >= a.assigned_at` enforces strict ITT — events fired before the context's first matching eval do NOT count.
- **Re-exposure does not reassign:** Once `experiment_assignments` has a row for `(experiment_id, context_type, context_key)`, subsequent evals (even with a different variant) do not overwrite it within the iteration.

### 2. Experiment Constraints (Rule + Flag Lock + Context Types)

- **Rule kind constraint:** Experiment create/update validates that:
  - If `flag_rule_id` is set, the bound rule's action must be a percentage-distribution rollout over the flag's variants. Specific-variant rules and segment-only rules are rejected with HTTP 422 `INVALID_RULE_KIND`.
  - If `targets_default_rule = true`, the flag must have `default_rule_distribution` set to a percentage distribution. If absent, reject with HTTP 422 `INVALID_DEFAULT_RULE_KIND`.
  - Exactly one of `flag_rule_id` or `targets_default_rule = true` must be set per experiment (XOR constraint, enforced via PG `CHECK`).
- **Context-type binding:** Experiment requires `unit_context_types text[] NOT NULL` with at least one entry. Each must be a known context type for the environment (validated against `context_type_registry`). Default `{user}`. Snapshotted into `experiment_iterations` at iteration start so changes between iterations are captured. Empty array → HTTP 422 `EMPTY_UNIT_CONTEXT_TYPES`; unknown type → HTTP 422 `UNKNOWN_CONTEXT_TYPE`.
- **Whole-flag freeze:** While the experiment is `running` or `paused`:
  - Any PATCH/PUT/DELETE on the flag (`/v1/environments/{env}/flags/{key}`), its variants, OR any of its rules (including the default-rule distribution) returns HTTP 409 `FLAG_LOCKED_BY_EXPERIMENT` with the experiment ID in the error body.
  - Replaces today's per-rule `frozen` flag with a flag-level lock derived from `EXISTS(experiment WHERE flag_id = ? AND status IN ('running','paused'))`.
  - Locked endpoints: flag update, variant CRUD, rule CRUD, default-rule distribution update, flag enable/disable, flag archive.
  - Flag GET + evaluate continue to work normally.
- **Restart-with-changes flow:** User stops experiment → modifies flag → restarts experiment → new iteration created with snapshot of new config (including any default-rule-distribution + `unit_context_types` changes).

### 3. Statistical Analysis (Full Set, Per-Context-Type)

Implemented in `stitchd-stats-service` and surfaced via `GET /v1/environments/{env}/experiments/{id}/results`. All analyses are computed independently per context type.

- **Frequentist:** Welch's t-test (continuous), two-proportion Z-test (conversion/count), 95% CI, Bonferroni correction when variants > 2.
- **Bayesian:** Beta-Binomial for proportion metrics, Normal-Normal for continuous; outputs posterior mean, 95% credible interval, probability-to-beat-control (PtB), expected lift.
- **CUPED:** Optional per-experiment `pre_period_days` setting (default 0 = off). When > 0, pre-period mean per context is the covariate, applied per metric. Pre-period events fetched from `events_v2` scoped to `unit_context_types`.
- **SRM detection:** Chi-square test of observed vs expected variant counts per context type (expected from `traffic_allocation` × percentage-distribution weights from the bound rule or default-rule distribution). p-value surfaced; `< 0.001` flags red.
- **Guardrail metrics:** New field `guardrail_metric_ids UUID[]` on `experiments`. Computed identically to primary metrics but UI flags direction violations with a warning badge.

### 4. Admin UI — Experiment Detail (`admin/src/pages/experiments/`)

Remove all `EXPERIMENTS` mockData usage from `ExperimentDetail.tsx`. Replace mock viz components with real-data versions reading from `/v1/environments/{env}/experiments/{id}/results`.

- **Context-type switcher:** Tab strip (or dropdown when ≥ 4) under the page header — one tab per `unit_context_types`. Active tab persists per-experiment in localStorage. All sub-tabs render scoped to the active context type. Exposure header reads "Exposures (`<context_type>`)" so the scope is unmissable.
- **Results tab:**
  - Per-metric card: variant rows with mean ± CI, p-value, PtB, expected lift; winner highlight via `goal_direction`.
  - Frequentist/Bayesian view toggle (persisted per-user in localStorage).
  - Multi-variant matrix when `variants > 2` (pair-wise comparisons).
- **Exposure / SRM panel** (new, above metrics):
  - Per-variant: assigned count, expected count, % deviation, χ² p-value, health pill (green/yellow/red).
  - Total exposures, unique contexts, iteration window dates.
  - "Bound to: <rule name>" or "Bound to: Default rule" badge.
- **Daily time-series tab:**
  - Per-metric per-variant daily series via metric-preview pipeline scoped to assigned contexts AND the active context type.
  - Sparkline in list view; click metric → expanded chart with hover tooltips.
- **Iteration history tab:**
  - Table of past iterations (number, start, end, duration, snapshot summary including `unit_context_types`).
  - Click row → past-iteration results (read-only).
  - "Recompute now" button → calls `TriggerRecompute` RPC, polls job status, shows toast on completion.
- **Create / Edit experiment modal:**
  - Rule picker shows: percentage-distribution rules + a "Default rule (fallthrough)" entry when the flag has a `default_rule_distribution`. Non-eligible rules disabled with tooltip "Experiments require a percentage rollout rule."
  - If no eligible target exists, surface a "Configure default-rule distribution on the flag first" CTA linking to the flag editor.
  - `unit_context_types` multi-select (sourced from `context_type_registry`); default `[user]`.
  - Primary metric_ids + guardrail metric_ids (separate Formik FieldArrays).
  - Optional `pre_period_days` (CUPED).
- **Experiments list:**
  - Status badge, primary metric name, exposed-context count (live), days remaining.
  - Filter by status + flag.
- **Flag editor enhancement:** Add a "Default rule" section where the default fallthrough can be configured as either a single variant (today's behavior, default) or a percentage distribution (new, required for default-rule-bound experiments). Locked while any experiment on this flag is running/paused.

### 5. API Surface (Gateway)

- `GET    /v1/environments/{env}/experiments/{id}/results` — already exists, response shape extended with:
  ```json
  {
    "results_by_context_type": {
      "user":    { "variants": [...], "srm": {...}, "guardrails": [...] },
      "account": { "variants": [...], "srm": {...}, "guardrails": [...] }
    },
    "bound_target": { "kind": "rule"|"default_rule", "rule_id": "<uuid|null>", "label": "..." },
    "pre_period_days": 0
  }
  ```
- `GET    /v1/environments/{env}/experiments/{id}/exposures?context_type=user` — paginated list of assignments (context_type, context_key, variant, assigned_at, matched_rule_id). `context_type` query param required.
- `GET    /v1/environments/{env}/experiments/{id}/timeseries?metric_id=...&context_type=user&days=N` — daily per-variant series; `context_type` required.
- `POST   /v1/environments/{env}/experiments/{id}/recompute` — triggers `TriggerRecompute` gRPC, returns `{ job_id, status }`.
- `GET    /v1/environments/{env}/experiments/{id}/recompute/{job_id}` — job status.
- `POST   /v1/environments/{env}/flags/{key}/default-rule-distribution` — set/update the flag's default-rule distribution (subject to flag-lock).
- All flag/variant/rule mutation endpoints add `409 FLAG_LOCKED_BY_EXPERIMENT` to OpenAPI responses.

## Non-Functional Requirements

- ClickHouse MV must keep up at ≥ 5k evals/sec sustained (validated with a load test); per-row cost capped by an internal limit on `experiment_iterations_active` dictionary cardinality.
- Optimistic locking on `experiments` (existing `version` column).
- Audit log on every experiment mutation + transition, plus default-rule-distribution updates.
- OpenTelemetry spans on results, exposures, timeseries, recompute, and eval-log-write (now emits `matched_rule_id`) paths.
- utoipa OpenAPI annotations on all new routes; contract-check job remains green.
- Coverage ≥ 90% on new code (per-crate tarpaulin).
- All admin UI forms use Formik + Yup (pattern from `boundaries_20260518`).
- TypeScript strict mode; no `any` casts.

## Acceptance Criteria

- [ ] `flag_evaluation_log_v2` adds `matched_rule_id Nullable(UUID)`; flag-service writes it on every eval (NULL for default-rule path, omitted for disabled flags).
- [ ] `feature_flags` adds `default_rule_distribution Jsonb`; `experiments` adds `targets_default_rule Boolean` + `guardrail_metric_ids UUID[]` + `pre_period_days Integer` + `unit_context_types text[] NOT NULL DEFAULT '{user}'`; XOR CHECK constraint enforces `flag_rule_id` xor `targets_default_rule`.
- [ ] `experiment_assignments_mv` MV created; populated by `flag_evaluation_log_v2` writes filtered to rows where `matched_rule_id` matches the active experiment's bound rule (or both NULL for default-rule experiments) AND `context_type` is in the experiment's `unit_context_types`; backfill migration produces correct first-exposure rows for the last 90 days.
- [ ] Stats queries (`aggregation`, `ratio`, `funnel`, `preview`) no longer reference event-side `experiment`/`iteration`/`variant` context tuples; verified by grep + integration test asserting events without those tags still attribute correctly.
- [ ] Pre-exposure events (occurred_at < assigned_at) excluded from results (integration test).
- [ ] Re-exposures with a different variant do NOT reassign within the iteration (integration test).
- [ ] Evals matching a DIFFERENT rule on the same flag do NOT count as exposure for the experiment (integration test).
- [ ] Evals for `context_type ∉ unit_context_types` do NOT count as exposure (integration test).
- [ ] Experiment create returns 422 `INVALID_RULE_KIND` when the bound rule is not a percentage rollout, 422 `INVALID_DEFAULT_RULE_KIND` when `targets_default_rule = true` but the flag has no default-rule distribution, 422 `EMPTY_UNIT_CONTEXT_TYPES` for empty array, 422 `UNKNOWN_CONTEXT_TYPE` for unregistered context type.
- [ ] Flag PATCH / variant CRUD / rule CRUD / default-rule-distribution update return 409 `FLAG_LOCKED_BY_EXPERIMENT` while experiment running/paused, including the experiment ID in the error body.
- [ ] Stopping the experiment unlocks the flag; restart creates a new iteration with snapshot of new config (including any default-rule-distribution + `unit_context_types` changes).
- [ ] `GET /results` returns `results_by_context_type` keyed by unit context type, plus `srm`, `guardrails`, `pre_period_days`, `bound_target`; verified via OpenAPI schema test.
- [ ] Per-context-type stats: fixture with divergent results across two context types (e.g., user-level winner ≠ account-level winner) produces independent stats for each.
- [ ] CUPED reduces CI width by ≥ 15% on a seeded test fixture where a known pre-period covariate explains variance.
- [ ] Admin UI Experiment Detail uses zero `EXPERIMENTS` mock data; all four tabs render from API; context-type tab strip switches all sub-tabs; verified by vitest snapshot + manual run.
- [ ] SRM panel shows red pill when chi-sq p < 0.001 (Vitest test with fixture).
- [ ] Create modal disables non-eligible rules with tooltip; includes "Default rule" entry when flag has a default-rule distribution; `unit_context_types` multi-select sources from registry.
- [ ] Flag editor lets user configure default-rule distribution; locked when experiment running/paused.
- [ ] Recompute button triggers RPC and updates results within the iteration's recompute interval.
- [ ] Coverage ≥ 90% per crate touched (tarpaulin) and per new UI component.
- [ ] OpenAPI contract-check passes; mdBook docs updated for new endpoints + default-rule-distribution + per-context-type attribution concepts.

## Out of Scope

- Multi-armed bandit / Thompson sampling auto-allocation.
- Sequential testing (always-valid p-values, mSPRT, group sequential boundaries) — separate dedicated track.
- Holdout group / global experiment holdback / Google-style experiment layers.
- Warehouse-backed (offline) event ingestion.
- Client-side (browser/mobile) SDK experiment helpers — server-side SDK only; the eval-log-attribution model is deliberately SDK-agnostic so any future SDK gets attribution for free.
- Email/Slack alerting on SRM red or guardrail violation (UI surfaces only).
- Per-user feature flag overrides for QA / debug-mode experiment forcing — separate follow-up track (requires `is_override` column on eval log + override management UI).
- Cross-experiment interaction analysis (k×k interaction tables across concurrent experiments).
- Cross-context-type interaction analysis (variant effect at user level conditional on `account` cohort).
- Retroactive backfill of `matched_rule_id` for eval log rows written before the schema migration (those rows simply don't contribute to assignments — go-forward attribution only).
