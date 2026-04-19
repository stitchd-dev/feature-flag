# Track Learnings: events_20260419

Patterns, gotchas, and context discovered during implementation.

## Codebase Patterns (Inherited)

- `clickhouse` crate v0.13 has no `derive` feature — use `uuid`, `time`, `lz4` features instead.
- **SQLx Offline Compilation:** `sqlx::query!` macros require a live DB or up-to-date `.sqlx` cache. New queries will break compilation in offline mode until `cargo sqlx prepare` is executed.
- **Database Extension Dependencies:** Call functions from extensions (like `pg_partman`) using plain `sqlx::query` to avoid macro-based compilation errors.
- **API Error Mapping:** Implement `IntoResponse` for a custom `ApiError` enum mapping internal errors to HTTP status codes.
- **Axum 0.8 Routing:** Use `{param}` syntax (e.g., `/environments/{env_id}`).
- **ID Newtypes:** Use `macro_rules!` to define UUID-based newtypes with `sqlx::Type(transparent)`.
- **Isolated DB Testing:** `#[sqlx::test(migrations = "./migrations")]` for fast, isolated SQLx tests.
- Axum router integration tests: use `tower::ServiceExt::oneshot`.
- `std::env::set_var` is **unsafe** in Rust 2024 — wrap in `unsafe {}` with `// SAFETY:` comment.

---

<!-- Learnings from implementation will be appended below -->
