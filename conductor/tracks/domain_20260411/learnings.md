# Track Learnings: domain_20260411

Patterns, gotchas, and context discovered during implementation.

## Codebase Patterns (Inherited)

### Code Conventions

- Rust 2024 edition requires `resolver = "3"` in workspace `Cargo.toml` (not `"2"`). (from: scaffold_20260411, 2026-04-11)
- `std::env::set_var` is **unsafe** in Rust 2024 — always wrap in `unsafe {}` with a `// SAFETY:` comment, even in build scripts. (from: scaffold_20260411, 2026-04-11)
- `rustfmt.toml` options like `imports_granularity`, `group_imports`, `wrap_comments`, `normalize_comments` are **nightly-only** — they silently no-op on stable without error. Strip them from stable configs. (from: scaffold_20260411, 2026-04-11)

### Architecture

- **Prometheus metrics:** Use `PrometheusBuilder::new().install_recorder()` to get a `PrometheusHandle`. Pass it as Axum `State` and call `handle.render()` in the `/metrics` route handler. (from: scaffold_20260411, 2026-04-11)
- **Graceful shutdown:** Use `tokio::select!` over `ctrl_c()` + `SIGTERM` (gated `#[cfg(unix)]`) as the shutdown signal. Pass to `axum::serve(...).with_graceful_shutdown(...)`. (from: scaffold_20260411, 2026-04-11)
- **Vendored protoc:** Add `protoc-bin-vendored` as a build dependency in `stitchd-proto` and set `PROTOC` env var in `build.rs`. Eliminates system `protoc` requirement. (from: scaffold_20260411, 2026-04-11)

### Gotchas

- `clickhouse` crate v0.13 has no `derive` feature — use `uuid`, `time`, `lz4` features instead. (from: scaffold_20260411, 2026-04-11)
- **OpenTelemetry version alignment:** `tracing-opentelemetry 0.29` requires `opentelemetry ^0.28`. `opentelemetry-otlp 0.27` requires `opentelemetry ^0.27`. Pin all OTel crates to same minor version. (from: scaffold_20260411, 2026-04-11)
- `opentelemetry_sdk 0.28` made `Resource::new()` private. Use `Resource::builder()`. (from: scaffold_20260411, 2026-04-11)

### Testing

- Axum router integration tests: use `tower::ServiceExt::oneshot` to send a single request through the router without starting a real TCP server. Add `tower` as a `[dev-dependencies]` entry. (from: scaffold_20260411, 2026-04-11)

---

## [2026-04-11 10:00] - Phase 1 Task 1: ID Newtypes
- **Implemented:** 15 UUID-based ID newtypes and a string-based `FlagKey` with validation.
- **Files changed:** `crates/stitchd-core/src/id.rs`, `crates/stitchd-core/src/lib.rs`, `crates/stitchd-core/Cargo.toml`
- **Commit:** [TBD]
- **Learnings:**
  - Patterns: Used `macro_rules!` to define repetitive ID newtypes with `sqlx::Type` and `sqlx(transparent)`.
  - Gotchas: Ensure `sqlx` is added to crate-level `Cargo.toml` when using `sqlx::Type` derive, even if defined in workspace.
---
