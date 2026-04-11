# Track Learnings: scaffold_20260411

Patterns, gotchas, and context discovered during implementation.

## Codebase Patterns (Inherited)

<!-- No patterns yet - this is the first track -->

---

## [2026-04-11] - Phase 1 Task 1: Cargo workspace root

- **Implemented:** Root `Cargo.toml` with workspace resolver 3, all six crates as members, shared `[workspace.dependencies]`
- **Files changed:** `Cargo.toml`
- **Learnings:**
  - Gotchas: Rust 2024 edition requires `resolver = "3"` (not `"2"` as in 2021). Using `"2"` compiles but is semantically wrong.
  - Gotchas: `clickhouse` crate v0.13 has no `derive` feature — use `uuid`, `time`, `lz4` instead.

---

## [2026-04-11] - Phase 2 Task 2: rustfmt.toml

- **Implemented:** Stable-only rustfmt config
- **Files changed:** `rustfmt.toml`
- **Learnings:**
  - Gotchas: Many popular `rustfmt.toml` options (`imports_granularity`, `group_imports`, `wrap_comments`, `normalize_comments`, `hex_literal_case`) are **nightly-only** and emit warnings on stable without taking effect. Strip them from stable toolchain configs.

---

## [2026-04-11] - Phase 3 Task 2: stitchd-proto build.rs

- **Implemented:** `tonic-build` + vendored `protoc` via `protoc-bin-vendored`
- **Files changed:** `crates/stitchd-proto/build.rs`, `Cargo.toml`
- **Learnings:**
  - Patterns: Use `protoc-bin-vendored` as a build-dep to avoid a system `protoc` install requirement. No CI or dev machine setup needed.
  - Gotchas: `std::env::set_var` is **unsafe** in Rust 2024 edition (potential data race in multi-threaded contexts). Always wrap in `unsafe { }` with a `// SAFETY:` comment in build scripts.

---

## [2026-04-11] - Phase 6 Task 1: OpenTelemetry setup

- **Implemented:** `SdkTracerProvider` wired to `tracing-opentelemetry` layer; Prometheus `/metrics` endpoint
- **Files changed:** `crates/stitchd-server/src/telemetry.rs`, `src/lib.rs`, `src/main.rs`
- **Learnings:**
  - Gotchas: `tracing-opentelemetry 0.29` depends on `opentelemetry ^0.28`, while `opentelemetry-otlp 0.27` depends on `opentelemetry ^0.27` — these are **incompatible types**. Pin all OTel crates to the same minor version.
  - Gotchas: `opentelemetry_sdk 0.28` made `Resource::new()` **private**. Must use the builder API (`Resource::builder()`) — exact method names to be confirmed when wiring OTLP in a future track.
  - Patterns: `metrics-exporter-prometheus::PrometheusBuilder::new().install_recorder()` installs a global recorder and returns a `PrometheusHandle`. Pass the handle as Axum `State` and call `handle.render()` in the `/metrics` handler.
  - Patterns: Graceful shutdown via `tokio::select!` on `ctrl_c` + `SIGTERM` (unix only, gated with `#[cfg(unix)]`) is the idiomatic Tokio pattern.
