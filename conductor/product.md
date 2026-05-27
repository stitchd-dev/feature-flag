# Initial Concept
Stitchd Feature Flag is a self-hosted platform for feature flagging and experimentation.
<!-- Last refreshed: 2026-05-27 (post schema_cutover_20260525 merge) -->

# Product Guide

## Vision

Stitchd Feature Flag is a Feature Flagging & Experimentation platform focused on 
self-hosted deployment. It targets internal engineering teams, SaaS product teams, 
and data/growth teams who need reliable flag evaluation and statistically rigorous 
A/B experimentation. Admin UI is coming later as a separate project.

## Target Users
- Internal engineering teams (self-hosted deployments)
- SaaS product teams (multi-tenant)
- Data / growth teams running A/B and multivariate experiments

## Deployment Model
- **Current:** Self-hosted (primary focus) — seven-service Docker Compose stack
- **Internal Architecture:** Seven gRPC microservices (`auth`, `flag`, `segmentation`, `analytics`, `experimentation`, `stats`) + REST gateway; `stitchd-server` monolith removed (2026-04-21); `stitchd-event-writer` library handles ClickHouse writes; `stitchd-sdk-rust` library is the server-side Rust SDK
- **Future:** Cloud SaaS offering

## Multi-Tenancy
Each tenant → multiple environments → each environment has SDK keys (min 1 active; 
supports rotation via create/revoke).

## Scoping Model
- **Project level:** Feature Flag definitions, Variant configurations
- **Environment level:** Rules, Segments, Experiments, Events

## Core Context Model
Each evaluation context: `{_type, key, parameters: Map<String, int|double|semver|string|boolean>, privateParameters: List<String>}`
`privateParameters` identifies fields that must be excluded from all logging.

## Data Persistence & Integrity
- **Optimistic Concurrency:** All mutable entities use version-based optimistic locking 
  to prevent lost updates in highly concurrent environments.
- **Audit Logging:** Every mutation (create, update, soft-delete) is automatically 
  recorded in a central audit log, capturing the actor, resource, and specific changes.
- **Soft Deletion:** Business-critical entities use soft-deletion to maintain data 
  relationships and auditability.

## Context Intelligence Layer
A dedicated layer that observes contexts flowing through the system and maintains 
a registry of known context types, their properties, and observed value ranges/enums.
Exposed as an API for the Admin UI (coming later) to power dropdown/autocomplete 
behaviour (e.g. when building segment rules or flag targeting conditions).

## Implementation Status (as of 2026-05-22)

| Module | Status |
|---|---|
| Domain model + DB scaffold | ✅ Complete |
| Segmentation (rule-based + list-based) | ✅ Complete |
| Feature Flags + Rule Engine | ✅ Complete |
| Events + Experimentation (Frequentist/Bayesian) | ✅ Complete |
| Server-side Rust SDK | ✅ Complete |
| Human Auth (JWT, Password, OIDC, SAML, MFA, Invites, Rate Limiting) | ✅ Complete |
| Microservice decomposition (6 services + gateway) | ✅ Complete |
| Admin UI — Superadmin + Org Management | ✅ Complete |
| Admin UI — Environments & SDK Keys RBAC | ✅ Complete |
| Admin UI — Feature Flags Full CRUD + Rule Builder | ✅ Complete |
| Admin UI — Segments Full CRUD (rule-based + list-based) | ✅ Complete |
| Flag Evaluation Preview (rule traces, rollout debug, OR/AND missing-context fix) | ✅ Complete |
| Context Intelligence (eval telemetry, context registry, autocomplete, explorer) | ✅ Complete |
| Database & Query Optimizations (PG indexes, N+1 elimination, SDK key cache, ClickHouse MVs, offset pagination) | ✅ Complete |
| ScyllaDB list-segment storage (generation swap, sweeper, metrics, OTel spans) | ✅ Complete |
| Boundary Hardening Refactor (boundaries_20260518) | ✅ Complete |
| Events Module — full admin UI + SDK ingestion + per-env quota | ✅ Complete |
| Metrics Layer — composable definitions (aggregation/ratio/funnel) + experiment cutover | ✅ Complete |
| **Experimentation as a whole — complete UI + Backend with eval-log-based first-exposure attribution, whole-flag lock, per-context-type stats, default-rule experiments, Frequentist + Bayesian + CUPED + SRM + Guardrails** | ✅ Complete |
| Flag-Evaluation Unification — single `stitchd-core::evaluation::evaluate_flag` orchestrator drives preview + SDK; canonical `hash_inputs` selector list (cross-context Key + Parameter mixing) end-to-end through Admin UI → REST → PG → preview AND snapshot → SDK | ✅ Complete |
| Schema Hard Cutover — collapsed 44 Postgres, 14 ClickHouse, and 5 ScyllaDB migrations into single V1 baselines; retired segment_rules, dual-write hash_inputs, and flag_evaluation_log_v2; migrated rollout percentages to u32 basis points (0.01% precision) | ✅ Complete |

## Modules

### 1. Segmentation
- Rule-Based Segments: rules evaluated against client Contexts
- List-Based Segments: per context-type include/exclude key lists
  - Persistence: **ScyllaDB** — wide-row tables partitioned by `(segment_id, context_type)`;
    atomic generation swap via LWT CAS; orphaned generations cleaned up by background sweeper.
    PostgreSQL retains segment metadata (name, type, counts, audit log) only.

### 2. Feature Flags
- Typed flags: `int | double | bool | string | json`; variants must match flag type
- States: enabled (default rule + custom rules) / disabled
- Output: specific variant OR percentage allocation (0.1% granularity)
  hash(targeted context keys/params, flag key, project id, environment)
- **Unified evaluation orchestrator (post-`flag_eval_unify_20260522`):** every variant assignment — preview path, SDK path, default-rule fallthrough, and default-rule distribution — funnels through the SOLE `stitchd-core::evaluation::evaluate_flag(...)` entry point. It owns rule iteration, percentage allocation, default-rule fallthrough, and trace emission. `evaluate_preview` is a thin wrapper that calls it with `TraceLevel::Full`; the Rust SDK's `evaluate(...)` calls it with the caller-requested trace level. There is no per-caller orchestration loop anywhere else in the codebase — the only legitimate path to a variant assignment goes through this function.
- **Cross-context percentage hashing:** a rule's percentage allocation carries an ordered `hash_inputs: Vec<HashSelector>` list. Each selector is either `ContextKey { context_type }` (hash the `key` field of the named context) or `ContextParameter { context_type, parameter }` (hash a named parameter of the named context). Selectors mix freely across context types — a single rule can hash on `user.key + user.params.tier + device.params.os + application.key`. Authored once in the Admin UI's `HashInputSelectorList`, persisted to PG as a JSONB column, sent over proto and REST in declaration order, and consumed by `evaluate_flag` via the `HashInputSpec` resolver. The hash algorithm and bucket math (Murmur3 → 0–999) remain unchanged.
- **Evaluate-Preview:** `POST /flags/{key}/evaluate-preview` accepts a mock context and returns
  the evaluated variant plus a full rule trace (which rule matched, why), rollout debug info,
  and OR/AND missing-context resolution details — used by the Admin UI "Test" panel

### 3. Events

- **Pre-registered only.** Each event has a unique `event_key` per environment and a `metric_type` classifier — one of:
  - `count` (occurrence marker, no value required),
  - `conversion` (bool),
  - `revenue` / `duration` / `numeric` (numeric value),
  - `custom` (free-form, optional JSON-schema validation on payload).
- **Optional JSON Schema** on the definition validates event payloads at ingestion (e.g. require `currency` ∈ `{USD, EUR, GBP}` for purchase events).
- **Multi-context attribution:** every firing carries a flat `contexts: {type: key, ...}` map so a single event can be attributed to multiple dimensions simultaneously (e.g. `{user: alice, account: acme, session: s99}`) without inflating count metrics. Stored in ClickHouse as `Array(Tuple(String, String))`.
- **Soft-delete (archive)** rather than hard delete; archived events reject new firings with HTTP 410 while ClickHouse history remains queryable.
- Backed by a `verify_track_event` admin-auth path (`POST /v1/admin/events/track`) for SDK debugging from the UI.

### 4. Metrics

- **Composable primitives** — three kinds, persisted in PostgreSQL `metric_definitions`:
  - **Aggregation** — `count / sum / avg / p50 / p90 / p99 / uniq` over one event stream, optionally filtered by a JsonLogic `where_clause` on `properties[...]`. The `on_field` references either the canonical numeric column (`value`) or a property key.
  - **Ratio** — `numerator / denominator` where both are existing aggregation metrics; below `min_denominator` the bucket emits null (insufficient-data semantics).
  - **Funnel** — ordered list of event-key steps with a `window_seconds` conversion deadline; ClickHouse `windowFunnel` evaluates per `(day, dedup_key)` and the final-step rate is reported as the bucket value.
- **Preview pipeline (Phase 4):** `POST /v1/metrics/{id}/preview` runs the kind-specific ClickHouse query against `events_v2` and returns a zero-filled daily time-series (days clamped to [1, 90]; default 7). Sparkline-ready.
- **Bidirectional UI back-link:** EventDetail page lists every metric that references the event (aggregation by `config.event_key` + funnel step matches; ratio metrics surface transitively through the aggregations they wrap).
- **Goal direction** (`increase` / `decrease` / `neutral`) drives experiment winning-variant logic and the up/down arrow shown in the metric list.

### 5. Experimentation
- Experiments reference **metric_ids** (cutover from raw event_key in migration `20260520000002_experiment_metrics_cutover.sql`); the per-iteration `metric_ids` column lives in `experiment_iterations`.
- **Attribution model (post-`experimentation_full_20260521`):** first-exposure intent-to-treat (ITT), derived server-side from `flag_evaluation_log_v2`. SDKs are experiment-unaware — they do NOT tag events with `(experiment, iteration, variant)` tuples. Eval-log rows route through `experiment_assignments_mv` into `experiment_assignments`; stats queries JOIN `events_v2` ⨝ `experiment_assignments` on `(env_id, context_type, context_key)` and filter `e.occurred_at >= a.assigned_at` for strict ITT.
- **Binding model:** an experiment binds to either (a) a percentage-distribution custom rule via `flag_rule_id`, OR (b) the flag's default-rule fallthrough via `targets_default_rule = true` (requires `feature_flags.default_rule_distribution`). XOR-constrained at the PG layer.
- **Whole-flag lock:** while running/paused, every flag/variant/rule mutation (including default-rule-distribution updates) returns HTTP 409 `FLAG_LOCKED_BY_EXPERIMENT` with the experiment ID in the body. Replaces the old per-rule `frozen` flag.
- **Per-context-type analysis:** every experiment carries `unit_context_types text[] NOT NULL` (default `{user}`). All stats (Frequentist t-test / two-proportion Z, Bayesian posteriors, CUPED, SRM chi-square, guardrail direction) compute independently per context type and surface in the Admin UI via a context-type tab strip.
- **Models:** Frequentist (Welch's t-test, two-proportion Z, Bonferroni correction for >2 variants) and Bayesian (Beta-Binomial / Normal-Normal posteriors, probability-to-beat-control, expected lift). CUPED variance reduction via per-experiment `pre_period_days`. Guardrail metrics flagged on direction violation.
- **Recompute** is scheduled (60-min via `stitchd-stats-service`) plus event-driven via the `TriggerRecompute` gRPC RPC; on-demand from the Admin UI via `POST /v1/.../experiments/{id}/recompute`.
- Future: warehouse-backed event ingestion; multi-armed bandit; sequential testing; cross-experiment interaction analysis.

### 6. Rule Engine
- Core: ordered rule list (first true = exit); AND combinator; per-rule NOT
- Segmentation rules: inherit core
- Feature flag rules: inherit core + "Is in Segment" + "Flag evaluated with variant X"

## Admin UI

The admin console (`admin/`) is a React 19 + Vite SPA with full feature parity:

- **Flags:** Create/edit/archive flags; variant management; rule builder (AND/OR/NOT condition trees, segment picker, percentage rollout). Percentage-rollout outputs (both per-rule allocations and the flag-level default-rule distribution) authored via the `HashInputSelectorList` component — ordered list of cross-context selectors with drag + keyboard reorder, live worked-example banner, context-type + parameter autocomplete sourced from the registry. Evaluate-Preview "Test" panel with rule trace output
- **Segments:** Rule-based (condition expression builder) + list-based (context-typed include/exclude key lists); full CRUD; segment picker in flag rule builder
- **Events:** Full CRUD (`/v1/events*`) — register key + name + metric_type + optional JSON schema; archive (soft-delete); EditEventModal exposes name/metric_type/description/schema (event_key is immutable). EventDetail page surfaces recent firings, 14-day sparkline, the TestEventWidget (admin-auth `POST /v1/admin/events/track`), the back-link "Metrics referencing this event", and "Experiments depending on this event".
- **Metrics:** Full CRUD (`/v1/metrics*`) — kind picker (Aggregation/Ratio/Funnel), event-key autocomplete bound to registered events (strict — unknown keys flagged inline), aggregator + on_field + JsonLogic where_clause for aggregations, numerator/denominator dropdowns for ratios, FieldArray steps for funnels. Detail page calls `POST /v1/metrics/{id}/preview` for the ClickHouse-backed sparkline.
- **Context Explorer:** Browse observed context types and their parameter registry (autocomplete source for rule builder)
- **Eval Analytics:** Evaluation stats per flag via ClickHouse `eval_stats` route; sparklines in flag list
- **Experiments / Environments / SDK Keys / Org Users / Audit Log:** Full management UI
- **Pagination:** All list views use URL-driven offset pagination (`?page=N`) — `PaginationParams` + `PaginatedResponse<T>` backed by `COUNT(*) OVER()` window queries

## Server-Side SDK (Rust — initial)
- `SdkClient::init(config)` blocks until first definition sync via gRPC, then polls at a configurable interval.
- Flag evaluation (`evaluate(&[EvalRequest], TraceLevel)`) is in-process: it delegates straight to `stitchd-core::evaluation::evaluate_flag`, so the SDK shares the exact orchestration the gateway's preview endpoint uses — same rule iteration, same cross-context hashing, same default-rule-distribution support. Each `EvalRequest` carries `flag_key` + `contexts: Vec<Context>`; multi-context bundles drive cross-context percentage hashing identically across SDK and preview.
- Rule-based segments evaluated locally; list-based segments resolved via REST lookup or optional LFU cache.
- Optional LFU membership cache pre-warms list-segment lookups for frequently-evaluated contexts (batch REST refresh on each poll cycle).
- Client-side SDKs (browser/mobile) and server-sent events are out of scope for the initial implementation.
- Future: streaming layer for server-pushed flag updates; direct event submission via SDK key.

## Data Stores
- PostgreSQL: flag/segment configuration, tenants, environments, SDK keys, audit logs
- ScyllaDB: list-segment entry storage (include/exclude lists, up to millions of entries per segment)
- ClickHouse: events, experiment results, metric aggregations

## ClickHouse Query Optimisations (Completed — db_optim_20260516)

The ClickHouse overhaul was completed as part of `db_optim_20260516`:

- **Injection fix:** `eval_stats` route now uses parameterized ClickHouse queries (no `format!()` SQL)
- **Experiment MVs:** `events_experiment_daily` (AggregatingMergeTree, keyed on `env_id, experiment_id, variant_key, metric_key, day`) + backfill migration; `experiment_queries.rs` reads from MVs
- **Partition tuning:** `events_v2` and `flag_evaluation_log_v2` use weekly `toMonday()` partitions + TTL
- **Scheduled stats:** 60-minute interval via `stitchd-stats-service`; Results API reads from pre-computed `experiment_results` table only
