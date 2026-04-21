# Tech Stack
<!-- Last refreshed: 2026-04-22 -->

## Architecture

The system is decomposed into six Cargo workspace crates, each a standalone gRPC microservice, fronted by a REST gateway:

| Crate | Role |
|---|---|
| `stitchd-gateway` | REST API facade — translates JSON ↔ gRPC, calls all domain services; hosts OpenAPI spec |
| `stitchd-auth-service` | JWT / SDK-key credential validation; RBAC context assembly |
| `stitchd-flag-service` | Flag + variant CRUD; server-streaming definition sync for SDK |
| `stitchd-segmentation-service` | Segment CRUD; rule-based + list-based membership evaluation |
| `stitchd-event-service` | Event definition registry; ClickHouse ingestion gRPC |
| `stitchd-experimentation-service` | Experiment lifecycle; reads pre-computed results from PostgreSQL |

Internal communication is exclusively gRPC (tonic). `stitchd-server` (previous monolith) has been removed.

## Backend

| Layer | Technology |
|---|---|
| Language | Rust 2024 |
| REST API | Axum 0.8 (in `stitchd-gateway`) |
| Internal RPC | gRPC (tonic 0.13 + prost 0.13) |
| Config / Flag Store | PostgreSQL 16+ (sqlx 0.8) — offline cache (`.sqlx/`) for compile-time safety in CI |
| DB Extensions | pg_partman (for segment list partitioning) |
| Events / Experiments Store | ClickHouse 24+ |
| Human Auth | JWT (jsonwebtoken 9) + OAuth2/OIDC (openidconnect 3) + SAML 2.0 (quick-xml 0.36 + flate2) |
| SDK Auth | SDK Key — scoped to project + environment; min 1 active enforced; Project Admin manages create/revoke |
| MFA | TOTP via totp-rs 5 (secrets AES-256-GCM encrypted with aes-gcm 0.10) |
| Password Hashing | Argon2id (argon2 0.5) |
| Email Delivery | lettre 0.11 (SMTP); offline link fallback when SMTP unconfigured |
| Rate Limiting | governor 0.10 + tower_governor 0.8; SmartIpKeyExtractor (x-forwarded-for / x-real-ip / peer) |
| Observability | OpenTelemetry (0.28) + Prometheus (metrics-exporter-prometheus 0.16) |

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

## Infrastructure (Self-Hosted)
- PostgreSQL 16+ for configuration, tenants, RBAC, audit logs, auth, experiments
- ClickHouse 24+ for events, experiment data, metric aggregations
- Docker Compose orchestrates all six service containers with health-checked dependencies
