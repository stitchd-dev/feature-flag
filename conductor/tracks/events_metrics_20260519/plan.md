# Plan: events_metrics_20260519

Building the complete Events + Metrics module per `spec.md`.

Track ID: `events_metrics_20260519`

## Phase 1: DB & Schema Foundations
<!-- depends: -->

PG schemas for `metric_definitions`, ClickHouse view adjustments,
domain types in `stitchd-core`, repo trait + Pg/composite impls in
`stitchd-db`. Sets up everything downstream phases depend on.

- [x] Task 1.1: Domain types in `stitchd-core` for MetricDefinition + MetricKind [8d1d6b9]
  <!-- files: crates/stitchd-core/src/metric/mod.rs, crates/stitchd-core/src/metric/kinds.rs, crates/stitchd-core/src/id.rs, crates/stitchd-core/src/lib.rs -->
  - TDD: type definitions, serde round-trip tests, MetricKind config validators (e.g. funnel must have ≥2 steps, ratio must have distinct num/denom)
- [x] Task 1.2: PG migration `metric_definitions` table [49bac46]
  <!-- files: crates/stitchd-db/migrations/20260520000001_metric_definitions.sql -->
  - Schema: id, env_id, key, name, description, kind, config JSONB, goal_direction, version, created_at, updated_at, deleted_at
  - Indexes: `(env_id, key) WHERE deleted_at IS NULL`, `(env_id) WHERE deleted_at IS NULL`
- [x] Task 1.3: `MetricRepository` trait + `PgMetricRepository` impl in `stitchd-db` [(commit)]
  <!-- files: crates/stitchd-db/src/repository/metric.rs, crates/stitchd-db/src/repository/mod.rs, crates/stitchd-db/src/repository/pg/metric.rs, crates/stitchd-db/src/repository/pg/mod.rs -->
  - TDD: CRUD, list_by_env, soft-delete, optimistic-locking, find_by_key
  - Use `#[sqlx::test(migrations = "./migrations")]`
- [x] Task 1.4: Proto messages for MetricDefinition + MetricKind oneof [8d2eda5]
  <!-- files: crates/stitchd-proto/proto/metric.proto, crates/stitchd-proto/proto/analytics.proto, crates/stitchd-proto/src/mapping/metric.rs -->
  - Add `MetricDefinition`, `AggregationConfig`, `RatioConfig`, `FunnelConfig`, `FunnelStep` messages
  - Add to `AnalyticsService` proto: `CreateMetric`, `GetMetric`, `ListMetrics`, `UpdateMetric`, `DeleteMetric`, `PreviewMetric`
  - Mapping tests
- [x] Task 1.5: ClickHouse schema for `events_v2` properties + index [45ebc69]
  <!-- files: crates/stitchd-db/clickhouse-migrations/0010_events_v2_properties.sql -->
  - Add `properties Map(String, String)` and `occurred_at DateTime64(3, 'UTC')` to `events_v2` if missing
  - Verify `events_experiment_daily` MV is still well-formed (or recreate)
- [x] Task 1.6: Conductor - User Manual Verification 'DB & Schema Foundations' [phase-checkpoint]

## Phase 2: Backend Ingestion Path
<!-- execution: parallel -->
<!-- depends: phase1 -->

REST `POST /v1/events/track` on gateway + new `TrackEvents` gRPC on
analytics-service + ClickHouse batch write + per-env quota.

- [x] Task 2.1: `AnalyticsService.TrackEvents` gRPC implementation [8dc3888]
  <!-- files: crates/stitchd-analytics-service/src/grpc/ingestion.rs, crates/stitchd-analytics-service/src/grpc/mod.rs, crates/stitchd-analytics-service/src/lib.rs -->
  - TDD: rejects unknown event_keys, archived events return 410, valid batch writes N rows to ClickHouse
  - Schema validation against `event_definitions` (PG); cache for hot-path lookups (moka, 60s TTL)
  - ClickHouse batch INSERT using `ch_pool::insert`
- [x] Task 2.2: Gateway `POST /v1/events/track` route + DTOs [8d33fde]
  <!-- files: crates/stitchd-gateway/src/routes/events.rs, crates/stitchd-gateway/src/routes/mod.rs, crates/stitchd-gateway/src/openapi.rs -->
  <!-- depends: task1 -->
  - SDK auth middleware extracts env_id, propagates as `x-env-id` to analytics-service
  - Returns 202 with per-event accepted/rejected detail; 413 on >5MB body
  - utoipa annotation for OpenAPI surface
- [x] Task 2.3: Per-env quota middleware (governor) for ingestion route [b27de1e]
  <!-- files: crates/stitchd-gateway/src/middleware/event_quota.rs, crates/stitchd-gateway/src/lib.rs -->
  <!-- depends: task2 -->
  - Default 1000 events/sec/env_id; configurable via `STITCHD_EVENT_QUOTA_PER_SEC`
  - Per-env state via `governor::Quota` keyed on `env_id` string
  - TDD: per-env limit enforced, exceeded returns 429
- [x] Task 2.4: Conductor - User Manual Verification 'Backend Ingestion Path' [auto-verify]

## Phase 3: Metric CRUD API & Service
<!-- execution: parallel -->
<!-- depends: phase1 -->

Analytics-service gRPC + gateway REST routes for metric_definitions
CRUD + preview endpoint.

- [x] Task 3.1: Analytics-service `MetricService` gRPC impl [9358c6f]
  <!-- files: crates/stitchd-analytics-service/src/grpc/metric.rs, crates/stitchd-analytics-service/src/grpc/mod.rs, crates/stitchd-analytics-service/src/lib.rs -->
  - TDD: CRUD + list_by_env + validation (kind-specific config validators)
  - Calls `PgMetricRepository` from Phase 1
  - `PreviewMetric` RPC runs the metric against last 7d, returns time-series
- [x] Task 3.2: Gateway `/v1/metrics` routes + DTOs + RBAC [ec56079]
  <!-- files: crates/stitchd-gateway/src/routes/metrics.rs, crates/stitchd-gateway/src/routes/mod.rs, crates/stitchd-gateway/src/openapi.rs -->
  <!-- depends: task1 -->
  - POST, GET, PATCH, DELETE `/v1/metrics`; GET `/v1/metrics/{id}`; POST `/v1/metrics/{id}/preview`
  - RBAC: `metric:read` for GET routes, `metric:write` for mutations
  - utoipa annotations
- [x] Task 3.3: Add `metric:read|write` permissions to RBAC role definitions [b9303ed]
  <!-- files: crates/stitchd-auth-service/src/rbac.rs, crates/stitchd-auth-service/tests/rbac_test.rs -->
  - `org_admin` → both; `org_member` → `metric:read` only
- [x] Task 3.4: Conductor - User Manual Verification 'Metric CRUD API & Service' [auto-verify]

## Phase 4: Stats-Service Metric Dispatch
<!-- depends: phase1, phase3 -->

Rewrite stats-service `compute_experiment` to dispatch on `metric.kind`
and generate the right ClickHouse query per kind.

- [ ] Task 4.1: ClickHouse query builders per MetricKind
  <!-- files: crates/stitchd-stats-service/src/queries/aggregation.rs, crates/stitchd-stats-service/src/queries/ratio.rs, crates/stitchd-stats-service/src/queries/funnel.rs, crates/stitchd-stats-service/src/queries/mod.rs -->
  - TDD: golden-query snapshots for each kind across known fixture configs
  - Funnel uses `windowFunnel` ClickHouse function with `mode='strict_order'`
  - Ratio: two CTEs (numerator, denominator) + delta-method variance approximation
- [ ] Task 4.2: `compute_experiment` dispatch + metric fetch
  <!-- files: crates/stitchd-stats-service/src/compute.rs, crates/stitchd-stats-service/src/lib.rs -->
  - TDD: dispatch table compiles all three kinds, results merged in `experiment_results`
  - Per-metric `posterior` (Bayesian) / `p_value` (frequentist) calc unchanged from current implementation
- [ ] Task 4.3: TriggerRecompute on metric config change (event-driven backfill)
  <!-- files: crates/stitchd-stats-service/src/grpc/trigger.rs, crates/stitchd-stats-service/src/grpc/mod.rs -->
  - When analytics-service emits `MetricUpdated` (over a new internal channel or polled), stats-service finds all running experiments referencing it and triggers async recompute
- [ ] Task 4.4: Conductor - User Manual Verification 'Stats-Service Metric Dispatch' (Protocol in workflow.md)

## Phase 5: Rust SDK track API
<!-- depends: phase2 -->

`Client::track()` + `EventBuffer` + flush triggers + client-side
validation against cached `event_definitions`.

- [ ] Task 5.1: `EventBuffer` struct + flush logic
  <!-- files: crates/stitchd-sdk-rust/src/event_buffer.rs, crates/stitchd-sdk-rust/src/lib.rs -->
  - TDD: size-trigger, interval-trigger, explicit flush, shutdown drain (5s deadline)
  - Backoff on POST failure: 3 retries with exp backoff, then drop + warn
- [ ] Task 5.2: `Client::track()` + validation + Buffer wiring
  <!-- files: crates/stitchd-sdk-rust/src/client.rs, crates/stitchd-sdk-rust/src/lib.rs -->
  <!-- depends: task1 -->
  - TDD: enqueues valid events, rejects unknown event_key, rejects mismatched value type, `is_event_registered` works
  - Event-definitions cache populated by the existing definition-sync poll
- [ ] Task 5.3: `Client::flush()` + `Client::shutdown()` public API
  <!-- files: crates/stitchd-sdk-rust/src/client.rs, crates/stitchd-sdk-rust/src/lib.rs -->
  <!-- depends: task1, task2 -->
  - TDD: graceful shutdown drains in-flight events; force shutdown drops with logged count
- [ ] Task 5.4: SDK Prometheus counters
  <!-- files: crates/stitchd-sdk-rust/src/metrics.rs -->
  - `events_buffered_total`, `events_flushed_total`, `events_dropped_total{reason}`, gauge `event_buffer_size`
- [ ] Task 5.5: Conductor - User Manual Verification 'Rust SDK track API' (Protocol in workflow.md)

## Phase 6: Admin UI — Events & Metrics
<!-- execution: parallel -->
<!-- depends: phase2, phase3 -->

Full events CRUD + tester + metrics CRUD + preview. Each task owns
distinct UI page files, so they can run in parallel.

- [ ] Task 6.1: Events list page + filters + pagination
  <!-- files: admin/src/pages/events/EventsList.tsx, admin/src/pages/events/EventsList.test.ts, admin/src/App.tsx, admin/src/components/icons.tsx -->
  - TDD (Vitest): list rendering, pagination URL sync, filter handlers
  - Reuse pagination primitive + table primitive
- [ ] Task 6.2: Event detail page (firings log + sparkline + experiments-depending-on)
  <!-- files: admin/src/pages/events/EventDetail.tsx, admin/src/pages/events/EventDetail.test.ts, admin/src/lib/api.ts -->
  <!-- depends: task1 -->
  - Calls `GET /v1/events/{key}/firings?limit=50` (new analytics-service endpoint added in this task)
  - Sparkline: 14d daily counts via existing `eval_stats` analog (new `event_stats` endpoint)
- [ ] Task 6.3: Event edit modal + archive flow
  <!-- files: admin/src/pages/events/EditEventModal.tsx, admin/src/pages/events/ArchiveEventModal.tsx, admin/src/lib/validation/eventDefinitionSchema.ts -->
  <!-- depends: task1 -->
  - Reuse existing CreateEventModal Formik+Yup pattern
- [ ] Task 6.4: Test-event widget on detail page
  <!-- files: admin/src/pages/events/TestEventWidget.tsx, admin/src/pages/events/TestEventWidget.test.ts -->
  <!-- depends: task2 -->
  - Form for context_type, context_key, value, properties
  - POSTs to `/v1/events/track` from the admin UI session (admin token, not SDK key); analytics-service marks rows with `test=true` flag
  - Shows resulting ClickHouse row from latest firings poll
- [ ] Task 6.5: Metrics list page + create/edit modal
  <!-- files: admin/src/pages/metrics/MetricsList.tsx, admin/src/pages/metrics/MetricsList.test.ts, admin/src/pages/metrics/CreateMetricModal.tsx, admin/src/pages/metrics/EditMetricModal.tsx, admin/src/lib/validation/metricSchema.ts, admin/src/App.tsx -->
  - TDD: list rendering, kind-switching form fields, Yup schema per kind
  - Discriminated-union form: aggregation (event_key + aggregator + on_field), ratio (numerator/denominator metric picker), funnel (steps list with event_key + window_seconds)
- [ ] Task 6.6: Metric preview component (time-series chart)
  <!-- files: admin/src/components/metrics/MetricPreview.tsx, admin/src/components/metrics/MetricPreview.test.ts -->
  <!-- depends: task5 -->
  - Calls `POST /v1/metrics/{id}/preview`; renders sparkline + last 7d values
- [ ] Task 6.7: Conductor - User Manual Verification 'Admin UI — Events & Metrics' (Protocol in workflow.md)

## Phase 7: Experiment Cutover Migration
<!-- depends: phase3, phase4 -->

Drop raw `event_key` references on experiments; require `metric_ids[]`.
One-shot migration auto-creates "count of {event_key}" metrics for
existing experiments and rewrites references.

- [ ] Task 7.1: Migration `experiment_metrics_cutover.sql`
  <!-- files: crates/stitchd-db/migrations/20260520000002_experiment_metrics_cutover.sql -->
  - For each (env_id, distinct event_key in experiments), INSERT a
    metric_definition `(kind=aggregation, config={event_key, aggregator:count})`
  - Add `metric_ids UUID[]` column; populate from old `metric_keys`
  - Drop old `metric_keys` column
  - Idempotency check at top of migration
- [ ] Task 7.2: Domain + Proto + Repo refactor for Experiment.metric_ids
  <!-- files: crates/stitchd-core/src/experimentation/mod.rs, crates/stitchd-proto/proto/experimentation.proto, crates/stitchd-db/src/repository/experiment.rs, crates/stitchd-db/src/repository/pg/experiment.rs -->
  - Replace `metric_keys: Vec<String>` with `metric_ids: Vec<MetricId>`
  - Update Pg repo + proto mapping + experimentation-service + stats-service consumers
- [ ] Task 7.3: Experiment admin UI (CreateExperimentModal) updated to pick metric_ids
  <!-- files: admin/src/pages/experiments/CreateExperimentModal.tsx, admin/src/pages/experiments/ExperimentDetail.tsx, admin/src/lib/validation/experimentSchema.ts -->
  - Replace `primary_metric` string field with a metric picker (calls `GET /v1/metrics`)
- [ ] Task 7.4: Conductor - User Manual Verification 'Experiment Cutover Migration' (Protocol in workflow.md)

## Phase 8: E2E Verification + Docs
<!-- depends: phase4, phase5, phase6, phase7 -->

End-to-end happy-path tests + documentation refresh.

- [ ] Task 8.1: E2E test — SDK fires events, experiment reads them via ratio metric
  <!-- files: tests/e2e/event_metric_e2e.rs, tests/e2e/mod.rs -->
  - Boot all services, register two events, create a ratio metric, create an experiment with that metric, SDK fires 1000 events split 60/40 across variants, stats-service compute_experiment populates experiment_results, assert lift / p-value within tolerance
- [ ] Task 8.2: OpenAPI contract update + check_openapi_contract.py allowlist refresh
  <!-- files: openapi.yaml, scripts/check_openapi_contract.py, crates/stitchd-gateway/src/openapi.rs -->
- [ ] Task 8.3: Docs — events module + metrics manager
  <!-- files: docs/src/architecture/events.md, docs/src/architecture/metrics.md, docs/src/SUMMARY.md, conductor/product.md -->
  - Architecture diagrams (mermaid), schema reference, SDK usage examples
  - Update `product.md` implementation status
- [ ] Task 8.4: Conductor - User Manual Verification 'E2E Verification + Docs' (Protocol in workflow.md)

---

## Parallel Execution Summary

**Task-Level Parallelism:**
- **Phase 1: DB & Schema Foundations** — 5 implementation tasks, sequential (schema migration order matters).
- **Phase 2: Backend Ingestion Path** — parallel. Task 2 depends on Task 1 (analytics gRPC must exist before gateway route); Task 3 depends on Task 2.
- **Phase 3: Metric CRUD API & Service** — parallel. Task 2 depends on Task 1; Task 3 independent.
- **Phase 4: Stats-Service Metric Dispatch** — sequential (each task builds on previous query primitives).
- **Phase 5: Rust SDK track API** — sequential (single crate, shared module).
- **Phase 6: Admin UI** — parallel. Most tasks are independent UI pages; Task 6.4 depends on Task 6.2 (needs detail page); Task 6.6 depends on Task 6.5.
- **Phase 7: Experiment Cutover** — sequential.
- **Phase 8: E2E + Docs** — sequential (E2E test must come last).

**Phase-Level Parallelism:**
- **Phase 1** runs first (no deps).
- **Phase 2 + Phase 3** run in parallel after Phase 1.
- **Phase 4 + Phase 5 + Phase 6** run in parallel after both Phase 2 + 3.
- **Phase 7** waits for Phase 3 + 4.
- **Phase 8** waits for everything.

This pattern matches the proven `boundaries_20260518` parallel worker
model — 6+ workers in the largest wave (Phase 4 + 5 + 6 + their internal
parallelism).
