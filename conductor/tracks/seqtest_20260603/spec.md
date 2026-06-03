# Spec: Sequential Testing (Always-Valid Inference)

Track ID: seqtest_20260603
Type: Feature

## Overview

Add **always-valid inference** to the experimentation stats engine so experiments can be
**peeked at any time** — including continuously on the Results dashboard — without inflating
the false-positive rate the way repeated fixed-horizon significance tests do.

Two complementary, dual constructions are implemented over a single normal-mixture core:

1. **mSPRT always-valid p-values** — a mixture Sequential Probability Ratio Test (Johari–Pekelis–Walsh
   "Always Valid Inference"). Each treatment-vs-control comparison yields a p-value sequence that is
   valid under *any* (even data-dependent) stopping rule: `P(inf_t p_t ≤ α) ≤ α` under H₀.
2. **Confidence sequences** — the mSPRT-dual anytime-valid confidence interval from the *same* mixture,
   so the effect-size bounds are valid uniformly over time (peek any time, the CI still covers at 1−α).

Both reduce every metric family to the same shape — an asymptotically-normal effect estimate `δ̂` plus its
standard error `se` — and run one uniform normal-mixture engine on `(δ̂, se, n)`. This mirrors how the
existing engine and the N-way interaction track already reduce ratio/funnel metrics via the delta method.

This is a backward-compatible extension of the mature Frequentist + Bayesian engine: it adds a third,
**opt-in** inference mode alongside the existing fixed-horizon results — it does not change or replace them.
`sequential testing` is already listed as a planned item in `product.md`.

## Functional Requirements

### FR1 — Pure sequential-stats core (`stitchd-core`)
- New module `stitchd-core::experimentation::stats::sequential` (pure, no I/O), reusing the existing
  shared primitives (`norm_cdf`/`erf` from `frequentist.rs`, delta-method patterns from `interaction`).
- `always_valid_p(delta_hat, se, tau_sq, prev_p) -> f64` — mixture-SPRT statistic `Λ` under a
  N(0, τ²) mixing prior on the effect; returns the **running-minimum** always-valid p-value
  `min(prev_p, 1/Λ)`, clamped to `[0,1]`, seeded at `1.0`.
- `confidence_sequence(delta_hat, se, tau_sq, alpha) -> ConfidenceInterval` — closed-form mSPRT-dual
  anytime-valid CI for the effect, valid uniformly over time.
- Per-family adapters that produce `(δ̂, se)` for: **conversion/count** (Bernoulli proportion diff),
  **continuous** (mean diff), **ratio** (delta-method on numerator/denominator), **funnel**
  (final-step conversion-rate diff) — full parity with the four fixed-horizon families.
- A `SequentialResult { always_valid_p, p_crossed (bool), confidence_sequence, method, insufficient_data }`
  struct returned per (metric, variant, context-type) comparison.
- Multiplicity across >2 variants handled consistently with the existing engine (per-comparison α
  split / Bonferroni applied to always-valid p-values).

### FR2 — Compute integration (`stitchd-stats-service`)
- In the 60-min scheduler, when an iteration has `sequential_testing_enabled`, compute the sequential
  result per (metric, variant, context-type) from the **same cumulative ITT sufficient statistics**
  already fetched for the fixed-horizon path (no new ClickHouse query shape required; counts / sums /
  sums-of-squares / funnel conversions reused).
- **Running-minimum persistence:** read the prior tick's `always_valid_p` from `experiment_results`,
  feed it as `prev_p`, write back the new running-min so the p-value sequence is monotone non-increasing
  across ticks. (Always-valid p-values are valid under the coarse 60-min look grid, and reading the
  dashboard between ticks is safe because the displayed value is already always-valid at the last tick.)
- Compose with **CUPED** (run sequential on the CUPED-adjusted estimate when `pre_period_days > 0`)
  and respect the `sequential_min_sample_size` first-look gate (below it → `insufficient_data`, no
  significance claimed).

### FR3 — Storage + transport
- Add nullable sequential columns to the ClickHouse `experiment_results` table:
  `sequential_p_value`, `sequential_ci_lower`, `sequential_ci_upper`, `sequential_method`,
  `sequential_crossed` (Bool), `sequential_insufficient_data` (Bool). New migration in
  `crates/stitchd-event-writer/migrations/` (auto-applied on analytics-service boot).
- Extend `WriteExperimentResultsRequest` and the results read message (`VariantResult` /
  `ContextTypeResults` in `experiments/v1`) with backward-compatible optional sequential fields
  (additive proto, no renumbering).

### FR4 — Per-experiment configuration (opt-in + advanced knobs)
- New columns on `experiments` (and snapshotted onto `experiment_iterations` at iteration start, like
  `pre_period_days` / `unit_context_types`): `sequential_testing_enabled BOOLEAN DEFAULT FALSE`,
  `sequential_alpha NUMERIC DEFAULT 0.05`, `sequential_tau_squared NUMERIC` (mixing variance;
  sensible default derived from a default minimum-detectable-effect prior when null),
  `sequential_min_sample_size BIGINT DEFAULT 100`.
- Additive proto fields on `Experiment` / `ExperimentIteration`; full create/update/read plumbing
  through experimentation-service → gateway REST.

### FR5 — Admin UI surfacing (columns + safe-to-stop decision)
- **Create/Edit experiment form:** a "Sequential testing" section — an opt-in toggle plus advanced
  knobs (α, τ²/mixing prior, minimum sample before first look), off by default.
- **Results tab:** when sequential testing is on, render always-valid p-value and anytime-CI columns,
  and an explicit **"safe to stop"** decision badge/banner for any variant whose always-valid p-value
  crosses α (equivalently, whose confidence sequence excludes 0) in the goal direction. Surfaced
  alongside the existing Frequentist/Bayesian view toggle, per context type.

### FR6 — Documentation
- Update `product.md` (move sequential testing from "Future" to implemented; describe the model),
  `tech-stack.md` (sequential stats module + new columns/migration), and the experimentation
  mdBook/statistics docs. Keep `cargo xtask docs` idempotent.

## Non-Functional Requirements

- **Backward compatible:** experiments without sequential testing enabled are byte-for-byte unchanged;
  all new proto fields additive; new ClickHouse columns nullable; existing fixed-horizon results untouched.
- **Statistical correctness:** the always-valid p-value must control time-uniform Type-I error at α under
  H₀ across an arbitrary look schedule; the confidence sequence must achieve ≥1−α uniform coverage.
  Validated by Monte-Carlo simulation tests (peeking under H₀ does not exceed α; power under H₁).
- **No new heavy dependencies:** all math hand-rolled on `std` + existing helpers (consistent with the
  interaction engine's "no statrs" convention).
- **Performance:** sequential computation reuses the per-tick sufficient statistics already fetched;
  no additional ClickHouse round-trips beyond reading the prior `always_valid_p`.
- **Coverage:** ≥90% per crate (CI-enforced); pure-core math unit-tested incl. simulation harness.

## Acceptance Criteria

1. With sequential testing enabled, the Results tab shows an always-valid p-value, an anytime confidence
   interval, and a "safe to stop" badge that appears only when the boundary is crossed in the goal direction.
2. A Monte-Carlo test demonstrates that continuously peeking under H₀ rejects at ≤ α (e.g. ~5%), whereas
   the fixed-horizon p-value under the same peeking inflates well above α — proving the inflation is fixed.
3. The always-valid p-value is monotone non-increasing across recompute ticks (running-minimum persisted).
4. All four metric families (conversion/count, continuous, ratio, funnel) produce sequential results, each
   with a simulation-backed correctness test.
5. Sequential testing composes correctly with CUPED and per-context-type analysis; multi-variant
   experiments apply the multiplicity correction.
6. Experiments with sequential testing **off** are unchanged (regression test); proto/ClickHouse changes
   are additive; `cargo sqlx prepare --check`, `cargo xtask docs` idempotency, and the OpenAPI contract
   check all pass.
7. Full CI green: `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -D warnings`,
   `cargo test --workspace`, admin vitest, sqlx-check, docs idempotent, contract covered.

## Out of Scope

- **Group-sequential / alpha-spending** (O'Brien–Fleming / Pocock) — the chosen approach is fully-sequential
  mSPRT + confidence sequences; pre-planned-look designs are not implemented.
- **Automatic experiment auto-stop / auto-shipping** — the UI recommends "safe to stop"; it does not
  automatically halt the experiment or change flag rollout.
- **Per-event (streaming) recomputation** — looks remain on the existing 60-min tick (plus on-demand
  recompute RPC); no event-by-event sequential update loop.
- **Always-valid extensions to the N-way interaction sweep** — interaction terms keep their current
  fixed-horizon + FDR treatment; sequential applies to per-metric variant comparisons.
- **Bayesian sequential / always-valid Bayes factors** beyond the existing posterior reporting (the
  existing Bayesian view is already peek-robust and is left as-is).
