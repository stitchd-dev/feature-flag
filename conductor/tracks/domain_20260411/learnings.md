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
- **Commit:** 92f4d97
- **Learnings:**
  - Patterns: Used `macro_rules!` to define repetitive ID newtypes with `sqlx::Type` and `sqlx(transparent)`.
  - Gotchas: Ensure `sqlx` is added to crate-level `Cargo.toml` when using `sqlx::Type` derive, even if defined in workspace.
---
---

## [2026-04-11] - Phase 1 Task 3: Flag types (FlagValueType, VariantValue, Variant)
- **Implemented:** FlagValueType enum, VariantValue enum with `matches_type`, Variant struct with full serde support.
- **Files changed:** `crates/stitchd-core/src/flag.rs`, `crates/stitchd-core/src/lib.rs`
- **Commit:** 97b620e
- **Learnings:**
  - Patterns: `matches!` macro with tuple patterns is the cleanest way to express multi-arm type dispatch without an explicit match expression.
  - Patterns: Use `#[serde(untagged)]` on enums when the JSON discriminant is implicit in the value shape (e.g. bool vs int vs string).
---

## [2026-04-11] - Phase 1 Task 4: Multi-tenancy types (Organisation, Project, Environment, SdkKey)
- **Implemented:** All four tenancy structs with soft-delete and optimistic-concurrency fields. `SdkKey::has_active_key` helper.
- **Files changed:** `crates/stitchd-core/src/tenant.rs`, `crates/stitchd-core/src/lib.rs`
- **Commit:** 411bb34
- **Learnings:**
  - Patterns: `Iterator::any` is the idiomatic way to implement `has_active_key` — avoids manual loops.
---

## [2026-04-11] - Phase 1 Task 5: User identity types (User, Role, Permission)
- **Implemented:** ResourceType, Action enums; Permission struct with wildcard `matches`; User and Role structs.
- **Files changed:** `crates/stitchd-core/src/user.rs`, `crates/stitchd-core/src/lib.rs`
- **Commit:** 82b0efe
- **Learnings:**
  - Patterns: `strip_suffix('*')` is the clean way to detect and handle `prefix-*` patterns without regex.
  - Gotchas: `"payments-"` (trailing dash after strip) still matches `prefix-*` — deliberate, the prefix itself is a valid resource name prefix.
---

## [2026-04-11] - Phase 1 Task 6: Wire lib.rs and verify suite
- **Implemented:** All 5 modules re-exported. 30 unit tests + 3 doctests pass. rustfmt applied.
- **Files changed:** `crates/stitchd-core/src/lib.rs`, `crates/stitchd-core/src/context.rs`, `crates/stitchd-core/src/id.rs`
- **Commit:** 2a730b9
- **Learnings:**
  - Gotchas: **Always run `cargo fmt` before committing.** rustfmt reorders imports alphabetically and collapses multi-line `#[derive(...)]` — failing to do so causes CI fmt check to fail.
---

## [2026-04-11] - Phase 2: Database Schemas
- **Implemented:** 6 PG migrations + 2 CH migrations. sqlx-cli installed. All applied cleanly.
- **Files changed:** `crates/stitchd-db/migrations/` (6 files), `crates/stitchd-db/clickhouse-migrations/` (2 files)
- **Commit:** 5996faf
- **Learnings:**
  - Gotchas: `psql` not in PATH on this machine — use `sqlx migrate info` to verify migration status instead.
  - Patterns: ClickHouse migrations applied via HTTP API: `curl -s "http://user:pass@localhost:8123/" --data-binary @file.sql`. No clickhouse-client required.
  - Patterns: `CREATE TABLE IF NOT EXISTS` makes ClickHouse migrations idempotent — safe to re-run.
  - Gotchas: `sqlx-cli` must be installed with `--no-default-features --features rustls,postgres` to avoid OpenSSL dependency.
---

## [2026-04-11] - Phase 3: Repository Layer
- **Implemented:** Repository traits and Postgres implementations for all aggregate roots. 8 integration tests using `#[sqlx::test]`.
- **Files changed:** `crates/stitchd-db/src/repository/pg/*.rs`, `crates/stitchd-db/tests/*.rs`, `crates/stitchd-db/src/lib.rs`
- **Learnings:**
  - Patterns: `#[sqlx::test(migrations = "./migrations")]` is the magic for fast, isolated DB tests in sqlx 0.8.
  - Gotchas: `find_by_id` MUST include `AND deleted_at IS NULL` to honor soft-deletion.
  - Gotchas: `sqlx::query_as!` requires explicit type casts for custom types in the SQL query (e.g. `id AS "id: OrganisationId"`).
  - Context: `sqlx` query macros require `DATABASE_URL` during compilation (or offline cache).

---

## [2026-04-11] - Phase 4: Server Wiring & sqlx Offline Mode
- **Implemented:** AppState with PgPool, DB health check, sqlx offline cache generation.
- **Files changed:** `crates/stitchd-server/src/*.rs`, `crates/stitchd-server/src/main.rs`, `.sqlx/`, `.github/workflows/ci.yml`
- **Learnings:**
  - Patterns: `cargo sqlx prepare --workspace` is essential for CI stability without live DB.
  - Gotchas: Axum 0.8 `State` extraction requires the state type to be `Clone`.
---
