# Track Learnings: coverage_20260417

Patterns, gotchas, and context discovered during implementation.

## Codebase Patterns (Inherited)

- Axum router integration tests: use `tower::ServiceExt::oneshot` to send a single request through the router without starting a real TCP server. Add `tower` as a `[dev-dependencies]` entry. (from: scaffold_20260411)
- `std::env::set_var` is **unsafe** in Rust 2024 — always wrap in `unsafe {}` with a `// SAFETY:` comment, even in tests. (from: scaffold_20260411)
- **SQLx Offline Compilation:** `sqlx::query!` macros require a live DB or up-to-date `.sqlx` cache. New queries will break compilation in offline mode until `cargo sqlx prepare` is executed. (from: segmentation_20260412)
- **Integer Truncation:** Rust 2024 lints against `as i32` on `usize` — use `try_into()`. (from: fix_errors_20260412)
- Rust 2024 edition: `#[deny(warnings)]` promotes all Clippy lints to hard errors — use `.is_ok_and()` not `.map().unwrap_or(false)`. (from: fix_ci_clippy_20260417)

---

<!-- Learnings from implementation will be appended below -->
