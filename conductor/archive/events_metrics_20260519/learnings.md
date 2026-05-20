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

## [2026-05-19 11:00] - Phase 3 Complete (Metric CRUD API & Service) + Phase 2 Tasks 1+2

**Wave 1 (3 parallel workers, all merged):**
| Worker | Branch | Commit | Tests | Patterns elevated |
|---|---|---|---|---|
| 1 (TrackEvents gRPC + CH batch INSERT) | w1_track_events | `8dc3888` | analytics-service 114/114 | `moka::future::Cache<(env_id, key), Arc<EventDefinition>>` 60s TTL for hot-path validation |
| 2 (MetricService gRPC impl, replaces 6 stubs) | w2_metric_grpc | `9358c6f` | analytics-service +24 new | `handle_*` extraction pattern keeps service.rs as the trait-impl spine |
| 3 (metric:read|write RBAC) | w3_rbac_perms | `b9303ed` | auth-service 96/96 + 3 new | `tests/rbac_test.rs` doesn't exist — RBAC tests live inline in `rbac.rs`; update FILES_OWNED for future RBAC tasks |

Merge conflicts (worker 1 vs worker 2): both extended `service.rs` imports + `ServiceState`. Resolved by keeping both additions — `metric_repo: Arc<PgMetricRepository>` + `event_def_cache: EventDefinitionCache` co-exist as separate `ServiceState` fields.

**Wave 2 (2 parallel workers, all merged):**
| Worker | Branch | Commit | Tests | Patterns elevated |
|---|---|---|---|---|
| 4 (POST /v1/events/track route) | w4_gw_events | `8d33fde` | gateway 169/169 (22 in events module) | `axum::body::Bytes::from_request(req, &()).await` honors `DefaultBodyLimit` and auto-maps oversize → `413` — avoids hand-rolling `LengthLimitError` chain |
| 5 (/v1/metrics CRUD + preview) | w5_gw_metrics | `ec56079` | gateway 182/182 (13 new) | (a) Local `tonic::Status → GatewayError` mapping for module-specific codes (e.g. `Aborted` → `Conflict`); (b) Domain type as response shape for admin-only routes via `stitchd-core` openapi feature; (c) Parameterised `Behaviour` enums + `Captured<...>` for in-process tonic mock |

Merge conflicts (worker 4 vs worker 5): both extended `router.rs` and `openapi.rs`. Resolved by combining both. **Worker 5 also moved Prometheus exposition from `/v1/metrics` to `/metrics`** (path collision with the new admin metric CRUD routes) — touched `URL_SPACE.md`, `tech-stack.md`, `dev.sh` as part of the fix-gaps-as-discovered.

**Beads state drift** (observed AGAIN this session, matches `boundaries_20260518` learnings): Phase milestones + verification subtasks revert from CLOSED → OPEN after the dolt export step. Required `bd close --force --no-auto` to re-close every time. Track the work via git commit + git note for truth; beads is best-effort coordination.

**Total commits on `track/events_metrics_20260519`**: 14 (8 from Phase 1 + 6 from Phase 2/3 parallel waves)

**Phase 2 remaining**: Task 2.3 (per-env quota middleware) in flight as worker_6. Then 2.4 verification + close milestone.

---

## [2026-05-19 10:15] - Phase 1 Complete (DB & Schema Foundations)

**6 tasks done, 7 commits on `track/events_metrics_20260519`.** Per-task summary:

| Task | Commit | Lines | Key artefact |
|---|---|---|---|
| 1.1 Domain types | `8d1d6b9` | +698 | `stitchd-core::metric::{MetricDefinition, MetricKind}` + 24 tests |
| 1.2 PG migration | `49bac46` | +55 | `metric_definitions` table + 3 indexes + 2 CHECK constraints |
| 1.3 Repo trait + Pg impl | `(commit)` | +865 | `MetricRepository` + `PgMetricRepository` + 17 integration tests |
| 1.4 Proto messages | `8d2eda5` | +225 | `AnalyticsService` extended with 6 metric RPCs + oneof config |
| 1.5 ClickHouse schema | `45ebc69` | +37 | `events_v2 + properties + occurred_at` columns |
| (Cleanup) | `0bf1084` | +38 | MockAnalyticsService stubs |

**Phase verification:**
- `cargo test -p stitchd-core -p stitchd-db -p stitchd-proto`: **all green** (414 + 17 new metric repo + 24 new metric domain tests pass)
- `cargo clippy --workspace --all-targets -- -D warnings`: clean

**Phase-level patterns to carry forward into Phase 2/3:**
- Adding RPCs to a tonic service trait BREAKS every existing impl — always grep for `impl <Service> for` across the workspace and add Unimplemented stubs to MockServices in tests too.
- `MetricKind` serde discriminator (`kind`) and proto oneof variant names line up exactly — no mapping layer needed between proto and domain.
- Raw `sqlx::query` strings (not the `query!`/`query_as!` macros) keep the `.sqlx/` cache stable for parallel workers — adopt for all new repos in this track.
- Versioned-entity columns are `BIGINT` in PG → `i64` in Rust. `RepositoryError::VersionConflict.{expected,actual}` are `i64`. Don't use `i32` for versions in new domain types.

---

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
