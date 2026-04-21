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

## [2026-04-21 00:00] - Phase 1 Task 1: PostgreSQL migrations
- **Implemented:** All 10 auth tables in a single migration `20260421000001_auth_schema.sql`; drops old per-org users table from migration 003 and replaces with platform-level schema
- **Files changed:** `crates/stitchd-db/migrations/20260421000001_auth_schema.sql`, `crates/stitchd-db/tests/auth_migrations_test.rs`
- **Commit:** b07de50
- **Learnings:**
  - Patterns: Old `users` and `user_project_roles` from migration 003 must be dropped in the new migration — the existing schema had a per-org user model
  - Gotchas: Verify FK references — `organisations`, `projects`, `environments` tables exist from earlier migrations; name all constraint explicitly
  - Context: `#[sqlx::test(migrations = "./migrations")]` tests pass compilation even without a live DB
---

## [2026-04-21 00:00] - Phase 1 Task 2: Domain types in stitchd-core
- **Implemented:** Auth domain types module at `crates/stitchd-core/src/auth/`; ID newtypes added to id.rs; all role enums with sqlx::Type + PartialOrd; User/OrgMembership/RefreshToken/AuthProvider/Invite structs
- **Files changed:** `crates/stitchd-core/src/auth/mod.rs`, `crates/stitchd-core/src/auth/types.rs`, `crates/stitchd-core/src/id.rs`, `crates/stitchd-core/src/lib.rs`
- **Commit:** 945d29a
- **Learnings:**
  - Patterns: The org ID type is `OrganisationId` (not `OrgId`) — check actual type names before using
  - Patterns: Enum variants ordered low-privilege first so Rust's derived `Ord` gives higher variants more privilege (OrgMember=0, OrgAdmin=1 → OrgAdmin > OrgMember)
  - Patterns: `sqlx::Type` with `#[sqlx(rename_all = "snake_case")]` maps Rust PascalCase variants to snake_case DB CHECK values
---
