# Track Learnings: events_metrics_20260519

Patterns, gotchas, and context discovered during implementation.

## Codebase Patterns (Inherited)

📚 Seeded from `conductor/patterns.md` (full list) — primary relevant patterns
for this track:

### From archived events_20260419 / experimentation_20260419
- `clickhouse` crate v0.13 has no `derive` feature — use `uuid`, `time`, `lz4` features.
- `sqlx::query!` macros require live DB or up-to-date `.sqlx` cache; new queries break offline-mode compilation until `cargo sqlx prepare` is run.
- `IntoResponse` on custom `ApiError` enum for HTTP error mapping.
- Axum 0.8 path params use `{param}` syntax (not `:param`).
- `#[sqlx::test(migrations = "./migrations")]` for isolated DB tests.
- `tower::ServiceExt::oneshot` for Axum integration tests without TCP server.
- Recursive types (expression trees / discriminated unions like MetricKind) need `Box<T>` for recursive variants.

### From boundaries_20260518 (most recent — fresh in mind)
- **Gateway is the sole SDK trust boundary** — backend services trust `x-env-id` propagated from gateway; never validate SDK keys in backends.
- **AdminFooJson vs FooJson** — admin UI gets full data, SDK gets minimal. Apply this to events + metrics: `AdminEventJson` for UI, `EventJson` for SDK definition sync.
- **Formik+Yup-only forms** in admin UI; modal primitives in `admin/src/components/`.
- **Per-crate `cargo test -p <crate>`** for clean signal during verification.
- **Body size limit on bulk routes**: `DefaultBodyLimit::max(5 * 1024 * 1024)` for the `/v1/events/track` route.
- **Worktree discipline**: `cd .worktrees/<track_id>/` before any `cargo` command; otherwise it compiles main branch.
- **`bd close --no-auto`** for parallel waves to prevent cascade into next phase.
- **Fix gaps as discovered** — file pre-existing issues as separate beads bugs (priority 2); fix in-scope drift inline.

### From db_optim_20260516 — ClickHouse
- **AggregatingMergeTree** insert/read combiners: `*State` to write, `*Merge` to read. NEVER `finalizeAggregation` in GROUP BY.
- **`sumState(Nullable(Float64))` type mismatch** — wrap with `ifNull(expr, 0.0)` if target column is non-nullable.
- **Weekly partitions** via `toMonday(event_date)`; TTL `INTERVAL 52 WEEK` for year retention.

### Track-specific anchors
- **Metric kind oneof** — `MetricKind` is a discriminated union; serde tag = `kind`. Protobuf maps to a `oneof` field; gRPC consumers must dispatch on the variant.
- **Cutover migration safety** — idempotency guard at top of migration (`WHERE NOT EXISTS (...)` for inserted metric_definitions; bail if `metric_ids` column already exists).
- **SDK buffer flush semantics** — three triggers (size, interval, explicit) — keep flush idempotent (drain → POST → on error retain).
- **Funnel ClickHouse `windowFunnel`** — output is `level: UInt8` (number of steps completed in order). Conversion rate at step N = `countIf(level >= N) / countIf(level >= 1)`.

---

<!-- Learnings from implementation will be appended below -->

## [2026-05-19 09:50] - Phase 1 Task 1.1: Domain types in stitchd-core for MetricDefinition + MetricKind

- **Implemented:** MetricDefinition struct + MetricKind discriminated union (Aggregation/Ratio/Funnel) with per-kind config structs + GoalDirection + MetricValidationError. 24 unit tests covering serde round-trips and shape invariants.
- **Files changed:** crates/stitchd-core/src/metric/mod.rs (new), crates/stitchd-core/src/metric/kinds.rs (new), crates/stitchd-core/src/lib.rs (add pub mod)
- **Commit:** 8d1d6b9
- **Learnings:**
  - **Pattern — serde `tag` + `#[serde(flatten)]` for discriminated enums in parent structs:** `MetricKind` uses `#[serde(tag = "kind", rename_all = "snake_case")]`. When embedded in `MetricDefinition` with `#[serde(flatten)]`, the top-level JSON gets the `kind` discriminator inline. This matches the protobuf oneof wire format exactly — no separate mapping needed.
  - **Pattern — `requires_field()` helper on enum variants:** `AggregationOperator::Count` doesn't need an `on_field`, the rest do. A `const fn requires_field()` helper centralizes the rule so validation + UI form rendering use one source of truth.
  - **Gotcha — pre-existing `experimentation::stats::MetricType` is NOT the same concept:** `MetricType::{Count, Numeric, Percentile, Funnel}` classifies the STATS methodology used to analyse experiment results. The new `metric::MetricKind` classifies the METRIC PRIMITIVE that produces values. They will compose (a Ratio metric uses Numeric methodology). Don't conflate.
  - **Pattern — `MetricId` newtype already in `id.rs`:** Line 72. No id.rs change needed for this task; the newtype macro `define_id!` already covered it.
  - **Pattern — validation returns typed errors (`MetricValidationError`) not stringly-typed:** Each shape invariant has its own enum variant — gives gateway + UI precise error mapping without parsing error messages.
- **Context:** Foundational task of Phase 1. Tasks 1.2–1.5 build on this (PG schema persists `MetricDefinition`, proto messages mirror it, ClickHouse properties feed the aggregation `on_field` path).

---
