# Tech Stack
<!-- Last refreshed: 2026-05-30 (post domain_boundaries_20260530 — lean-gateway boundary enforced, canonical error mapping) -->

<!--
domain_boundaries_20260530 conventions (see conductor/patterns.md for the full set):
- Gateway carries ZERO domain logic: pure REST↔gRPC translation + cross-cutting
  (auth/rate-limit/quota) + multi-service orchestration. Every domain rule lives
  in its owning service behind gRPC.
- Canonical RepositoryError→tonic::Status mapping is available as
  `impl From<RepositoryError> for tonic::Status` in `stitchd-db` behind the
  optional `tonic` feature.
- Backward-compatible proto additions this track: MutateFlagRequest.enabled_override
  (proto3 optional), MutationKind.REPLACE_VARIANTS / REPLACE_RULES,
  TrackEventsRequest.mark_test (proto3 optional). No breaking contract changes.
-->


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
| `stitchd-sdk-rust` | Server-side Rust SDK — in-process flag evaluation via `SdkClient::evaluate(&[EvalRequest], TraceLevel)`, which delegates to `stitchd-core::evaluation::evaluate_flag` (library; naming convention: `stitchd-sdk-{lang}`) | Library |
| `stitchd-core` | Domain model, rule engine, segmentation logic, hashing, ID types. Hosts the SOLE flag-evaluation orchestrator `evaluation::evaluate_flag(...)` (post-`flag_eval_unify_20260522`) — preview path + SDK path both delegate here. Owns the canonical `HashSelector` / `HashInputSpec` / `TraceLevel` / `ListMembershipIndex` / `EvalOutcome` / `EvaluationTrace` / `FlagEvaluationResult` types | Library |
| `stitchd-db` | Database access layer (sqlx repositories + ClickHouse) | Library |
| `stitchd-proto` | Protobuf definitions and generated tonic stubs for all services. `flags.v1.PercentageAllocation` carries the canonical `hash_inputs: repeated HashSelector` at tag 3 (post-`flag_eval_unify_20260522`); legacy `context_hash_specs` map at tag 1 is retired | Library |
| `xtask` | Build tool: mdBook docs generation, tool installation | Binary |

Internal communication is exclusively gRPC (tonic). `stitchd-server` (previous monolith) has been removed. The `stitchd-events` crate was renamed to `stitchd-event-writer` as part of the `boundaries_20260518` refactor; all references to the old name are retired.

## Backend

| Layer | Technology |
|---|---|
| Language | Rust 2024 — workspace MSRV = 1.95 (`workspace.package.rust-version`); both `rust-toolchain.toml` and CI's `dtolnay/rust-toolchain@stable` lines stay on `stable` so toolchain releases pick up automatically; rollout allocations use integer basis points (u32, `percentage_bp` where 10000 = 100%, 0.01% precision) |
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

Defined in Postgres V1 Baseline (`20260525000001_v1_baseline.sql`):

| Index | Purpose |
|---|---|
| `idx_sdk_keys_key_hash_active` on `(key_hash, is_active)` | Fast SDK key auth lookup |
| 6 partial indexes `WHERE deleted_at IS NULL` on flags, segments, projects, environments, event_definitions, experiments | Soft-delete query pruning |
| `idx_segment_list_entries_covering` on `(segment_id, context_type, list_type, entry_key)` | Covering index for membership checks |
| `idx_context_type/param_registry_last_seen` | Enables efficient purge of stale context registry entries |

Production deploys must run `CREATE INDEX CONCURRENTLY` manually outside a transaction.

## ClickHouse Schema

**Tables and materialized views as of 2026-05-27 (post-`schema_cutover_20260525`):**

| Table | Engine | Notes |
|---|---|---|
| `events` | MergeTree, monthly partitions | Legacy ingestion table |
| `events_v2` | MergeTree, weekly `toMonday()` partitions | Optimized partition granularity. `contexts Array(Tuple(String, String))` carries multi-context attribution per firing; `metric_key LowCardinality(String)`, three nullable typed value columns (`value_bool / value_int / value_double`); `properties Map(String, String)`; `timestamp DateTime64(3, 'UTC')` + `occurred_at DateTime64(3, 'UTC')` |
| `flag_evaluation_log` | MergeTree, weekly `toMonday()` partitions + TTL | Eval log. Columns: `env_id`, `flag_id`, `flag_key`, `variant_key`, `targeting_on` (Boolean, true when flag active), `matched_rule_id` (matched rule UUID), `evaluated_at`, `context_type`, `context_key`, `params_json` |
| `events_experiment_daily` | AggregatingMergeTree | Pre-aggregated experiment stats by `(env_id, experiment_id, variant_key, metric_key, day)` |
| `events_experiment_daily_mv` | Materialized View | Auto-populates `events_experiment_daily` on `events` insert using `*State` combiners |
| `experiment_results` | MergeTree | Pre-computed per-experiment results; owned by `stitchd-analytics-service`; written by `stitchd-stats-service`. `context_type` column (default 'user') supports per-context-type results |
| `experiment_assignments` | ReplacingMergeTree(`_version`) | First-exposure (ITT) assignments keyed on `(experiment_id, iteration_id, context_type, context_key)`. Inverted version column (`-toUnixTimestamp64Milli(assigned_at)`) so MAX(_version) returns the MIN(assigned_at) — first exposure wins. Monthly partitions, 180-day TTL |
| `experiment_assignments_mv` | Materialized View | Watches `flag_evaluation_log` inserts; routes rows where `targeting_on = true AND dictHas('experiment_iterations_active', (env_id, flag_id, matched_rule_id, context_type))` into `experiment_assignments` |
| `experiment_interactions` | MergeTree | Pairwise cross-experiment interaction results, keyed `(env_id, experiment_id_a, experiment_id_b, context_type, metric_key)`. Holds `shared_count`, `cell_stats` (JSON per Aᵥ×Bᵥ cell), `interaction_estimate`, `p_value`, `significant`, `computed_at`. Written by `stitchd-stats-service` interaction sweep (60-min tick); read by `stitchd-experimentation-service` `GetExperimentInteractions`. Added in `xexp_interaction_20260602` (auto-applied on boot via `event_writer::migrations` array). |

**ClickHouse dictionaries:**

| Dictionary | Source | Keys | Notes |
|---|---|---|---|
| `experiment_iterations_active` | PG `experiment_iterations_active` view | `(env_id UUID, flag_id UUID, matched_rule_id Nullable(UUID), context_type String)` | PG-backed active experiment iterations view source. Returns `(experiment_id UUID, iteration_id UUID)`. `LIFETIME(MIN 30 MAX 60)` + explicit `SYSTEM RELOAD DICTIONARY` on every iteration start/stop. |

**AggregatingMergeTree invariants:**
- Insert: use `*State` combiners (`countState()`, `sumState(Float64)`, `uniqState()`)
- Read: use `*Merge` combiners (`countMerge`, `sumMerge`, `uniqMerge`) in GROUP BY — NOT `finalizeAggregation` (scalar only)
- `sumState(Nullable(Float64))` mismatches `AggregateFunction(sum, Float64)` — wrap with `ifNull(..., 0.0)`

**ReplacingMergeTree first-exposure pattern:**
- `experiment_assignments` uses `ReplacingMergeTree(_version)` where `_version = -toUnixTimestamp64Milli(assigned_at)`. MAX(_version) during merges → MIN(assigned_at) → first-exposure variant wins per `(experiment_id, iteration_id, context_type, context_key)`.
- Readers MUST use `FINAL` (or `argMin(...)` GROUP BY) to collapse the unmerged window between INSERT and merge.
- Tests force determinism with `OPTIMIZE TABLE experiment_assignments FINAL` before assertion.

## Database Migrations (V1 Baselines)

All historical migrations (PostgreSQL, ClickHouse, ScyllaDB) were collapsed into single V1 baseline schemas during the `schema_cutover_20260525` track:

- **PostgreSQL:** Defined in `crates/stitchd-db/migrations/20260525000001_v1_baseline.sql` (plus a partial unique constraint fix in `20260525000002_fix_flag_key_unique_partial.sql`). Retired `segment_rules` table and `context_hash_specs` dual-write columns.
- **ClickHouse:** Defined in `crates/stitchd-event-writer/migrations/20260525000001_v1_baseline.sql`. Table `flag_evaluation_log_v2` renamed back to `flag_evaluation_log`.
- **ScyllaDB:** Defined in `crates/stitchd-db/scylla-migrations/0001_v1_baseline.cql`.

Post-baseline incremental migrations:
- `crates/stitchd-db/migrations/20260602000001_exclusion_groups.sql` (`xexp_interaction_20260602`): adds the `exclusion_groups` table (per-env, immutable `salt`, version/audit/soft-delete; partial unique index on `(env_id, name) WHERE deleted_at IS NULL`) and the nullable `exclusion_group_id` + `group_bucket_lo/hi` columns on `experiments` (CHECK `0 <= lo < hi <= 10000` or both NULL) and snapshot columns on `experiment_iterations`.
- `crates/stitchd-event-writer/migrations/20260602000002_experiment_interactions.sql` (`xexp_interaction_20260602`): the ClickHouse `experiment_interactions` table (registered in the `event_writer::migrations` MIGRATIONS array so it auto-applies on analytics-service boot).

The experimentation-service proto gained additive RPCs (`CreateExclusionGroup`/`ListExclusionGroups`/`UpdateExclusionGroup`/`DeleteExclusionGroup`/`AssignExperimentToGroup`/`UnassignExperiment`/`GetExperimentInteractions`); `flags.v1.PercentageAllocation` gained an additive `exclusion_gate` (group_salt + context_type + bucket_lo/hi) carried on the existing definition-sync path.

## Infrastructure (Self-Hosted)
- PostgreSQL 16+ for configuration, tenants, RBAC, audit logs, auth, experiments
- ClickHouse 24+ for events, experiment data, metric aggregations
- ScyllaDB 6+ for list-segment entry storage (wide rows, million-scale per segment)
- Docker Compose orchestrates all service containers with health-checked dependencies (scylladb service added in `segment_scylla_20260516`)
