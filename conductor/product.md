# Initial Concept
Stitchd Feature Flag is a self-hosted platform for feature flagging and experimentation.
<!-- Last refreshed: 2026-05-16 -->

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
- **Current:** Self-hosted (primary focus) — six-service Docker Compose stack
- **Internal Architecture:** Six gRPC microservices (`auth`, `flag`, `segmentation`, `event`, `experimentation`) + REST gateway; `stitchd-server` monolith removed (2026-04-21)
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

## Implementation Status (as of 2026-05-16)

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

## Modules

### 1. Segmentation
- Rule-Based Segments: rules evaluated against client Contexts
- List-Based Segments: per context-type include/exclude key lists
  - Persistence: monthly range-partitioned storage for list entries via pg_partman

### 2. Feature Flags
- Typed flags: `int | double | bool | string | json`; variants must match flag type
- States: enabled (default rule + custom rules) / disabled
- Output: specific variant OR percentage allocation (0.1% granularity)
  hash(targeted context keys/params, flag key, project id, environment)
- **Evaluate-Preview:** `POST /flags/{key}/evaluate-preview` accepts a mock context and returns
  the evaluated variant plus a full rule trace (which rule matched, why), rollout debug info,
  and OR/AND missing-context resolution details — used by the Admin UI "Test" panel

### 3. Experimentation
- Events: pre-registered only; each event has a known key and typed metric value 
  (bool/int/double) — unknown events are rejected at ingestion
- Event payload: `{_type, key}` context + metric key + typed value + timestamp
- Experiments: bound to a flag rule, duration-locked (flag frozen while active)
- Models: Frequentist or Bayesian (with/without CUPED)
- Metrics: event count, numeric aggregation (sum/avg/percentile), funnel/conversion
- Future: warehouse-backed event ingestion

### 4. Rule Engine
- Core: ordered rule list (first true = exit); AND combinator; per-rule NOT
- Segmentation rules: inherit core
- Feature flag rules: inherit core + "Is in Segment" + "Flag evaluated with variant X"

## Admin UI

The admin console (`admin/`) is a React 19 + Vite SPA with full feature parity:

- **Flags:** Create/edit/archive flags; variant management; rule builder (AND/OR/NOT condition trees, segment picker, percentage rollout); Evaluate-Preview "Test" panel with rule trace output
- **Segments:** Rule-based (condition expression builder) + list-based (context-typed include/exclude key lists); full CRUD; segment picker in flag rule builder
- **Context Explorer:** Browse observed context types and their parameter registry (autocomplete source for rule builder)
- **Eval Analytics:** Evaluation stats per flag via ClickHouse `eval_stats` route; sparklines in flag list
- **Experiments / Events / Environments / SDK Keys / Org Users / Audit Log:** Full management UI
- **Pagination:** All list views use URL-driven offset pagination (`?page=N`) — `PaginationParams` + `PaginatedResponse<T>` backed by `COUNT(*) OVER()` window queries

## Server-Side SDK (Rust — initial)
- `SdkClient::init(config)` blocks until first definition sync via gRPC, then polls at a configurable interval.
- Flag evaluation (`evaluate()`) is in-process: rule-based segments evaluated locally; list-based segments resolved via REST lookup or optional LFU cache.
- Optional LFU membership cache pre-warms list-segment lookups for frequently-evaluated contexts (batch REST refresh on each poll cycle).
- Client-side SDKs (browser/mobile) and server-sent events are out of scope for the initial implementation.
- Future: streaming layer for server-pushed flag updates; direct event submission via SDK key.

## Data Stores
- PostgreSQL: flag/segment configuration, tenants, environments, SDK keys
- ClickHouse: events, experiment results, metric aggregations

## ClickHouse Query Optimisations (Completed — db_optim_20260516)

The ClickHouse overhaul was completed as part of `db_optim_20260516`:

- **Injection fix:** `eval_stats` route now uses parameterized ClickHouse queries (no `format!()` SQL)
- **Experiment MVs:** `events_experiment_daily` (AggregatingMergeTree, keyed on `env_id, experiment_id, variant_key, metric_key, day`) + backfill migration; `experiment_queries.rs` reads from MVs
- **Partition tuning:** `events_v2` and `flag_evaluation_log_v2` use weekly `toMonday()` partitions + TTL
- **Scheduled stats:** 60-minute interval via `stitchd-stats-service`; Results API reads from pre-computed `experiment_results` table only
