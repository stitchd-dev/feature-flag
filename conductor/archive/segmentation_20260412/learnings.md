# Track Learnings: segmentation_20260412

Patterns, gotchas, and context discovered during implementation.

## Codebase Patterns (Inherited)

### Code Conventions
- Rust 2024 edition requires `resolver = "3"` in workspace `Cargo.toml` (not `"2"`).
- `std::env::set_var` is **unsafe** in Rust 2024 — always wrap in `unsafe {}` with a `// SAFETY:` comment.
- `rustfmt.toml` options like `imports_granularity`, `group_imports` are **nightly-only** — strip from stable configs.

### Rust 2024 Edition Patterns
- When iterating a `HashMap` and only needing keys or values, use `.keys()` / `.values()` — `for (k, _) in map` triggers `clippy::for_kv_map` as a warning-level error with `-D warnings`.
- In filter closures over iterator references (e.g. `.filter(|(_, d)| ...)`), the value `d` is `&&T` — dereference with `**d` or use `.filter(|&(_, d)| ...)`. Pattern-binding `&d` inside a non-reference outer pattern fails in Rust 2024.

### Architecture
- **Axum router integration tests:** use `tower::ServiceExt::oneshot` to send a single request through the router without starting a real TCP server. Add `tower` as a `[dev-dependencies]` entry.
- **Prometheus metrics:** Use `PrometheusBuilder::new().install_recorder()` to get a `PrometheusHandle`. Pass it as Axum `State`.
- **Graceful shutdown:** Use `tokio::select!` over `ctrl_c()` + `SIGTERM`.

### Gotchas
- `clickhouse` crate v0.13 has no `derive` feature — use `uuid`, `time`, `lz4` features instead.
- **OpenTelemetry version alignment:** pin all OTel crates to the same minor version to avoid incompatible types.

---

## [2026-04-12 00:15] - Phase 3 Completion: Track Finalized
- **Implemented:** Complete segmentation module (core eval, db persistence with partitioning, and REST API).
- **Files changed:** crates/stitchd-core/src/segment.rs, crates/stitchd-db/src/repository/mod.rs, crates/stitchd-db/src/repository/pg/segment.rs, crates/stitchd-db/migrations/*, crates/stitchd-server/src/*
- **Learnings:**
  - Patterns: Recursive validation of `ConditionExpr` to enforce domain constraints (e.g. segment independence) before reaching the database.
  - Patterns: Implementing `IntoResponse` for a custom `ApiError` enum that maps `RepositoryError` and `ValidationError` to HTTP status codes.
  - Gotchas: `sqlx::query!` macros require a live database connection or an up-to-date `.sqlx` cache for compilation. When adding new tables/queries, compilation will fail in offline mode until `cargo sqlx prepare` is run.
  - Gotchas: `pg_partman` functions like `run_maintenance` should be called with plain `sqlx::query` to avoid hard dependencies on extensions during offline compilation.
---
