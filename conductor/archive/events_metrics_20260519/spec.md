# Spec: events_metrics_20260519

Build a complete, end-to-end Events + Metrics module as the foundation for
experimentation. Covers admin UI, REST + gRPC API, Rust SDK firing
mechanism, and a composable metrics layer (aggregations + ratios +
funnels) that experiments target instead of raw event keys.

## Overview

Today the events module is a stub: `event_definitions` exists in PG, a
`CreateEventModal` exists in the admin UI, and ClickHouse has the
`events_v2` ingestion table — but there is no way to list/edit events from
the UI, no way for a Rust SDK consumer to fire an event programmatically,
and experiments reference raw `event_key` strings with no composable
notion of a "metric". The aggregation logic in `events_experiment_daily`
hardcodes `count(*) by metric_key`, which means anything beyond a simple
event-count metric requires backend changes.

This track delivers the missing surfaces in one cohesive piece:

1. **Admin UI** — full events CRUD (list, detail, edit, archive),
   recent-firings log per event, and a "fire test event" widget for
   debugging SDK integrations.
2. **API** — REST batch `POST /v1/events/track` on the gateway with SDK
   key auth + per-env quotas; backed by analytics-service gRPC ingestion.
3. **Rust SDK** — `Client::track()` API with client-side buffer, async
   flush on size/interval/shutdown, schema validation against locally
   cached event_definitions.
4. **Metrics layer** — new `metric_definitions` table (env-scoped) with
   three primitive kinds (aggregation, ratio, funnel), referenced by
   experiments via `metric_id`. Stats-service rewritten to compute
   per-metric aggregations from ClickHouse.
5. **Migration** — pre-launch cutover: drop raw `event_key` references on
   experiments and require `metric_id`; one-shot migration auto-creates
   "count of {event_key}" metrics for existing experiments and rewrites
   their references.

## Functional Requirements

### F1. Events admin UI
- F1.1 — List page at `/org/:orgId/events` shows env-scoped events with
  columns: key, name, metric_type, last_fired_at (from ClickHouse), 24h
  count, archived state. Filters: search by key/name, metric_type, status.
  URL-driven pagination.
- F1.2 — Detail page at `/org/:orgId/events/:eventKey` shows: full
  definition, JSON schema (if any), recent firings (last 50, queried
  ClickHouse), sparkline of last 14 days, list of experiments depending
  on this event.
- F1.3 — Edit modal: name, description, schema. Key is immutable.
- F1.4 — Archive flow (soft-delete in `event_definitions`) with confirm.
  Archived events reject new firings at the gateway with 410 Gone.
- F1.5 — Test-event widget on detail page: fires a synthetic event
  against the current env via the SDK key path (or a new admin-token
  path); shows the resulting ClickHouse row. RBAC: requires
  `event:write` permission.

### F2. SDK Rust track API
- F2.1 — `Client::track(event_key, context, value: Option<TypedValue>,
  properties: Option<Map<String, Value>>)` enqueues an event on a
  per-client `EventBuffer`.
- F2.2 — Buffer flushes on:
  - Size threshold (default 100 events; configurable per `SdkConfig`)
  - Time interval (default 5s)
  - `Client::flush().await` explicit call
  - `Client::shutdown().await` (best-effort, with 5s timeout)
- F2.3 — On flush, SDK calls `POST /v1/events/track` with `x-sdk-key`
  header and the buffered batch. Failed batches retry with exponential
  backoff (3 retries, then dropped + warning logged).
- F2.4 — Client-side validation: SDK rejects events whose `event_key` is
  not in the locally cached `event_definitions` payload (already polled
  by the SDK for flags), AND whose `value` type doesn't match the
  registered `metric_type`. Rejected events emit a `tracing::warn!` and
  are NOT enqueued.
- F2.5 — `Client::is_event_registered(event_key)` synchronous helper for
  pre-flight checks.

### F3. Gateway + analytics-service ingestion
- F3.1 — `POST /v1/events/track` route on gateway (under SDK auth
  middleware). Body: `TrackEventsRequest { events: Vec<TrackEvent> }`
  with `TrackEvent { event_key, context_type, context_key, value:
  Option<TypedValue>, properties: Option<JsonValue>, occurred_at:
  Option<Iso8601> }`. Returns `202 Accepted` with per-event status
  (accepted/rejected with reason).
- F3.2 — Per-env quota enforcement (governor middleware): default 1k
  events/sec per env_id, configurable via env var.
- F3.3 — Gateway proxies to analytics-service via a new
  `AnalyticsService.TrackEvents` gRPC. Analytics-service validates,
  writes to ClickHouse `events_v2` in batches.
- F3.4 — Body size limit: 5MB (one batch ~ 25k tiny events). Larger
  batches return 413.

### F4. Metrics layer
- F4.1 — New `metric_definitions` PG table with columns: id, env_id,
  key, name, description, kind (`aggregation` | `ratio` | `funnel`),
  config (JSONB — kind-specific config), goal_direction (`increase` |
  `decrease` | `neutral`), version, created_at, updated_at, deleted_at.
- F4.2 — Three metric kinds:
  - **Aggregation**: `{event_key, aggregator: count|sum|avg|p50|p90|p99
    |uniq, on_field: Option<String>, where_clause: Option<JsonLogic>}`
  - **Ratio**: `{numerator_metric_id, denominator_metric_id, min_denominator}`
  - **Funnel**: `{steps: [{event_key, where_clause: Option<JsonLogic>}],
    window_seconds, count_repeats: bool}`
- F4.3 — Admin UI: `/org/:orgId/metrics` list page + create/edit modal
  with kind-specific form. Reuse Formik+Yup pattern.
- F4.4 — REST routes: `POST/GET/PATCH/DELETE /v1/metrics`, `GET
  /v1/metrics/{id}`. RBAC-gated under `metric:read|write`.
- F4.5 — Metric preview endpoint: `POST /v1/metrics/{id}/preview` runs
  the metric against last 7 days of data and returns time-series
  values — used by the metric-builder UI for instant feedback.

### F5. Experiment integration
- F5.1 — Add `metric_ids: Vec<MetricId>` to `experiments` table;
  drop the old `metric_keys: Vec<String>` column (pre-launch cutover).
- F5.2 — Migration `20260520000001_experiment_metrics_cutover.sql`:
  for each existing experiment, auto-create a "count of {event_key}"
  aggregation metric per old `metric_keys[]`, populate `metric_ids[]`,
  drop `metric_keys`.
- F5.3 — Stats-service `compute_experiment` rewritten to dispatch on
  `metric.kind` (aggregation/ratio/funnel) when generating ClickHouse
  queries. Per-kind ClickHouse query templates live in
  `crates/stitchd-stats-service/src/queries/{aggregation,ratio,funnel}.rs`.
- F5.4 — Backfill: when a metric's `config` changes (e.g. funnel window
  widened), trigger an async `TriggerRecompute` gRPC for every running
  experiment that references it. Stats-service replays ClickHouse data
  from experiment_start_date through now.

## Non-Functional Requirements

- **Coverage**: ≥90% per crate (CI threshold) — applies to
  `stitchd-analytics-service` (new ingestion path), `stitchd-stats-service`
  (new query dispatch), `stitchd-sdk-rust` (new track API + buffer),
  `stitchd-db` (metric repo), admin UI (Vitest, current bar is
  234 tests all passing).
- **Throughput**: SDK + gateway ingestion must sustain 10k events/sec
  per gateway pod with p99 < 50ms (excludes ClickHouse write time).
- **Backwards compat**: Pre-launch → none required for the
  experiment_metrics cutover. SDK API is additive.
- **Observability**: per-route OTel spans on `/v1/events/track`,
  metric-eval traces in stats-service, Prometheus counter
  `events_ingested_total{env_id, event_key, result}` and gauge
  `event_buffer_size{client_id}` for SDK.
- **Security**: SDK key validates env_id; event payload size ≤ 5MB;
  per-env quota; reject schemas with unknown fields when registered
  JSON schema is strict-mode.

## Acceptance Criteria

- [ ] User can create, list, edit, archive an event from the admin UI.
- [ ] User can click "Fire test event" on an event's detail page and see
      the resulting ClickHouse row within 2 seconds.
- [ ] A Rust SDK consumer can call `client.track("checkout_completed",
      context, Some(TypedValue::Bool(true)), None).await` and the event
      lands in ClickHouse with the correct env_id + context.
- [ ] SDK rejects an `track()` call with an unregistered `event_key`,
      emits a warning, and does not enqueue.
- [ ] User can create a metric_definition of each kind (aggregation,
      ratio, funnel) from the admin UI; preview endpoint returns
      time-series data.
- [ ] An experiment configured with a ratio metric correctly computes
      lift, p-value (frequentist) and posterior (Bayesian) via the
      stats-service.
- [ ] All 8 phases pass their workflow phase-verification checkpoint.
- [ ] CI green: cargo workspace tests, cargo clippy --workspace
      --all-targets -- -D warnings, npm test (admin), OpenAPI contract
      check.
- [ ] Tarpaulin per-crate coverage ≥ 90%.

## Out of Scope

- Server-pushed event streaming (SSE/WebSocket) — current polling model
  is sufficient.
- Browser/mobile SDK ports — Rust SDK only in this track.
- Custom raw SQL metric escape hatch (deferred — security + cost
  questions need separate review).
- Cohort/segment-scoped metrics (e.g. "checkout rate for premium tier
  users") — covered by the experiment's targeting rule, not metric
  config.
- Event sampling — full ingestion only in this track.
- Multi-region / multi-cluster event routing.
