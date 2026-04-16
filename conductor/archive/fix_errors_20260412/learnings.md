# Track Learnings: fix_errors_20260412

Patterns, gotchas, and context discovered during implementation.

## 2026-04-12 10:30 - Final Status: COMPLETED
- **Implemented:** Fixed widespread errors across the workspace, including SQLx cache, missing dependencies, Axum 0.8 syntax changes, and Clippy lints.
- **Files changed:** 
  - `crates/stitchd-db/migrations/20260412000002_segment_list_entries.sql`
  - `crates/stitchd-server/Cargo.toml`
  - `crates/stitchd-server/src/api/segments/handlers.rs`
  - `crates/stitchd-server/src/api/router.rs`
  - `crates/stitchd-server/src/api/segments/types.rs`
  - `crates/stitchd-server/src/startup.rs`
  - `crates/stitchd-server/src/lib.rs`
  - `crates/stitchd-server/src/main.rs`
  - `crates/stitchd-server/tests/segments.rs`
  - `crates/stitchd-db/tests/segment_repository.rs`
  - `crates/stitchd-core/src/flag.rs`
  - `crates/stitchd-core/src/rule_engine/eval_leaf.rs`
  - `crates/stitchd-db/src/repository/pg/segment.rs`
- **Learnings:**
  - Patterns: 
    - Axum 0.8 uses `{param}` for path captures instead of `:param`.
    - `sqlx::test` handles DB lifecycle but migrations must be compatible with the environment (e.g. `pg_partman` might be missing).
  - Gotchas:
    - Clippy flags `3.14` as `approx_constant` (PI).
    - `as i32` on `usize` triggers truncation lints in Rust 2024.
  - Context:
    - Workspace dependency alignment is critical for avoiding multiple versions of `chrono` and other core crates.

## Codebase Patterns (Inherited)

- Rust 2024 edition requires `resolver = "3"` in workspace `Cargo.toml`.
- `std::env::set_var` is **unsafe** in Rust 2024.
- `rustfmt.toml` options like `imports_granularity` are **nightly-only**.
- **SQLx Offline Compilation:** `sqlx::query!` macros require a live DB or up-to-date `.sqlx` cache.
- **OpenTelemetry version alignment:** Pin all OTel crates to the same minor version to avoid incompatible types.

---
