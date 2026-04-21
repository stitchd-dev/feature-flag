# Track Learnings: auth_20260421

Patterns, gotchas, and context discovered during implementation.

## Codebase Patterns (Inherited)

- Rust 2024 edition — `resolver = "3"` in workspace Cargo.toml
- `std::env::set_var` requires `unsafe {}` with `// SAFETY:` comment
- `macro_rules!` for UUID-based newtypes with `sqlx::Type(transparent)`
- `#[sqlx::test(migrations = "./migrations")]` for isolated DB integration tests
- New `sqlx::query!` macros need `cargo sqlx prepare` before offline CI compile
- Axum 0.8: use `{param}` path syntax (not `:param`)
- `IntoResponse` on custom `ApiError` enum for HTTP status mapping
- `tower::ServiceExt::oneshot` for handler unit tests without TCP server
- `-D warnings` in CI — all clippy lints are hard errors

---

<!-- Learnings from implementation will be appended below -->
