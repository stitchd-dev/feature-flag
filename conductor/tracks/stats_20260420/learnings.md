# Track Learnings: stats_20260420

Patterns, gotchas, and context discovered during implementation.

## Codebase Patterns (Inherited)

- Rust 2024 edition; `resolver = "3"` in workspace Cargo.toml
- UUID-based ID newtypes via `macro_rules!` with `sqlx::Type(transparent)`
- `#[sqlx::test(migrations = "./migrations")]` for isolated DB tests
- `IntoResponse` on custom `ApiError` enum for HTTP error mapping
- Axum 0.8 path params use `{param}` syntax (not `:param`)
- `std::env::set_var` requires `unsafe {}` in Rust 2024
- New sqlx queries require `cargo sqlx prepare` to update `.sqlx` cache
- Recursive types need `Box<T>` for recursive variants
- `tower::ServiceExt::oneshot` for Axum integration tests without TCP server
- OpenTelemetry crates must all pin to same minor version
- `clickhouse` crate v0.13: use `uuid`, `time`, `lz4` features (no `derive` feature)
- Plain `sqlx::query` (not macro) for extension functions like pg_partman

---

<!-- Learnings from implementation will be appended below -->
