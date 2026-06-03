# Track Learnings: seqtest_20260603

Patterns, gotchas, and context discovered during implementation.

## Codebase Patterns (Inherited)

Seeded from `conductor/patterns.md` (20 pattern sections) + closely-related archived tracks
(`stats_20260420`, `scheduled_stats_20260423`, `experimentation_full_20260521`,
`xexp_interaction_20260602`, `nway_interaction_20260603`). Most relevant for sequential testing:

- **Experimentation Patterns** — first-exposure ITT attribution via `experiment_assignments`
  (ReplacingMergeTree, reader `FINAL`); stats read pre-computed `experiment_results`; per-context-type
  analysis is computed independently and surfaced via the context-type tab strip.
- **N-Way Interaction / Parallel-Stats Patterns** — the "no statrs" convention: all statistics are
  hand-rolled on `std` + existing helpers (normal CDF/`erf`, chi-square SF, F-dist SF). Ratio metrics
  reduce via the **delta method**; every family collapses to an asymptotically-normal `(estimate, se)`.
  Stats core built via parallel worker-waves with strict file-ownership tables.
- **ClickHouse** — `experiment_results` is MergeTree; AggregatingMergeTree needs `*State`/`*Merge`
  combiners; `sumState(Nullable(Float64))` mismatches `AggregateFunction(sum, Float64)` (wrap with
  `ifNull(.,0.0)`); event-writer migrations array auto-applies on analytics-service boot.
- **Testing** — Monte-Carlo simulation tests for statistical correctness (seed determinism; this track
  must prove peeking-under-H₀ ≤ α and fixed-horizon inflation by simulation).
- **Verification & CI Gotchas** — the final gate MUST include `cargo fmt --all --check` (formatting drift
  has reached main before); run `cargo clean -p stitchd-proto` before a cold `sqlx prepare --check` after
  a big merge to avoid phantom "no field on proto type" errors; `cargo sqlx prepare` must use the SAME
  flags as CI (`--all-targets --features stitchd-sdk-rust/test-util`), not a narrower `-- --tests`.
- **Docs Autogeneration Patterns** — gRPC `docs/src/grpc/*_service.md` + `openapi.json` are
  gitignored/ephemeral; the docs-idempotency gate only covers tracked READMEs/env-vars/quickstart. Edit
  source-of-truth (proto, `//!` preamble, env-var decl, lib.rs Quickstart), never the generated page.

### Track-specific design commitments (confirmed in spec)

- **Method:** mSPRT always-valid p-values + mSPRT-dual confidence sequences, over a single normal-mixture
  core operating on `(δ̂, se)` — all four metric families (conversion/count, continuous, ratio, funnel).
- **Looks ride the existing 60-min tick** — always-valid p-values stay valid on a coarse look grid;
  dashboard peeking between ticks is safe. No per-event update loop.
- **Always-valid p-value is a persisted running-minimum** across recompute ticks (monotone non-increasing,
  seeded at 1.0). Requires reading the prior tick's value from `experiment_results`.
- **"Safe to stop" is advisory only** — no automatic experiment halt / auto-ship.
- **Opt-in per experiment** with advanced knobs (α, τ², min-sample-before-first-look), snapshotted onto
  `experiment_iterations` like `pre_period_days` / `unit_context_types`.

---

<!-- Learnings from implementation will be appended below -->
