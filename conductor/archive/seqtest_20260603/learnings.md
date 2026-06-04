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

## [2026-06-03] Wave 4 — Phase 6 (docs + final CI gate) — orchestrator, commit `2693abc` (docs)

- Docs: new mdBook page `docs/src/experimentation/sequential-testing.md` (+ SUMMARY + index Topics/status/Out-of-scope), `product.md` (status row + Experimentation bullet + removed from Future), `tech-stack.md` (change-note block + `experiment_results.sequential_result` row). `cargo xtask docs` idempotent (internal-link check 40 files; zero tracked diff — generated grpc/openapi pages are gitignored).
- **Final CI gate ALL GREEN on the merged track:** `cargo fmt --all --check` ✓; `cargo clippy --workspace --all-targets -- -D warnings` ✓; `cargo test --workspace` ✓ **2334 passed / 0 failed** (92 result groups, 59 sequential test refs); `cargo sqlx prepare --workspace --check -- --all-targets --features stitchd-sdk-rust/test-util` ✓ (warm target — no phantom stale-proto error, so the `cargo clean -p stitchd-proto` precaution wasn't needed this time); admin `tsc` ✓ / `lint` 0 errors ✓ / **859 vitest** ✓; OpenAPI contract ✓ (23/23).
- Gotcha: a backgrounded `cargo test ... | tail -60` masks cargo's real exit code (the pipeline exit is `tail`'s) AND truncates the log — re-run to a full log with `; echo $?` (zsh: `$?`, not `${PIPESTATUS[0]}`) to get a conclusive pass/total.
---

## [2026-06-03] Wave 3 — Phase 5 (Admin UI) — W5 `66cebf7` (form) ∥ W6 `9a1f0e5` (Results); merged `bbcbd10`/`cb2bee4`

- **W5 form:** "Sequential testing" section in `CreateExperimentModal.tsx` (gated `SequentialTestingSection`: toggle + α/τ²/min-sample, advanced knobs render only when enabled), mirroring `pre_period_days`/CUPED. Form type + Yup in `validation/experiment.ts` (alpha & min_sample gated via `.when('sequential_testing_enabled')`, tau always-optional `>0`); body built in `CreateExperimentModal.helpers.ts` (`sequentialTestingFields()`, tau OMITTED when blank → gateway `Option<f64>` auto-derive). **No `api.ts`/`lib/types.ts` change needed** (modal builds body inline + posts). Config JSON keys = field names (no serde rename): `sequential_testing_enabled`, `sequential_alpha`, `sequential_tau_squared`, `sequential_min_sample_size`.
- **W6 Results:** third **`'sequential'`** `ResultsView` (toggle shown only when a variant has `sequential_p_value != null`; stale-pref guard falls back). Columns Variant·Samples·Lift·**Always-valid p**·**Anytime CI** (`fmtAnytimeCI`, "insufficient" when flagged/absent; control "—"). `isSafeToStop(row, goalDirection)` = `sequential_crossed === true && lift directional` → "✓ Safe to stop" badge + winner highlight (never control/insufficient). `VariantResultJson` (`lib/types.ts`) gained the 6 optional `sequential_*` fields.
- **Inline gap-fix (W5):** `CreateExperimentExclusion.test.ts` `validBase` fixture needed the new required `ExperimentFormValues` fields to compile (mechanical, no behavior change).
- Combined admin verify on track: `tsc` clean, `npm run lint` passes (warnings only — pre-existing `set-state-in-effect` + the established exported-helper `react-refresh` pattern), **859 vitest pass / 53 files**.
- Test env is `node` (no jsdom): assert on `renderToString` HTML via `data-*` hooks (e.g. `data-safe-to-stop`), matching the existing `data-winner` convention.
---

## [2026-06-03] Wave 2b — Phase 4 (read path + gateway) — commit `16276b9` (W4), merged into track

- `get_results` (experimentation-service `service.rs`) parses `ExperimentResult.sequential_result` JSON blob → `VariantResult.sequential_*` per variant (mirrors the `frequentist_result` parse). Fields stay `None` when no blob.
- **Gateway results JSON keys (snake_case, Admin UI consumes these):** `sequential_p_value`, `sequential_ci_lower`, `sequential_ci_upper`, `sequential_crossed`, `sequential_insufficient_data`, `sequential_method` — on each variant in `results_by_context_type[].variants[]` (and `…guardrails[]`). All `Option` with `skip_serializing_if=Option::is_none` → **keys are OMITTED (not null)** when sequential disabled, and CI keys omitted when insufficient. **No `safe_to_stop` field** — the UI computes it from `sequential_crossed` + `lift` + `goal_direction`.
- Tests: experimentation-service 88, gateway 249 (+ openapi schema test); OpenAPI contract check green (additive fields don't affect route-level contract). utoipa `ToSchema` derive auto-documents the new DTO fields.
- W4 also fixed a pre-existing test fixture (`make_analytics_result`) that didn't compile against the Phase 2 proto (missing `sequential_result`).
---

## [2026-06-03] Wave 2 — Phase 3 (compute + storage reconcile) — commit `1f1a003` (W3), merged `9589793`

- **Storage reconciled to a JSON blob.** Phase 2's 6 scalar `sequential_*` columns were the wrong granularity (a result row is per `(metric, context_type)` with variants packed in JSON, `variant_key=''`). W3 dropped them and added a single `sequential_result String` CH column; `WriteExperimentResultsRequest.sequential_result` = tag **13**, analytics read `ExperimentResult.sequential_result` = tag **14**. Shape: `{ "<variant_key>": {"always_valid_p":f64, "p_crossed":bool, "ci_lower":f64|null, "ci_upper":f64|null, "insufficient_data":bool, "method":"msprt"} }` (control baseline = p 1.0 / not crossed; ci null when insufficient). **W4 (read) parses this.**
- New `sequential_compute.rs`; `build_metric_summaries` gained a `sequential_per_pair: &HashMap<(String,String),serde_json::Value>` arg. Config flows iteration-snapshot → `enrich_sequential_settings` (uses `GetExperimentIteration`; `ListRunningExperiments` does NOT carry the fields) → `resolve_sequential_config`. τ² default = unit-information pooled variance (`p̄(1−p̄)` / pooled sample var), floored at 1e-9. Running-min reads prior `sequential_result` from CH `experiment_results` (MAX `computed_at` for `(env,exp,iter,metric,context)`; variant_key='' so not in the key), seeds 1.0.

### ⚠️ MAJOR pre-existing-state discovery (Revision 1 / `revisions.md`)
The **stats-service per-metric compute is a pre-existing scaffold**: `main.rs:~178` calls `write_results(exp, &[])` with EMPTY summaries (`// Stats computation is deferred to Phase 3 full implementation`), and a workspace-wide grep finds **no service calls any stitchd-core stats fn** (`frequentist::`/`bayesian::`/`cuped`/`srm`). So NO live experiment_results are computed today — for frequentist/bayesian either. Sequential is delivered at **parity** (core + storage + config + read + UI, wired at the `build_metric_summaries` seam) and activates the instant a real compute pass lands. Follow-ups: **`feature-flag-k1l`** (wire the live per-metric compute pass — affects all stats) and **`feature-flag-2lh`** (ratio delta-method sum aggregation in that pass). The experimentation-service proto's own `VariantResult` keeps the per-variant scalar `sequential_*` fields (W2) — those are the read-API output W4 populates from the blob.
---

## [2026-06-03] Wave 1 — Phase 1 (stats core) + Phase 2 (schema/config/proto)

Commits: Phase 1 `69e0218` (W1), Phase 2 `99e456f` (W2). Merged to track `35f6abe`/`7f78714`. Integration build green; 709 stitchd-core tests pass.

**Public API of `stitchd-core::experimentation::stats::sequential` (consumed by Phase 3):**
- `SequentialConfig { alpha: f64, tau_squared: f64, min_sample_size: i64 }` (Default: 0.05 / 1.0 / 100).
- `SequentialResult { always_valid_p, p_crossed, ci_lower, ci_upper, method: String, insufficient_data }`.
- `RatioGroupStats { n, num_sum, den_sum, num_sq_sum, den_sq_sum, num_den_sum }`.
- `sequential_test(delta_hat, se, n, &cfg, prev_p)`; adapters `sequential_count/numeric/funnel(&VariantStats,&VariantStats,&cfg,prev_p)`, `sequential_ratio(&RatioGroupStats,&RatioGroupStats,&cfg,prev_p)`; `split_alpha(&cfg, num_variants)` (Bonferroni on the threshold α/(K−1)).

**Gotchas / patterns discovered:**
- **mSPRT must be computed in log space.** Computing the mixture LR `lambda` directly overflows to +∞ for strong effects, which would spuriously collapse a clearly-significant look to `insufficient_data`. Compute `ln_lambda`, then `p_look = exp(-ln_lambda).min(1.0)`. The CI is independent of lambda and stays finite.
- **Ratio sufficient-stats are NOT on `VariantStats`** → `sequential_ratio` takes explicit `RatioGroupStats` (mirrors `interaction::ratio::RatioAgg`); difference-of-ratios SE via delta method `Var(R) ≈ (var_num − 2R·cov + R²·var_den)/(mean_den²·n)`.
- **Insufficient-data sentinels:** `always_valid_p=1.0`, `p_crossed=false`, CI=(−∞,+∞). Triggered by n<min_sample, non-finite/≤0 se or τ², α∉(0,1), degenerate ratio groups.
- **Experiment PG repo uses dynamic `sqlx::query()` (runtime-checked), not `query!` macros** → adding columns does NOT change the `.sqlx` offline cache (regeneration produced no diff). Good to know for sqlx-check.
- **ClickHouse ALTER needs HTTP POST** (`curl --data-binary`); GET implies readonly (`Code: 164`). CH `clickhouse` 0.15 inserts by **named** column, so physical column order (appended via ALTER vs mid-table in migration) is irrelevant.
- **Baseline-edit-in-place works** (system not live): later real migrations (`20260602…`) still apply on top of the edited baseline; `#[sqlx::test]` builds fresh DBs from baseline + all migrations and stays green.
- **Proto config round-trip gated on `sequential_testing_enabled || sequential_tau_squared.is_some()`** in the gateway UPDATE path — proto3 can't distinguish unset from false/0 for plain scalars, so this avoids clobbering config on unrelated PATCHes.
- **Proto tags claimed:** Experiment 21–24, ExperimentIteration 12–15, VariantResult 10–15, WriteExperimentResultsRequest 13–18.

**Key file paths for Phase 3/4:**
- Experiment repo: `crates/stitchd-db/src/repository/pg/experiment.rs` (+ iteration snapshot in `apply_transition`).
- Mappers: `crates/stitchd-experimentation-service/src/service.rs` (`core_to_proto`, `iteration_to_proto`).
- Analytics results: `crates/stitchd-analytics-service/src/repo/experiment_results.rs` (`WriteResultRow`, `ExperimentResultRow`) + `src/grpc/experiment_results.rs`.
- Stats writer: `crates/stitchd-stats-service/src/results_writer.rs` (currently sends sequential fields as None).
---
