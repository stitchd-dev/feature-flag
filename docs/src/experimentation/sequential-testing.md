# Sequential Testing (Always-Valid Inference)

Stitchd supports **always-valid inference** so a running experiment can be **peeked at any time**
— including continuously on the Results dashboard — without inflating the false-positive rate that
repeated fixed-horizon significance tests suffer from.

Two complementary, dual constructions are computed over a single normal-mixture core:

1. **mSPRT always-valid p-values** — a mixture Sequential Probability Ratio Test. Each
   treatment-vs-control comparison yields a p-value process that is valid under *any* stopping rule:
   `P( inf_t p_t ≤ α ) ≤ α` under H₀. The reported value is the **running minimum** of `1/Λ_t`, so it
   is monotone non-increasing across looks.
2. **Confidence sequences** — the mSPRT-dual anytime-valid confidence interval from the *same*
   mixture, valid uniformly over time (peek any time; the interval still covers the true effect at
   `1 − α`).

Every metric family reduces to the same shape — an asymptotically-normal effect estimate `δ̂` and its
standard error — and runs through one engine (`stitchd-core::experimentation::stats::sequential`):

| Family            | Effect estimate                          |
|-------------------|------------------------------------------|
| conversion/count  | Bernoulli proportion difference          |
| continuous        | mean difference                          |
| funnel            | final-step conversion-rate difference    |
| ratio             | delta-method on numerator / denominator  |

## Per-experiment configuration (opt-in)

Sequential testing is **opt-in per experiment** and **off by default** — existing experiments are
unchanged. Configured in the create/edit experiment form (snapshotted onto each iteration like
`pre_period_days`):

| Field                        | Default | Meaning                                                        |
|------------------------------|---------|----------------------------------------------------------------|
| `sequential_testing_enabled` | `false` | Master opt-in toggle.                                           |
| `sequential_alpha`           | `0.05`  | Significance level α (0 < α < 1).                              |
| `sequential_tau_squared`     | *auto*  | mSPRT mixing variance τ². Blank → unit-information default.     |
| `sequential_min_sample_size` | `100`   | Minimum samples before a look is considered (else insufficient).|

When `sequential_tau_squared` is left blank the stats service derives a unit-information default
(pooled effect-scale variance), so most users only flip the toggle.

## Looks ride the scheduled tick

Always-valid p-values are valid under *any* look schedule, including the coarse 60-minute stats tick.
The dashboard value is therefore safe to read between ticks. The always-valid p-value is a **running
minimum persisted across ticks** (each tick reads the prior value and writes `min(prev, 1/Λ_now)`), so
it never increases over an experiment's life.

## Surfacing & the "safe to stop" decision

Results are stored per `(metric, context_type)` as a `sequential_result` JSON blob keyed by variant
(mirroring the existing `frequentist_result` / `bayesian_result` blobs), surfaced through the read path
as per-variant fields on `VariantResult` and over REST as `sequential_p_value`, `sequential_ci_lower`,
`sequential_ci_upper`, `sequential_crossed`, `sequential_insufficient_data`, `sequential_method`.

The Admin UI Results tab adds a **Sequential** view (shown when sequential data is present) with
**Always-valid p** and **Anytime CI** columns, plus a **"✓ Safe to stop"** badge on any variant whose
always-valid p-value has crossed α *in the metric's goal direction*. The decision is advisory — it does
not auto-stop the experiment or change rollout.

## Statistical guarantee (validated by simulation)

The core ships Monte-Carlo tests demonstrating the guarantee: under H₀, continuously peeking at the
running-minimum always-valid p rejects at ≤ α (measured ≈1.3% at α=0.05 over 20 looks), whereas a naive
per-look z-test under the *same* peeking inflates to ≈25% — exactly the error the method fixes. Power
under a real effect and uniform CS coverage are likewise tested.

## Activation note

The always-valid statistics are produced by the scheduled per-metric stats pass — the same pass that
computes the Frequentist / Bayesian / CUPED / SRM results. Sequential inference is wired at that pass's
`build_metric_summaries` seam and activates alongside the other per-metric statistics. (The end-to-end
scheduled per-metric compute orchestration is tracked separately; sequential testing adds no new
dependency beyond it.)

## Out of scope

- **Group-sequential / alpha-spending** (O'Brien–Fleming / Pocock) — the implemented approach is
  fully-sequential mSPRT + confidence sequences.
- **Automatic auto-stop / auto-ship** — "safe to stop" is advisory only.
- **Per-event (streaming) recomputation** — looks ride the existing 60-minute tick.
- **Always-valid extensions to the N-way interaction sweep** — interaction terms keep their
  fixed-horizon + FDR treatment.
