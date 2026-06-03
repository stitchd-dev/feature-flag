# Track Learnings: nway_interaction_20260603

Patterns, gotchas, and context discovered during implementation.

## Codebase Patterns (Inherited)

See `conductor/patterns.md` for the full set. Seeded from the direct predecessor
`xexp_interaction_20260602` (pairwise interaction) — most relevant to this track:

- **Stats query builders are pure** (`queries::{aggregation,ratio,funnel,preview,
  interaction_metric}` → `BuiltQuery`), parameterized (no `format!()` SQL). The
  k-way self-join generalizes the same shape — extend, don't fork.
- **ClickHouse first-exposure** assignments live in `experiment_assignments`
  (ReplacingMergeTree, inverted `_version`); readers use `FINAL`/`argMin`. The
  interaction sweep self-joins this table on `(env_id, context_type, context_key)`;
  the k-way ITT bound is `greatest(a…k.assigned_at)`.
- **Pure stats live in `stitchd-core`** (`experimentation::stats::interaction`),
  non-async, no I/O — validated against hand-computed fixtures. The order-2 path
  MUST reproduce the legacy `binary_2x2`/`binary_rxc`/`continuous_interaction`
  outputs (regression gate) before the legacy fns are retired.
- **insufficient_data sentinels**: NaN in memory → `0.0` for ClickHouse Float64; the
  `insufficient_data Bool` column distinguishes real values from sentinels. Never
  show a sentinel as significant. Generalize this rule to k-D cells + sub-
  `min_denominator` ratio cells.
- **BH-FDR is applied once across the whole sweep's Frequentist family** — collect
  non-insufficient p-values, run the step-up, map decisions back in original order.
  The family now spans all orders + decomposed terms. Bayesian outputs are NOT in
  the FDR family.
- **Transport split**: stats-service WRITES `experiment_interactions`;
  experimentation-service READS it (`interactions_reader.rs`) and exposes the
  `GetExperimentInteractions` gRPC; gateway translates to REST; admin UI consumes.
  Generalize the proto message ADDITIVELY (keep wire back-compat).
- **Parallel waves:** isolated worktrees, file-ownership table per worker prompt,
  repo-side worker owns shared traits/seams, plain `bd close <id>` (`--no-auto` is
  unreliable per workflow.md beads-close gotcha). Phase 2's five stats submodules
  are disjoint files — ideal wave; reconcile on `interaction/mod.rs` only.
- **CI gotcha:** the final gate MUST include `cargo fmt --all --check` (a worker-wave
  on the predecessor skipped it and formatting drift reached main). On a cold target
  after a big merge, run `cargo clean -p stitchd-proto` before `sqlx prepare --check`
  to avoid phantom "no field on proto type" errors.

---

## [2026-06-03] Code-review fixes (max-effort review, 15 findings)

A max-effort multi-agent review (9 finder angles + verify + sweep) of the merged
track found 9 correctness issues + 6 cleanup. Fixed all correctness + low-risk
cleanup; deferred 2 behavior-neutral perf/dedup refactors (`feature-flag-ef5`).

- **ReplacingMergeTree dedup key must include the FULL natural key.** The
  `experiment_interactions` ORDER BY omitted `experiment_ids`, so a `main:X` /
  `2way:AxB` term emitted by multiple same-order tuples collided and one was
  silently lost under FINAL. Added `experiment_ids` to the sort key. The unit
  tests (in-memory fake writer) were blind to this — only a live-CH round-trip
  with ≥3 overlapping experiments catches it. New test added.
- **Interaction query builders MUST be derived from the canonical
  `aggregation.rs`/`ratio.rs`/`funnel.rs`** — they had diverged and dropped the
  metric `where_clause`, fanned out the ratio via a double LEFT JOIN (fix:
  pre-aggregate each leg to one value per context before a 1:1 join), and put an
  inequality in a funnel JOIN ON (ClickHouse rejects; fix: fold the ITT bound into
  each windowFunnel step). All three were uncovered by tests (single-pair agg-only
  integration test).
- **Don't let "context" rows pollute a corrected family.** Main-effect terms are
  single-experiment quantities; including their (tiny) p-values in the interaction
  Benjamini–Hochberg family shifted the step-up threshold, and counting them in the
  UI "interaction detected" banner fired it on every well-powered experiment.
  Fix: exclude `main:` terms from FDR + the banner; never mark them significant.
- **An uncorrected directional Bayesian prob (`Φ(|E|/sd)`, ≈0.5 under the null)
  must not drive a page-level warning** — it fires on noise and contradicts the
  FDR'd per-row badge. Show it per-row; gate the banner on the corrected
  frequentist flag.
- **Full ANOVA decomposition shares ONE pooled error term across all terms.**
  Delegating the 2-way to the legacy pairwise fn used a different (collapsed-grid)
  error than the main/3-way terms. The single-common-error refactor also removed
  ~13 redundant SS passes — and still reproduces the legacy fn bit-for-bit at
  order 2 (where the collapsed grid IS the full table).

<!-- Learnings from implementation will be appended below -->
