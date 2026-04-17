# Track Learnings: fix_lint_errors_20260417

Patterns, gotchas, and context discovered during implementation.

## Codebase Patterns (Inherited)

- Rust 2024 edition requires `resolver = "3"` in workspace `Cargo.toml`.
- `std::env::set_var` is **unsafe** in Rust 2024.
- `rustfmt.toml` options like `imports_granularity` are **nightly-only**.
- When iterating a `HashMap` and only needing keys or values, use `.keys()` / `.values()`.
- In filter closures over iterator references, the value `d` is `&&T` — dereference with `**d` or use `.filter(|&(_, d)| ...)`.
- Rust 2024 lints against implicit/risky conversions; `as i32` on `usize` should be avoided or handled with `try_into()`.

---

<!-- Learnings from implementation will be appended below -->
