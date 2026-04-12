# Codebase Patterns

Reusable patterns discovered during development. Read this before starting new work.

## Code Conventions

- Rust 2024 edition requires `resolver = "3"` in workspace `Cargo.toml` (not `"2"`). (from: scaffold_20260411, 2026-04-11)
- `std::env::set_var` is **unsafe** in Rust 2024 — always wrap in `unsafe {}` with a `// SAFETY:` comment, even in build scripts. (from: scaffold_20260411, 2026-04-11)
- `rustfmt.toml` options like `imports_granularity`, `group_imports`, `wrap_comments`, `normalize_comments` are **nightly-only** — they silently no-op on stable without error. Strip them from stable configs. (from: scaffold_20260411, 2026-04-11)

## Architecture

- **Prometheus metrics:** Use `PrometheusBuilder::new().install_recorder()` to get a `PrometheusHandle`. Pass it as Axum `State` and call `handle.render()` in the `/metrics` route handler. (from: scaffold_20260411, 2026-04-11)
- **Graceful shutdown:** Use `tokio::select!` over `ctrl_c()` + `SIGTERM` (gated `#[cfg(unix)]`) as the shutdown signal. Pass to `axum::serve(...).with_graceful_shutdown(...)`. (from: scaffold_20260411, 2026-04-11)
- **Vendored protoc:** Add `protoc-bin-vendored` as a build dependency in `stitchd-proto` and set `PROTOC` env var in `build.rs`. Eliminates system `protoc` requirement for all contributors and CI. (from: scaffold_20260411, 2026-04-11)

## Rust 2024 Edition Patterns

- When iterating a `HashMap` and only needing keys or values, use `.keys()` / `.values()` — `for (k, _) in map` triggers `clippy::for_kv_map` as a warning-level error with `-D warnings`. (from: rule_engine_20260412, 2026-04-12)
- In filter closures over iterator references (e.g. `.filter(|(_, d)| ...)`), the value `d` is `&&T` — dereference with `**d` or use `.filter(|&(_, d)| ...)`. Pattern-binding `&d` inside a non-reference outer pattern fails in Rust 2024. (from: rule_engine_20260412, 2026-04-12)

## Gotchas

- `clickhouse` crate v0.13 has no `derive` feature — use `uuid`, `time`, `lz4` features instead. (from: scaffold_20260411, 2026-04-11)
- **SQLx Offline Compilation:** `sqlx::query!` macros require a live DB or up-to-date `.sqlx` cache. New queries will break compilation in offline mode until `cargo sqlx prepare` is executed. (from: segmentation_20260412, 2026-04-12)
- **Database Extension Dependencies:** Call functions from extensions (like `pg_partman`) using plain `sqlx::query` to avoid macro-based compilation errors when extensions aren't available in the local build environment. (from: segmentation_20260412, 2026-04-12)
- **OpenTelemetry version alignment:** `tracing-opentelemetry 0.29` requires `opentelemetry ^0.28`. `opentelemetry-otlp 0.27` requires `opentelemetry ^0.27`. These produce **incompatible types** — pin all OTel crates to the same minor version. (from: scaffold_20260411, 2026-04-11)
- `opentelemetry_sdk 0.28` made `Resource::new()` private. Use `Resource::builder()` — confirm exact builder API before wiring OTLP in the observability track. (from: scaffold_20260411, 2026-04-11)

## Testing

- Axum router integration tests: use `tower::ServiceExt::oneshot` to send a single request through the router without starting a real TCP server. Add `tower` as a `[dev-dependencies]` entry. (from: scaffold_20260411, 2026-04-11)

---
Last refreshed: 2026-04-11
