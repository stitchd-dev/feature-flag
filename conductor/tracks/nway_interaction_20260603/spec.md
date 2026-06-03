# Spec: N-Way (3-Way) Cross-Experiment Interaction Analysis
#       + Funnel/Ratio Metrics + Bayesian Interaction Modeling

**Track:** nway_interaction_20260603
**Type:** Feature (extends xexp_interaction_20260602)

## Overview

Extend the cross-experiment interaction engine along three axes, all on a single
unified code path:

1. **3-way analysis** — analyze three simultaneously-overlapping experiments at
   once (interaction order capped at 3), in addition to the existing pairwise.
2. **Full metric coverage** — add **funnel** and **ratio** metrics to interaction
   analysis (previously aggregation/conversion + continuous only).
3. **Bayesian modeling** — add Bayesian interaction posteriors alongside the
   existing Frequentist tests, mirroring the platform's experiment-level
   Frequentist+Bayesian duality.

Storage, proto/REST, and Admin UI contracts are generalized from the hardcoded
`experiment_id_a`/`experiment_id_b` pairwise shape to a unified ordered-array
representation, so pairwise and 3-way share one write/read path. For each candidate
tuple the analysis reports a **full hierarchical decomposition** (highest-order term
+ all constituent lower-order interaction terms + main effects), each term carrying
**both** a Frequentist result and a Bayesian posterior.

Builds directly on `xexp_interaction_20260602`; the in-`stitchd-core` math is
reframed from 2D grids to general contingency tensors / multi-factor models.

Transport topology (unchanged actors, generalized contract):
Writer = `stitchd-stats-service` · Reader/RPC (`GetExperimentInteractions`) =
`stitchd-experimentation-service` · Math = `stitchd-core` · REST = `stitchd-gateway`
· UI = `admin/`.

## Functional Requirements

### FR1 — Unified data model (supersede pairwise schema)
- Replace `experiment_interactions` columns `experiment_id_a`/`experiment_id_b`
  with `experiment_ids Array(UUID)` (sorted) + `interaction_order UInt8`.
- Add a `term` discriminator (`main:<exp>`, `2way:<a>x<b>`, `3way:<a>x<b>x<c>`).
- Generalize `cell_stats` to an N-dimensional cell list keyed by the variant tuple.
- Add Bayesian output columns (see FR6) alongside the existing Frequentist columns.
- Pairwise computation re-expressed onto this table (order = 2): one write path,
  one read path. No historical backfill.

### FR2 — Candidate tuple enumeration
- Order 2: reuse existing pairwise enumeration unchanged.
- Order 3: enumerate triples where every constituent pair satisfies `can_interact`
  (distinct flags, overlapping windows, shared metric, not same exclusion group —
  if any two of the three co-share an exclusion group the triple is excluded).

### FR3 — k-way self-join over `experiment_assignments`
- Extend the self-join to k aliases on `(env_id, context_type, context_key)`.
- ITT lower bound generalized to `greatest(a.assigned_at, …, k.assigned_at)`,
  preserving platform-wide first-exposure attribution.

### FR4 — Metric coverage: aggregation, continuous, FUNNEL, RATIO
- **Aggregation/conversion (binary):** unchanged cell model (n, successes).
- **Continuous (revenue/duration/numeric):** unchanged (n, sum, sum-of-squares).
- **Funnel (NEW):** per `(dedup_key)` the unit either reaches the final step within
  `window_seconds` or not → modeled as a Bernoulli outcome per cell, analyzed with
  the **binary** interaction path (the windowFunnel evaluation feeds reached/total
  into each variant-tuple cell).
- **Ratio (NEW):** each cell carries numerator and denominator aggregate sums; the
  cell statistic is the ratio, with variance via the **delta method**. Cells below
  the metric's `min_denominator` are flagged `insufficient_data` (matching ratio's
  null-bucket semantics). Interaction contrast computed on the cell ratios.

### FR5 — Hierarchical Frequentist decomposition (stitchd-core)
- **Binary path (incl. funnel):** log-linear model over the R×C(×D) contingency
  table → highest-order interaction term + all lower-order interaction terms +
  main effects (chi-square + correct df per term).
- **Continuous path:** multi-factor ANOVA decomposition → main effects, all 2-way
  interaction terms, and (order 3) the 3-way term (F-test per term).
- **Ratio path:** delta-method interaction contrast per term.
- Each term emits the existing Frequentist `InteractionResult`
  (`estimate`, `statistic`, `p_value`, `df`, `significant`, `insufficient_data`).
- `insufficient_data` rules generalized to k dimensions (empty participating cell,
  degenerate variance, non-positive residual df, denominator below `min_denominator`);
  0.0 sentinels persisted as in pairwise, never shown significant.

### FR6 — Bayesian interaction modeling (stitchd-core)
- **Binary/funnel:** Beta-Binomial cell posteriors; posterior over the interaction
  contrast (e.g. difference-in-differences of cell rates) via Monte-Carlo /
  conjugate sampling. Report `prob_interaction` (posterior probability the
  interaction effect ≠ 0 / exceeds a ROPE), `expected_interaction` (posterior mean
  effect), and a credible interval.
- **Continuous/ratio:** Normal-Normal cell posteriors on cell means; posterior over
  the interaction contrast; same reported quantities.
- Reuses the existing experiment-level Bayesian primitives (Beta-Binomial /
  Normal-Normal) where possible; results attach to each term row.
- Bayesian outputs are not subject to FDR (Bayesian inference handles multiplicity
  via the model); they surface independently of the Frequentist significance flag.

### FR7 — FDR correction across the full Frequentist term family
- One Benjamini–Hochberg (FDR 0.05) step-up across all non-insufficient Frequentist
  p-values in the sweep (all orders, all decomposed terms). Insufficient-data terms
  excluded; remain non-significant.

### FR8 — Transport + API
- Generalize the interactions DTO (proto `ExperimentInteraction` message + REST
  response + `ExperimentInteraction` TS type): array of participating experiments
  (ids+names), `interaction_order`, `term`, the Frequentist fields, and the new
  Bayesian fields. Additive proto field numbering — keep wire back-compat.
- `GET /v1/environments/{envId}/experiments/{experimentId}/interactions` returns
  every term row in which the focused experiment participates (orders 2 and 3),
  via the `GetExperimentInteractions` RPC on the experimentation-service.

### FR9 — Admin UI surfacing
- Interactions tab renders pairwise and 3-way rows: participating experiment(s),
  order, term, metric, shared count, Frequentist estimate + p-value + significance
  badge, and Bayesian prob/expected-effect (+ credible interval).
- Funnel/ratio interaction rows render with appropriate value formatting.
- Results-tab warning banner fires when any term (any order) the experiment
  participates in is significant (Frequentist) and not insufficient-data; Bayesian
  high-probability interactions also contribute to the banner.

## Non-Functional Requirements
- **Bounded sweep:** order capped at 3; triples gated by FR2 validity; sweep cost
  and any skipped tuples `tracing`-logged (no silent truncation).
- **Attribution parity:** k-way ITT bound matches the platform first-exposure model.
- **Recompute path:** existing 60-min stats tick (on-demand `TriggerRecompute`
  wiring tracked separately as `feature-flag-uga`).
- **Coverage:** ≥90% per crate (CI-enforced); pure stats validated against
  hand-computed fixtures.

## Acceptance Criteria
- [ ] Unified schema (array + order + term + N-D cell_stats + Bayesian columns)
      replaces a/b; pairwise results identical through the new path (regression).
- [ ] Valid overlapping triples enumerated; same-group / non-overlapping excluded.
- [ ] Aggregation, continuous, funnel, and ratio metrics all analyzed; funnel via
      binary path, ratio via delta method with `min_denominator` gating.
- [ ] Frequentist hierarchical decomposition (log-linear + multi-factor ANOVA +
      ratio delta) emits full term sets with correct df, validated against
      hand-computed fixtures.
- [ ] Bayesian posteriors (Beta-Binomial / Normal-Normal) emit
      prob/expected-effect/credible-interval per term, validated against fixtures.
- [ ] BH-FDR applied once across the combined Frequentist term family; Bayesian
      outputs independent of FDR.
- [ ] `insufficient_data` flags sparse k-cells and sub-`min_denominator` ratio
      cells; sentinels never render as significant.
- [ ] REST/DTO + Admin UI render pairwise and 3-way rows with both Frequentist and
      Bayesian columns; warning banner reflects significant/high-prob interactions.
- [ ] CI green: Rust tests + clippy + sqlx check, admin vitest, docs idempotent,
      contract covered, `cargo fmt --all --check`.

## Out of Scope
- Interaction order ≥ 4 (arbitrary N).
- On-demand interaction recompute wiring (`feature-flag-uga`, tracked separately).
- Historical backfill of pre-existing pairwise rows into the new schema.
