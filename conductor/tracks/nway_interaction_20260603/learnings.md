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

<!-- Learnings from implementation will be appended below -->
