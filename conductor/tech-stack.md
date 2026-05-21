# Tech Stack
<!-- Last refreshed: 2026-05-22 (post experimentation_full_20260521 merge) -->

## Architecture

The system is decomposed into seven Cargo workspace crates, each a standalone gRPC microservice, fronted by a REST gateway. Two library crates (`stitchd-event-writer`, `stitchd-sdk-rust`) support services and SDK consumers respectively:

| Crate | Role | Type |
|---|---|---|
| `stitchd-gateway` | REST API facade — translates JSON ↔ gRPC, calls all domain services; hosts OpenAPI spec; serves real Prometheus metrics at `GET /metrics` (the conventional path; `/v1/metrics` is the admin metric-definitions CRUD surface) | Binary |
| `stitchd-auth-service` | JWT / SDK-key credential validation; RBAC context assembly | Binary |
| `stitchd-flag-service` | Flag + variant CRUD; server-streaming definition sync for SDK | Binary |
| `stitchd-segmentation-service` | Segment CRUD; rule-based + list-based membership evaluation; ScyllaDB-backed list entry storage | Binary |
| `stitchd-analytics-service` | Event-definition CRUD + ingestion gRPC (multi-context `Array(Tuple(String, String))` rows in ClickHouse `events_v2`); metric-definition CRUD + ClickHouse-backed preview (`POST /v1/metrics/{id}/preview` via `dispatch_preview_query`); owns `experiment_results` in ClickHouse | Binary |
| `stitchd-experimentation-service` | Experiment lifecycle; reads pre-computed results from ClickHouse `experiment_results` table; experiments now reference `metric_ids` (cutover migration `20260520000002`) | Binary |
| `stitchd-stats-service` | Scheduled stats computation (60-min interval); gRPC-only consumer; writes pre-aggregated results to ClickHouse `experiment_results`. Exposes pure query builders under `queries::{aggregation, ratio, funnel, preview}` (experiment-scoped vs day-bucketed preview); shared `jsonlogic_to_sql` translator for metric `where_clause` filters | Binary |
| `stitchd-event-writer` | ClickHouse event ingestion and migration helpers (library; replaces retired `stitchd-events` crate name) | Library |
| `stitchd-sdk-rust` | Server-side Rust SDK — in-process flag evaluation (library; naming convention: `stitchd-sdk-{lang}`) | Library |
| `stitchd-core` | Domain model, rule engine, segmentation logic, hashing, ID types | Library |
| `stitchd-db` | Database access layer (sqlx repositories + ClickHouse) | Library |
| `stitchd-proto` | Protobuf definitions and generated tonic stubs for all services | Library |
| `xtask` | Build tool: mdBook docs generation, tool installation | Binary |

Internal communication is exclusively gRPC (tonic). `stitchd-server` (previous monolith) has been removed. The `stitchd-events` crate was renamed to `stitchd-event-writer` as part of the `boundaries_20260518` refactor; all references to the old name are retired.

## Backend

| Layer | Technology |
|---|---|
| Language | Rust 2024 — workspace MSRV = 1.95 (`workspace.package.rust-version`); both `rust-toolchain.toml` and CI's `dtolnay/rust-toolchain@stable` lines stay on `stable` so toolchain releases pick up automatically |
| REST API | Axum 0.8 (in `stitchd-gateway`) |
| Internal RPC | gRPC (tonic 0.14 + tonic-prost 0.14 + prost 0.14 — codec split since 0.14; `tonic_prost_build::configure()` in build scripts) |
| Config / Flag Store | PostgreSQL 16+ (sqlx 0.8) — offline cache (`.sqlx/`) for compile-time safety in CI |
| DB Extensions | pg_partman (for segment list partitioning) |
| List-Entry Store | ScyllaDB 6+ (scylla 1.6, Cassandra-compatible CQL) — wide-row tables per segment; LWT-based generation swap; keyspace renamed `stitchd_segments` (was `stitchd`) |
| Events / Experiments Store | ClickHouse 24+ via `clickhouse 0.15` driver (insert API is async + generic over `<Row>`) |
| Human Auth | JWT (jsonwebtoken 10) + OAuth2/OIDC (openidconnect 4 — endpoint type-state) + SAML 2.0 (quick-xml 0.40 + flate2) |
| SDK Auth | SDK Key — scoped to project + environment; min 1 active enforced; Project Admin manages create/revoke |
| MFA | TOTP via totp-rs 5 (secrets AES-256-GCM encrypted with aes-gcm 0.10) |
| Password Hashing | Argon2id (argon2 0.5) |
| Email Delivery | lettre 0.11 (SMTP); offline link fallback when SMTP unconfigured |
| Rate Limiting | governor 0.10 + tower_governor 0.8; SmartIpKeyExtractor (x-forwarded-for / x-real-ip / peer) |
| Observability | OpenTelemetry (0.28) + Prometheus (metrics-exporter-prometheus 0.16) |

### Environment Variable Naming Convention

All Stitchd-owned environment variables carry the `STITCHD_` prefix (the sole exception is `RUST_LOG`, which follows the Rust ecosystem standard). Service ports follow a standard pattern:

| Pattern | Example |
|---|---|
| `STITCHD_{SERVICE}_GRPC_PORT` | `STITCHD_AUTH_SERVICE_GRPC_PORT=50051` |
| `STITCHD_{SERVICE}_METRICS_PORT` | `STITCHD_AUTH_SERVICE_METRICS_PORT=9091` |
| `STITCHD_GATEWAY_HTTP_PORT` | `STITCHD_GATEWAY_HTTP_PORT=8080` (gateway REST) |
| `STITCHD_GATEWAY_METRICS_PORT` | `STITCHD_GATEWAY_METRICS_PORT=9080` (gateway Prometheus) |

## Admin UI (Frontend)

Located in `admin/` at the workspace root. Built with:

| Layer | Technology |
|---|---|
| Framework | React 19 |
| Routing | React Router v7 (`react-router-dom ^7`) |
| Build Tool | Vite 8 |
| Language | TypeScript 6 (`verbatimModuleSyntax: true` — requires `import type` for type-only imports) |
| HTTP Client | Axios |
| Form Layer | Formik 2.x (`^2.4.9`) + Yup 1.x (`^1.7.1`) — all admin forms use `<Formik>` + Yup schema; primitives in `admin/src/components/form/`; schemas in `admin/src/lib/validation/` |
| Dev Proxy | Vite server proxy: `/api → http://localhost:8080` (strips `/api` prefix, `changeOrigin: true`) |
| Linting | ESLint with `eslint-plugin-react-hooks` + `eslint-plugin-react-refresh` |
| Testing | Vitest `^4.1.6` + `@vitest/ui`; run with `npm test` (CI mode) or `npm run test:ui` |
| Package Name | `@stitchd/admin` |

Auth model: JWT decoded client-side (base64 payload only) to extract `is_system`. `org_id` comes from the login response body. Superadmin users (`is_system=true`) use `/superadmin/*` routes; org users use `/org/:orgId/*`.

## Pagination (REST API)

Shared offset pagination types live in `crates/stitchd-gateway/src/pagination.rs`:

- `PaginationParams` — extracted from `?page=N&per_page=N`; defaults page=1, per_page=50, cap=200. Uses a custom `de_u32_from_str` visitor to handle `serde_urlencoded` string coercion.
- `PaginatedResponse<T>` — wraps `{items, total, page, per_page}`
- Repository layer uses `COUNT(*) OVER()` window function to return total alongside items in one query

All list endpoints (`flags`, `segments`, `experiments`, `sdk_keys`, `org_users`) support pagination. Frontend components use URL-driven state (`useSearchParams` → `?page=N`).

## Context Intelligence Layer

Tables in PostgreSQL (`20260515000001_context_registry.sql`):
- `context_type_registry`: tracks observed context types per environment with `last_seen` timestamp
- `context_param_registry`: tracks parameter names + observed value samples per context type

Routes in `stitchd-gateway/src/routes/context_intel.rs`:
- `GET /v1/environments/{env_id}/context-types` — list observed context types
- `GET /v1/environments/{env_id}/context-types/{context_type}/params` — list parameters with value samples (autocomplete source for rule builder)

## Client SDK

| Layer | Technology |
|---|---|
| Initial SDK | Rust (server-side, in-process evaluation) |
| Definition Sync | gRPC (tonic + prost) — periodic polling via gateway passthrough |
| List Membership | REST (reqwest 0.13) — per-call fallback; optional LFU in-memory cache |
| Auth | SDK Key per environment (`x-sdk-key` on both gRPC metadata and REST header) |

## Serialization
- gRPC payloads: Protobuf via prost
- REST payloads: JSON (serde_json)
- SAML: XML via quick-xml 0.36 + flate2 (decompression)

## Key Dependencies

| Crate | Version | Purpose |
|---|---|---|
| `axum` | 0.8 | REST framework |
| `tonic` / `tonic-prost` / `prost` | 0.14 | gRPC — codec split (since tonic 0.14: `tonic-prost` is the prost codec, `tonic-prost-build` is the build helper) |
| `sqlx` | 0.8 | PostgreSQL async driver (offline-mode compile checks) |
| `clickhouse` | 0.15 | ClickHouse driver (`uuid`, `time`, `chrono`, `lz4` features; insert API is `async` + generic over `<Row>`) |
| `jsonwebtoken` | 10.4 | JWT issuance + verification |
| `openidconnect` | 4.0 | OIDC discovery + PKCE auth flow (endpoint type-state; reqwest::Client passed by reference) |
| `totp-rs` | 5.7 | TOTP secret generation + verification |
| `aes-gcm` | 0.10 | AES-256-GCM encryption (TOTP secrets, provider configs) |
| `argon2` | 0.5 | Argon2id password + recovery-code hashing |
| `lettre` | 0.11 | SMTP email delivery |
| `quick-xml` + `flate2` | 0.40 / 1.1 | SAML 2.0 XML processing |
| `governor` + `tower_governor` | 0.10 / 0.8 | Auth endpoint rate limiting |
| `secrecy` | 0.10 | Zero-on-drop secret wrapping |
| `siphasher` + `murmur3` + `sha2` | 1.0 / 0.5 / 0.11 | Consistent hashing (flag evaluation) |
| `scylla` | 1.6 | ScyllaDB async CQL driver (`metrics` feature enabled) |
| `utoipa` + `utoipa-axum` | 5.5 / 0.2 | OpenAPI 3.1 spec generation |
| `rand` / `reqwest` | 0.10 / 0.13 | RNG (`rand::rng()` + `RngExt::random_range`) / HTTP client (`rustls` + `form` + `http2` features; `default-features = false`) |
| Observability | `tracing-opentelemetry 0.33`, `opentelemetry* 0.32`, `metrics-exporter-prometheus 0.18` |

## Build Tools

| Tool | Purpose |
|---|---|
| `protoc-bin-vendored` | Bundles `protoc` binary as a build dependency — no system install required |
| `cargo-tarpaulin` | Code coverage (≥90% threshold enforced in CI) |
| `Swatinem/rust-cache` | GitHub Actions dependency caching |
| `sqlx-cli` | Database migration management |
| `xtask` (crate) | Single `cargo run --package xtask -- docs` command: protoc-gen-doc → OpenAPI export → rustdoc copy → mdbook build |
| `mdBook` + `mdbook-mermaid` | Static documentation site in `docs/`; built by xtask; deployed to GitHub Pages |
| `utoipa` + `utoipa-axum` | OpenAPI 3.1 spec generation from `#[utoipa::path]` annotations on Axum routes |
| `protoc-gen-doc` | Generates Markdown from `.proto` files into `docs/src/grpc/` |

## Scheduler Pattern (stitchd-stats-service)

`tokio::time::interval` drives the 60-minute stats computation loop:

```rust
let mut ticker = tokio::time::interval(scheduler_interval);
loop {
    ticker.tick().await;          // first tick fires immediately
    // fetch running experiments, spawn per-experiment compute tasks
}
```

Key invariants:
- `ticker.tick()` fires once immediately on entry, then every `scheduler_interval`
- `chrono::Duration` is NOT `std::time::Duration`; convert via `.to_std().unwrap()`
- On-demand recompute is handled by the gRPC `TriggerRecompute` RPC (spawns a task, returns job_id)
- `stats_schedule.last_computed_at` is the authoritative staleness signal; results are stale when it is >60 min old or absent

## Caching
- **SDK Key Cache** (`stitchd-auth-service`): `moka 0.12` async `Cache<String, SdkKey>` keyed on `key_hash`, TTL = 60 s.
  - `SdkKeyCache::get_or_load(hash, loader)` — cache hit skips DB; miss coalesces concurrent callers to one DB round-trip.
  - Invalidated eagerly on revocation via `SdkKeyCache::invalidate(hash)`.

## PostgreSQL Index Layer

Added in `db_optim_20260516` (`crates/stitchd-db/migrations/2026051600000{1-4}_*.sql`):

| Migration | Index | Purpose |
|---|---|---|
| 000001 | `idx_sdk_keys_key_hash_active` on `(key_hash, is_active)` | Fast SDK key auth lookup |
| 000002 | 6 partial indexes `WHERE deleted_at IS NULL` on flags, segments, projects, environments, event_definitions, experiments | Soft-delete query pruning |
| 000003 | `idx_segment_list_entries_covering` on `(segment_id, context_type, list_type, entry_key)` | Covering index for membership checks |
| 000004 | `idx_context_type/param_registry_last_seen` | Enables efficient purge of stale context registry entries |

Production deploys must run `CREATE INDEX CONCURRENTLY` manually outside a transaction.

### Events + Metrics migrations (`events_metrics_20260519`)

| Migration | Change | Purpose |
|---|---|---|
| `20260519000001_drop_experiment_results.sql` | Drop PostgreSQL `experiment_results` table | Source of truth moved to ClickHouse |
| `20260520000001_metric_definitions.sql` | Create `metric_definitions(id, environment_id, key, name, description, kind TEXT, config JSONB, goal_direction TEXT, version BIGINT, created_at, updated_at, deleted_at)` | Composable metric primitives table |
| `20260520000002_experiment_metrics_cutover.sql` | Add `metric_ids UUID[]` column to `experiments`; backfill from prior raw `event_key` references | Experiments → metric_ids cutover |
| `20260520000003_experiment_iterations_metric_ids.sql` | Add `metric_ids UUID[]` to `experiment_iterations` | Per-iteration metric pinning |
| `20260520000004_event_definitions_admin_fields.sql` | Add `name`, `description`, `metric_type TEXT` CHECK-constrained, `schema JSONB` columns to `event_definitions`; partial index on metric_type | Admin UI surface for event registration + JSON-schema validation |

## ClickHouse Schema

**Tables and materialized views as of 2026-05-22 (post-`experimentation_full_20260521`):**

| Table | Engine | Notes |
|---|---|---|
| `events` | MergeTree, monthly partitions | Legacy ingestion table |
| `events_v2` | MergeTree, weekly `toMonday()` partitions | Optimized partition granularity (migration 000007). `contexts Array(Tuple(String, String))` carries multi-context attribution per firing; `metric_key LowCardinality(String)`, three nullable typed value columns (`value_bool / value_int / value_double`); `properties Map(String, String)`; `timestamp DateTime64(3, 'UTC')` + `occurred_at DateTime64(3, 'UTC')` |
| `flag_evaluation_log_v2` | MergeTree, weekly `toMonday()` partitions + TTL | Eval log (migration `0004_flag_evaluation_log_v2.sql`). New columns (`experimentation_full_20260521`): `targeting_on Bool` (renamed from `is_disabled`) + `matched_rule_id Nullable(UUID)` — drive `experiment_assignments_mv` row routing |
| `events_experiment_daily` | AggregatingMergeTree | Pre-aggregated experiment stats by `(env_id, experiment_id, variant_key, metric_key, day)` |
| `events_experiment_daily_mv` | Materialized View | Auto-populates `events_experiment_daily` on `events` insert using `*State` combiners |
| `experiment_results` | MergeTree | Pre-computed per-experiment results; owned by `stitchd-analytics-service`; written by `stitchd-stats-service`; replaces the retired PostgreSQL `experiment_results` table (PG drop migration `20260519000001_drop_experiment_results.sql`). `experimentation_full_20260521` migration adds `context_type LowCardinality(String) DEFAULT 'user'` so per-context-type results sit on the same row shape |
| `experiment_assignments` | ReplacingMergeTree(`_version`) | **NEW** (`experimentation_full_20260521`). First-exposure (ITT) assignments keyed on `(experiment_id, iteration_id, context_type, context_key)`. Inverted version column (`-toUnixTimestamp64Milli(assigned_at)`) so MAX(_version) returns the MIN(assigned_at) — first exposure wins. Monthly partitions, 180-day TTL |
| `experiment_assignments_mv` | Materialized View | **NEW** (`experimentation_full_20260521`). Watches `flag_evaluation_log` inserts; routes rows where `targeting_on = true AND dictHas('experiment_iterations_active', (env_id, flag_id, matched_rule_id, context_type))` into `experiment_assignments` |

**ClickHouse dictionaries:**

| Dictionary | Source | Keys | Notes |
|---|---|---|---|
| `experiment_iterations_active` | PG `experiment_iterations_active` view | `(env_id UUID, flag_id UUID, matched_rule_id Nullable(UUID), context_type String)` | **NEW** (`experimentation_full_20260521`). Returns `(experiment_id UUID, iteration_id UUID)`. `LIFETIME(MIN 300 MAX 600)` + explicit `SYSTEM RELOAD DICTIONARY` on every iteration start/stop. Cardinality bounded by `count(running_experiments) * sum(unit_context_types)` |

**AggregatingMergeTree invariants:**
- Insert: use `*State` combiners (`countState()`, `sumState(Float64)`, `uniqState()`)
- Read: use `*Merge` combiners (`countMerge`, `sumMerge`, `uniqMerge`) in GROUP BY — NOT `finalizeAggregation` (scalar only)
- `sumState(Nullable(Float64))` mismatches `AggregateFunction(sum, Float64)` — wrap with `ifNull(..., 0.0)`

**ReplacingMergeTree first-exposure pattern (new):**
- `experiment_assignments` uses `ReplacingMergeTree(_version)` where `_version = -toUnixTimestamp64Milli(assigned_at)`. MAX(_version) during merges → MIN(assigned_at) → first-exposure variant wins per `(experiment_id, iteration_id, context_type, context_key)`.
- Readers MUST use `FINAL` (or `argMin(...)` GROUP BY) to collapse the unmerged window between INSERT and merge.
- Tests force determinism with `OPTIMIZE TABLE experiment_assignments FINAL` before assertion.

### Experimentation migrations (`experimentation_full_20260521`)

PG migrations:

| Migration | Change |
|---|---|
| `20260521000001_experiment_attribution_fields.sql` | Add `targets_default_rule Boolean`, `guardrail_metric_ids UUID[]`, `pre_period_days Integer`, `unit_context_types text[] NOT NULL DEFAULT '{user}'`, `flag_id UUID NOT NULL` to `experiments`; XOR `CHECK ((flag_rule_id IS NOT NULL AND targets_default_rule = false) OR (flag_rule_id IS NULL AND targets_default_rule = true))`; replace `idx_experiments_one_active_per_rule` with `idx_experiments_one_active_per_flag` |
| `20260521000002_flag_default_rule_distribution.sql` | Add `default_rule_distribution Jsonb` to `feature_flags` |
| `20260521000003_experiment_iterations_snapshot.sql` | Add `targets_default_rule`, `unit_context_types`, `default_rule_distribution` snapshot columns to `experiment_iterations` (so restart-with-changes captures the new config) |

CH migrations (under `stitchd-event-writer/migrations/`):

| Migration | Change |
|---|---|
| `20260521000001_flag_eval_log_matched_rule.sql` | Add `targeting_on Bool` + `matched_rule_id Nullable(UUID)` to `flag_evaluation_log`. Rename pattern: MATERIALIZE COLUMN → MODIFY COLUMN DEFAULT → DROP COLUMN to break DEFAULT-expression dependency on the old `is_disabled` |
| `20260521000002_experiment_iterations_active_dict.sql` | Create the `experiment_iterations_active` PG-backed dictionary |
| `20260521000003_experiment_assignments_mv.sql` | Create `experiment_assignments` table + `experiment_assignments_mv` materialized view |
| `20260521000004_backfill_experiment_assignments.sql` | One-shot 90-day backfill from `flag_evaluation_log` via the dictionary |
| `20260521000005_experiment_results_context_type.sql` | Add `context_type LowCardinality(String) DEFAULT 'user'` to `experiment_results` (per-context-type stats) |

## Infrastructure (Self-Hosted)
- PostgreSQL 16+ for configuration, tenants, RBAC, audit logs, auth, experiments
- ClickHouse 24+ for events, experiment data, metric aggregations
- ScyllaDB 6+ for list-segment entry storage (wide rows, million-scale per segment)
- Docker Compose orchestrates all service containers with health-checked dependencies (scylladb service added in `segment_scylla_20260516`)
