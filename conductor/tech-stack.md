# Tech Stack
<!-- Last refreshed: 2026-05-19 -->

## Architecture

The system is decomposed into seven Cargo workspace crates, each a standalone gRPC microservice, fronted by a REST gateway. Two library crates (`stitchd-event-writer`, `stitchd-sdk-rust`) support services and SDK consumers respectively:

| Crate | Role | Type |
|---|---|---|
| `stitchd-gateway` | REST API facade — translates JSON ↔ gRPC, calls all domain services; hosts OpenAPI spec; serves real Prometheus metrics at `GET /v1/metrics` | Binary |
| `stitchd-auth-service` | JWT / SDK-key credential validation; RBAC context assembly | Binary |
| `stitchd-flag-service` | Flag + variant CRUD; server-streaming definition sync for SDK | Binary |
| `stitchd-segmentation-service` | Segment CRUD; rule-based + list-based membership evaluation; ScyllaDB-backed list entry storage | Binary |
| `stitchd-analytics-service` | Event definition registry; ClickHouse ingestion gRPC (owns `experiment_results` in ClickHouse) | Binary |
| `stitchd-experimentation-service` | Experiment lifecycle; reads pre-computed results from ClickHouse `experiment_results` table | Binary |
| `stitchd-stats-service` | Scheduled stats computation (60-min interval); gRPC-only consumer; writes pre-aggregated results to ClickHouse `experiment_results` table (PG table dropped in `20260519000001_drop_experiment_results.sql`) | Binary |
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
| Language | Rust 2024 |
| REST API | Axum 0.8 (in `stitchd-gateway`) |
| Internal RPC | gRPC (tonic 0.13 + prost 0.13) |
| Config / Flag Store | PostgreSQL 16+ (sqlx 0.8) — offline cache (`.sqlx/`) for compile-time safety in CI |
| DB Extensions | pg_partman (for segment list partitioning) |
| List-Entry Store | ScyllaDB 6+ (scylla 1.5, Cassandra-compatible CQL) — wide-row tables per segment; LWT-based generation swap; keyspace renamed `stitchd_segments` (was `stitchd`) |
| Events / Experiments Store | ClickHouse 24+ (owns `experiment_results` table; PG version retired) |
| Human Auth | JWT (jsonwebtoken 9) + OAuth2/OIDC (openidconnect 3) + SAML 2.0 (quick-xml 0.36 + flate2) |
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
| List Membership | REST (reqwest 0.12) — per-call fallback; optional LFU in-memory cache |
| Auth | SDK Key per environment (`x-sdk-key` on both gRPC metadata and REST header) |

## Serialization
- gRPC payloads: Protobuf via prost
- REST payloads: JSON (serde_json)
- SAML: XML via quick-xml 0.36 + flate2 (decompression)

## Key Dependencies

| Crate | Version | Purpose |
|---|---|---|
| `axum` | 0.8 | REST framework |
| `tonic` / `prost` | 0.13 | gRPC |
| `sqlx` | 0.8 | PostgreSQL async driver (offline-mode compile checks) |
| `clickhouse` | 0.13 | ClickHouse driver (`uuid`, `time`, `lz4` features; no `derive` feature) |
| `jsonwebtoken` | 9 | JWT issuance + verification |
| `openidconnect` | 3 | OIDC discovery + PKCE auth flow |
| `totp-rs` | 5 | TOTP secret generation + verification |
| `aes-gcm` | 0.10 | AES-256-GCM encryption (TOTP secrets, provider configs) |
| `argon2` | 0.5 | Argon2id password + recovery-code hashing |
| `lettre` | 0.11 | SMTP email delivery |
| `quick-xml` + `flate2` | 0.36 / 1 | SAML 2.0 XML processing |
| `governor` + `tower_governor` | 0.10 / 0.8 | Auth endpoint rate limiting |
| `secrecy` | 0.10 | Zero-on-drop secret wrapping |
| `siphasher` + `murmur3` + `sha2` | 1 / 0.5 / 0.10 | Consistent hashing (flag evaluation) |
| `scylla` | 1.5 | ScyllaDB async CQL driver (`metrics` feature enabled) |
| `utoipa` + `utoipa-axum` | 5 / 0.2 | OpenAPI 3.1 spec generation |

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

## ClickHouse Schema

**Tables and materialized views as of 2026-05-19 (post-`boundaries_20260518`):**

| Table | Engine | Notes |
|---|---|---|
| `events` | MergeTree, monthly partitions | Primary ingestion table |
| `events_v2` | MergeTree, weekly `toMonday()` partitions | Optimized partition granularity (migration 000007) |
| `flag_evaluation_log_v2` | MergeTree, weekly `toMonday()` partitions + TTL | Eval log (migration `0004_flag_evaluation_log_v2.sql`) |
| `events_experiment_daily` | AggregatingMergeTree | Pre-aggregated experiment stats by `(env_id, experiment_id, variant_key, metric_key, day)` |
| `events_experiment_daily_mv` | Materialized View | Auto-populates `events_experiment_daily` on `events` insert using `*State` combiners |
| `experiment_results` | MergeTree | Pre-computed per-experiment results; owned by `stitchd-analytics-service`; written by `stitchd-stats-service`; replaces the retired PostgreSQL `experiment_results` table (PG drop migration `20260519000001_drop_experiment_results.sql`) |

**AggregatingMergeTree invariants:**
- Insert: use `*State` combiners (`countState()`, `sumState(Float64)`, `uniqState()`)
- Read: use `*Merge` combiners (`countMerge`, `sumMerge`, `uniqMerge`) in GROUP BY — NOT `finalizeAggregation` (scalar only)
- `sumState(Nullable(Float64))` mismatches `AggregateFunction(sum, Float64)` — wrap with `ifNull(..., 0.0)`

## Infrastructure (Self-Hosted)
- PostgreSQL 16+ for configuration, tenants, RBAC, audit logs, auth, experiments
- ClickHouse 24+ for events, experiment data, metric aggregations
- ScyllaDB 6+ for list-segment entry storage (wide rows, million-scale per segment)
- Docker Compose orchestrates all service containers with health-checked dependencies (scylladb service added in `segment_scylla_20260516`)
