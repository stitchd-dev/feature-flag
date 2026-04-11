# Plan: Workspace Scaffold & Project Foundation

## Phase 1: Cargo Workspace & Crate Skeleton

- [ ] Task: Create root `Cargo.toml` with workspace definition and shared `[workspace.dependencies]`
  - Include: `tokio`, `axum`, `tonic`, `prost`, `sqlx`, `serde`, `tracing`, `thiserror`, `anyhow`, `uuid`, `chrono`
  - Pin versions; set `resolver = "2"`, `edition = "2024"`
- [ ] Task: Create `stitchd-core` crate (`crates/stitchd-core/`)
  - `Cargo.toml`, `src/lib.rs` with deny/warn attributes and a `// TODO: domain types` comment
  - Stub test: `#[test] fn it_compiles() {}`
- [ ] Task: Create `stitchd-db` crate (`crates/stitchd-db/`)
  - `Cargo.toml` (deps: `sqlx`, `tokio`, `thiserror`), `src/lib.rs` stub
  - Stub test
- [ ] Task: Create `stitchd-events` crate (`crates/stitchd-events/`)
  - `Cargo.toml` (deps: `clickhouse` client crate, `tokio`, `thiserror`), `src/lib.rs` stub
  - Stub test
- [ ] Task: Create `stitchd-proto` crate (`crates/stitchd-proto/`)
  - `Cargo.toml` (deps: `tonic`, `prost`), `src/lib.rs` stub
  - `build.rs` stub (wiring `tonic-build`)
- [ ] Task: Create `stitchd-sdk` crate (`crates/stitchd-sdk/`)
  - `Cargo.toml` (deps: `tonic`, `tokio`, `thiserror`), `src/lib.rs` stub
  - Stub test
- [ ] Task: Create `stitchd-server` crate (`crates/stitchd-server/`)
  - `Cargo.toml` (deps: `axum`, `tonic`, `tokio`, `tracing`, `tracing-subscriber`), `src/lib.rs` + `src/main.rs` stub
  - Stub test
- [ ] Task: Verify `cargo build --workspace` succeeds with zero warnings
- [ ] Task: Conductor - User Manual Verification 'Phase 1: Cargo Workspace & Crate Skeleton' (Protocol in workflow.md)

## Phase 2: Toolchain & Lint Configuration

- [ ] Task: Create `rust-toolchain.toml`
  - Pin `channel = "stable"`, `components = ["rustfmt", "clippy"]`
- [ ] Task: Create `rustfmt.toml`
  - `edition = "2024"`, `max_width = 100`, `imports_granularity = "Crate"`, `group_imports = "StdExternalCrate"`
- [ ] Task: Create `clippy.toml`
  - `msrv = "<current stable>"`, cognitive complexity threshold
- [ ] Task: Create `.cargo/config.toml`
  - `[build]` section: set `rustflags = ["-D", "warnings"]`
  - Linker config for faster builds (`mold` or `lld` if available)
- [ ] Task: Add `#![deny(warnings, missing_docs, clippy::all)]` + `#![warn(clippy::pedantic, clippy::nursery)]` to every `lib.rs`
- [ ] Task: Verify `cargo fmt --check` and `cargo clippy -- -D warnings` both pass clean
- [ ] Task: Conductor - User Manual Verification 'Phase 2: Toolchain & Lint Configuration' (Protocol in workflow.md)

## Phase 3: Protobuf File Structure

- [ ] Task: Create `proto/` directory structure
  - `proto/common/v1/context.proto` — package `stitchd.common.v1`; skeleton `Context` and `ParameterValue` messages
  - `proto/flags/v1/flag_sync.proto` — package `stitchd.flags.v1`; skeleton `FlagSyncService` with `Sync` RPC
  - `proto/segments/v1/segment.proto` — package `stitchd.segments.v1`; skeleton `Segment` message
  - `proto/events/v1/event.proto` — package `stitchd.events.v1`; skeleton `EventService` with `Ingest` RPC
- [ ] Task: Wire `stitchd-proto/build.rs` with `tonic-build::compile_protos` for all `.proto` files
- [ ] Task: Re-export generated types from `stitchd-proto/src/lib.rs`
- [ ] Task: Verify `cargo build -p stitchd-proto` succeeds and generated types are importable
- [ ] Task: Conductor - User Manual Verification 'Phase 3: Protobuf File Structure' (Protocol in workflow.md)

## Phase 4: Local Dev Infrastructure

- [ ] Task: Create `docker-compose.yml` at workspace root
  - `postgres` service: image `postgres:16`, health check, named volume, env vars from `.env`
  - `clickhouse` service: image `clickhouse/clickhouse-server:24`, health check, named volume
  - Networks: single `stitchd` bridge network
- [ ] Task: Create `.env.example` documenting all required env vars
  - `DATABASE_URL`, `CLICKHOUSE_URL`, `APP_ENV`, `JWT_SECRET`, `OTEL_EXPORTER_OTLP_ENDPOINT`
- [ ] Task: Add `.env` to `.gitignore`
- [ ] Task: Verify `docker compose up -d` brings up both services healthy
- [ ] Task: Conductor - User Manual Verification 'Phase 4: Local Dev Infrastructure' (Protocol in workflow.md)

## Phase 5: GitHub Actions CI Pipeline

- [ ] Task: Create `.github/workflows/ci.yml`
  - Trigger: `push` (all branches), `pull_request` (target `main`)
  - Job `fmt`: `cargo fmt --check`
  - Job `clippy`: `cargo clippy -- -D warnings`
  - Job `test`: `cargo test --workspace`
  - Job `coverage`: `cargo tarpaulin --workspace --fail-under 90`
  - All jobs use `Swatinem/rust-cache@v2` for dependency caching
  - `ubuntu-latest` runner, stable toolchain via `dtolnay/rust-toolchain@stable`
- [ ] Task: Push branch and verify all CI jobs pass green
- [ ] Task: Conductor - User Manual Verification 'Phase 5: GitHub Actions CI Pipeline' (Protocol in workflow.md)

## Phase 6: Observability Bootstrap

- [ ] Task: Add `tracing` + `tracing-subscriber` initialisation in `stitchd-server/src/main.rs`
  - JSON format when `APP_ENV=production`, pretty format otherwise
  - Log level controlled by `RUST_LOG` env var
- [ ] Task: Add OpenTelemetry OTLP exporter stub
  - Initialise tracer provider with `opentelemetry-otlp`
  - Wire `tracing-opentelemetry` layer into subscriber
  - Graceful shutdown on SIGTERM
- [ ] Task: Add Prometheus metrics endpoint
  - Mount `/metrics` route in Axum router
  - Export via `metrics-exporter-prometheus`
  - Add basic process metrics (uptime, request count stub)
- [ ] Task: Verify server starts, logs JSON on stdout, `/metrics` returns 200
- [ ] Task: Conductor - User Manual Verification 'Phase 6: Observability Bootstrap' (Protocol in workflow.md)
