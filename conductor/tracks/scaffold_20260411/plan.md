# Plan: Workspace Scaffold & Project Foundation

## Phase 1: Cargo Workspace & Crate Skeleton

- [x] Task: Create root `Cargo.toml` with workspace definition and shared `[workspace.dependencies]`
  - Include: `tokio`, `axum`, `tonic`, `prost`, `sqlx`, `serde`, `tracing`, `thiserror`, `anyhow`, `uuid`, `chrono`
  - Pin versions; set `resolver = "3"`, `edition = "2024"`
- [x] Task: Create `stitchd-core` crate (`crates/stitchd-core/`)
- [x] Task: Create `stitchd-db` crate (`crates/stitchd-db/`)
- [x] Task: Create `stitchd-events` crate (`crates/stitchd-events/`)
- [x] Task: Create `stitchd-proto` crate (`crates/stitchd-proto/`)
- [x] Task: Create `stitchd-sdk` crate (`crates/stitchd-sdk/`)
- [x] Task: Create `stitchd-server` crate (`crates/stitchd-server/`)
- [x] Task: Verify `cargo build --workspace` succeeds with zero warnings
- [x] Task: Conductor - User Manual Verification 'Phase 1: Cargo Workspace & Crate Skeleton' (Protocol in workflow.md)

## Phase 2: Toolchain & Lint Configuration

- [x] Task: Create `rust-toolchain.toml`
- [x] Task: Create `rustfmt.toml` (stable-only options; nightly options documented as comments)
- [x] Task: Create `clippy.toml` (MSRV 1.85, cognitive complexity 15)
- [x] Task: Create `.cargo/config.toml` (`-D warnings` globally, release profile with LTO)
- [x] Task: Add `#![deny(warnings, missing_docs, clippy::all)]` + `#![warn(clippy::pedantic, clippy::nursery)]` to every `lib.rs`
- [x] Task: Verify `cargo fmt --check` and `cargo clippy -- -D warnings` both pass clean
- [x] Task: Conductor - User Manual Verification 'Phase 2: Toolchain & Lint Configuration' (Protocol in workflow.md)

## Phase 3: Protobuf File Structure

- [x] Task: Create `proto/` directory structure
  - `proto/common/v1/context.proto` — `Context`, `ParameterValue` (oneof all supported types)
  - `proto/flags/v1/flag_sync.proto` — `FeatureFlag`, `FlagSyncService.Sync` RPC
  - `proto/segments/v1/segment.proto` — `ListSegment`, `RuleSegment`, `SegmentBundle`
  - `proto/events/v1/event.proto` — `Event`, `EventService.Ingest` RPC
- [x] Task: Wire `stitchd-proto/build.rs` with `tonic-build::compile_protos` + vendored protoc
- [x] Task: Re-export generated types from `stitchd-proto/src/lib.rs`
- [x] Task: Verify `cargo build -p stitchd-proto` succeeds and generated types are importable
- [x] Task: Conductor - User Manual Verification 'Phase 3: Protobuf File Structure' (Protocol in workflow.md)

## Phase 4: Local Dev Infrastructure

- [x] Task: Create `docker-compose.yml` at workspace root (postgres:16-alpine + clickhouse:24-alpine, health checks, named volumes, `stitchd` bridge network)
- [x] Task: Create `.env.example` documenting all required env vars
- [x] Task: Add `.env` to `.gitignore`
- [x] Task: Verify `docker compose up -d` brings up both services healthy
- [x] Task: Conductor - User Manual Verification 'Phase 4: Local Dev Infrastructure' (Protocol in workflow.md)

## Phase 5: GitHub Actions CI Pipeline

- [x] Task: Create `.github/workflows/ci.yml` (fmt, clippy, test, coverage jobs; `Swatinem/rust-cache@v2`; `SQLX_OFFLINE=true`)
- [x] Task: Push branch and verify all CI jobs pass green (pending GitHub remote)
- [x] Task: Conductor - User Manual Verification 'Phase 5: GitHub Actions CI Pipeline' (Protocol in workflow.md)

## Phase 6: Observability Bootstrap

- [x] Task: Add `tracing` + `tracing-subscriber` (JSON/pretty based on `APP_ENV`, `RUST_LOG` controlled)
- [x] Task: Add OpenTelemetry tracer provider stub (OTLP wiring deferred — `Resource::new` private in sdk 0.28)
- [x] Task: Add Prometheus metrics endpoint (`/metrics` via `metrics-exporter-prometheus`, `/health` liveness probe)
- [x] Task: Verify server starts, logs JSON on stdout, `/metrics` returns 200
- [x] Task: Conductor - User Manual Verification 'Phase 6: Observability Bootstrap' (Protocol in workflow.md)
