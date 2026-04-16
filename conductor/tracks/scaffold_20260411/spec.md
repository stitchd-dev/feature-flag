# Spec: Workspace Scaffold & Project Foundation

## Goal

Establish the complete Rust workspace, crate skeleton, toolchain configuration, CI pipeline,
local dev infrastructure, Protobuf file structure, and observability bootstrap. This track
produces no business logic — it is the foundation every subsequent track builds on.

## Background

The platform is a multi-module Feature Flagging & Experimentation system written in Rust.
It exposes a REST Admin API (Axum) and a gRPC SDK API (tonic). Data is stored in PostgreSQL
(config) and ClickHouse (events). The client SDK is also Rust. All components live in a
single Cargo workspace.

## Scope

### In Scope

**Cargo Workspace**
- Root `Cargo.toml` defining the workspace and shared dependency versions
- The following crates, each as a library (unless noted), with stub `lib.rs` / `main.rs`:
  - `stitchd-core` — domain types, rule engine, shared models (lib)
  - `stitchd-server` — Axum REST + tonic gRPC server (bin + lib)
  - `stitchd-db` — sqlx queries, migrations, repository layer (lib)
  - `stitchd-events` — ClickHouse event ingestion (lib)
  - `stitchd-sdk` — Rust client SDK (lib)
  - `stitchd-proto` — prost-generated Protobuf types + tonic stubs (lib)

**Toolchain Configuration**
- `rust-toolchain.toml` pinning stable channel + Rust 2024 edition
- `rustfmt.toml` — idiomatic formatting rules
- `clippy.toml` — pedantic lint configuration
- `#![deny(warnings, missing_docs, clippy::all)]` in each lib root
- `.cargo/config.toml` — workspace-level cargo settings (e.g. linker, build flags)

**Proto File Structure**
- `proto/` directory with package-namespaced `.proto` files:
  - `proto/common/v1/context.proto` — Context, ParameterValue types
  - `proto/flags/v1/flag_sync.proto` — SDK flag sync service skeleton
  - `proto/segments/v1/segment.proto` — Segment types skeleton
  - `proto/events/v1/event.proto` — Event ingestion service skeleton
- `stitchd-proto/build.rs` wiring `prost-build` + `tonic-build`

**Local Dev Infrastructure**
- `docker-compose.yml` at workspace root:
  - PostgreSQL 16 service with health check
  - ClickHouse 24 service with health check
  - Named volumes for persistence
  - `.env.example` documenting all required env vars

**GitHub Actions CI**
- `.github/workflows/ci.yml`:
  - Triggers: push to any branch, PRs to `main`
  - Jobs (all run on `ubuntu-latest`):
    1. `fmt` — `cargo fmt --check`
    2. `clippy` — `cargo clippy -- -D warnings`
    3. `test` — `cargo test --workspace`
    4. `coverage` — `cargo-tarpaulin` with 90% minimum threshold
  - Caching: `Swatinem/rust-cache` action
  - Matrix: single stable toolchain

**Observability Bootstrap**
- `tracing` + `tracing-subscriber` wired in `stitchd-server/src/main.rs`
- OpenTelemetry exporter stub (OTLP via `opentelemetry-otlp`)
- Prometheus metrics endpoint stub (`/metrics`) via `axum-prometheus` or `metrics-exporter-prometheus`
- Log format: JSON in production, pretty in dev (controlled by `APP_ENV` env var)

### Out of Scope
- Any business logic (domain types, rule engine, flag evaluation)
- Database migrations (schema defined in later tracks)
- Auth middleware
- Any actual gRPC or REST endpoint implementation beyond stubs

## Acceptance Criteria

- [ ] `cargo build --workspace` succeeds with zero warnings
- [ ] `cargo fmt --check` passes across entire workspace
- [ ] `cargo clippy -- -D warnings` passes across entire workspace
- [ ] `cargo test --workspace` passes (stub tests exist in each crate)
- [ ] `docker compose up -d` brings up PostgreSQL and ClickHouse successfully
- [ ] `cargo run -p stitchd-server` starts without panic; logs a startup message
- [ ] CI pipeline passes on a fresh branch push
- [ ] Proto files compile via `build.rs` and generated types are importable
- [ ] Prometheus `/metrics` endpoint responds with 200
- [ ] All crate `lib.rs` roots have `#![deny(warnings, missing_docs, clippy::all)]`
