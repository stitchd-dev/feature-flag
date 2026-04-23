# Track Learnings: scheduled_stats_20260423

Patterns, gotchas, and context discovered during implementation.

## Codebase Patterns (Inherited)

### Scheduler / Service Patterns
- **Graceful shutdown:** Use `tokio::select!` over `ctrl_c()` + SIGTERM (gated `#[cfg(unix)]`) as the shutdown signal. Pass to `axum::serve(...).with_graceful_shutdown(...)`.
- **Prometheus metrics:** Use `PrometheusBuilder::new().install_recorder()` to get a `PrometheusHandle`. Pass it as Axum `State` and call `handle.render()` in the `/metrics` route handler.

### Database Patterns
- **SQLx Offline Compilation:** `sqlx::query!` macros require a live DB or up-to-date `.sqlx` cache. New queries will break compilation in offline mode until `cargo sqlx prepare` is executed.
- **Isolated DB Testing:** `#[sqlx::test(migrations = "./migrations")]` is the idiomatic way to run fast, isolated database tests with automatic migration handling in SQLx 0.8.
- **API Error Mapping:** Implement `IntoResponse` for a custom `ApiError` enum that maps internal errors (Repository, Validation, etc.) to HTTP status codes.
- **Database Extension Dependencies:** Call functions from extensions (like `pg_partman`) using plain `sqlx::query` to avoid macro-based compilation errors when extensions aren't available in the local build environment.

### ClickHouse
- `clickhouse` crate v0.13 has no `derive` feature — use `uuid`, `time`, `lz4` features instead.

### Rust 2024 Edition
- `std::env::set_var` is **unsafe** in Rust 2024 — always wrap in `unsafe {}` with a `// SAFETY:` comment.
- When iterating a `HashMap` and only needing keys or values, use `.keys()` / `.values()` — `for (k, _) in map` triggers `clippy::for_kv_map`.
- In filter closures over iterator references, the value `d` is `&&T` — dereference with `**d` or use `.filter(|&(_, d)| ...)`.
- **Integer Truncation:** Rust 2024 lints against implicit/risky conversions; use `try_into()` where overflow is possible.

### gRPC / Proto
- **Vendored protoc:** `protoc-bin-vendored` is a build dependency in `stitchd-proto` — no system `protoc` needed.
- **Axum 0.8 Routing:** Use `{param}` syntax (e.g. `/experiments/{id}/recompute`), not `:param`.

### Auth / ID Types
- The org identifier type is `OrganisationId` (not `OrgId`) — check `crates/stitchd-core/src/id.rs` before use.
- **ID Newtypes:** Use `macro_rules!` to define repetitive UUID-based newtypes with `sqlx::Type(transparent)`.

---

<!-- Learnings from implementation will be appended below -->
