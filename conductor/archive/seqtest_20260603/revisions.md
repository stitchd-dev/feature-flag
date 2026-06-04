# Revisions — seqtest_20260603

## Revision 1 — 2026-06-03 — Plan premise (Phase 3 "Compute Integration")

**Type:** Plan/spec premise correction (scope clarification).

**Trigger (discovered during Phase 3, W3):** The spec/plan framed sequential testing as a "natural
extension of the mature Frequentist+Bayesian stats engine" and Phase 3 as wiring sequential into the
60-min scheduler "reusing the cumulative ITT sufficient statistics already fetched for the fixed-horizon
path." Investigation (workspace-wide grep + reading `stitchd-stats-service/src/main.rs:178`,
`scheduler.rs`, `results_writer.rs`) shows there is **no live per-metric compute pass**: the scheduler is
an explicit scaffold (`// Stats computation is deferred to Phase 3 full implementation`) that calls
`write_results(experiment, &[])` with EMPTY summaries, and **no service anywhere calls the stitchd-core
stats functions** (`frequentist::analyze_*`, `bayesian::*`, `cuped`, `srm`). The math engine in
`stitchd-core` is complete + tested; the orchestration that would execute the metric queries, build
`VariantStats`, run the analyses, and persist non-empty `experiment_results` rows was never wired (it
predates this track). The interaction sweep has its own separate live compute path; the base per-metric
results compute does not.

**Decision (scope boundary):** Deliver sequential testing **at full parity with the existing stats** —
i.e. complete and tested at every layer this track owns:
- Pure core (`stitchd-core::…::stats::sequential`) ✓ (Phase 1)
- Storage + transport (`experiment_results.sequential_result` JSON blob, proto) ✓ (Phase 2 + Phase 3 reconcile)
- Config (opt-in + α/τ²/min-sample, snapshotted) ✓ (Phase 2)
- Compute module + running-min + config flow (`sequential_compute.rs`), wired into the SAME
  `build_metric_summaries` seam frequentist/bayesian use (Phase 3) ✓ — activates the instant a real
  compute pass populates `MetricSummary`.
- Read path → VariantResult + safe-to-stop (Phase 4) and Admin UI (Phase 5).

Building the deferred end-to-end per-metric compute orchestration (execute queries → VariantStats →
frequentist + bayesian + sequential + CUPED + SRM across all four families) is a **separate, larger
effort outside this track's "add sequential testing" spec** and would equally be required for the
existing frequentist/bayesian to produce live values. It is filed as a Beads follow-up (see
`feature-flag-*` "Wire live per-metric stats compute pass"). No spec.md/plan.md rewrite needed — the
deliverable (sequential capability across the engine) stands; only the "reuse the existing fixed-horizon
compute" assumption was inaccurate, and sequential is wired at the correct seam regardless.

**Also (Phase 3, W3):** ratio sequential delta-method sufficient-stats aggregation in the compute pass is
filed as `feature-flag-2lh` (the adapter + storage are ready; only the query-side sum aggregation is
deferred with the rest of the compute pass).
