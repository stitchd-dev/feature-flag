# Track Learnings: fix_ci_clippy_20260417

Patterns, gotchas, and context discovered during implementation.

## Codebase Patterns (Inherited)

- Rust 2024 edition: `#[deny(warnings)]` promotes all Clippy lints to hard errors — even stylistic ones like `map_unwrap_or`.
- Use `.is_ok_and(|v| ...)` instead of `.map(|v| ...).unwrap_or(false)` on `Result` values.
- `std::env::set_var` is **unsafe** in Rust 2024 — always wrap in `unsafe {}` with a `// SAFETY:` comment.

---

<!-- Learnings from implementation will be appended below -->
