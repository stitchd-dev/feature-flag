//! Compile-time + grep-based purity guards for the `evaluation` module.
//!
//! The unified `evaluate_flag` entry point is documented as a pure function:
//! no async, no I/O, no side-effecting logging, no clock reads. To keep
//! that contract honest as the module evolves, this file holds two tests
//! that fail at `cargo test` time if the contract is broken:
//!
//! 1. `evaluation_module_does_not_use_warn_or_error` — grep-walks the
//!    source of every `evaluation/*.rs` file looking for forbidden tokens
//!    (`tracing::warn!`, `warn!(`, `tracing::error!`, `error!(`, `tokio::`,
//!    `reqwest::`, `sqlx::`). Comments are stripped before scanning so
//!    docstrings mentioning these tokens don't trip the check.
//! 2. `evaluation_module_does_not_import_io_crates` — checks the same
//!    files for `use tokio::`, `use reqwest::`, `use sqlx::` import lines
//!    so a future contributor pulling in an I/O dep at module level fails
//!    fast.
//!
//! These are runtime checks (the test harness reads source files), not
//! compile-time checks — but they still run in CI on every `cargo test
//! -p stitchd-core --lib` so regressions are caught immediately.

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;

    /// Returns the absolute path to each `.rs` file in the `evaluation`
    /// module that should be subject to purity checks — engine.rs,
    /// preview.rs, types.rs, mod.rs. Excludes `purity.rs` itself because
    /// its forbidden-token list naturally contains the forbidden tokens.
    fn evaluation_module_files() -> Vec<PathBuf> {
        // CARGO_MANIFEST_DIR points at `crates/stitchd-core` when this test
        // runs. The evaluation module lives at `src/evaluation/`.
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/evaluation");
        let mut out = Vec::new();
        for entry in fs::read_dir(&dir).expect("read evaluation/ dir") {
            let entry = entry.expect("read entry");
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) == Some("rs")
                && path.file_name().and_then(|s| s.to_str()) != Some("purity.rs")
            {
                out.push(path);
            }
        }
        out.sort();
        assert!(
            !out.is_empty(),
            "expected at least one .rs file in evaluation/ to scan"
        );
        out
    }

    /// Strip `//` line comments and `/* … */` block comments from `src` so
    /// docstrings referencing forbidden tokens don't trip the grep. Naïve
    /// scanner — does not handle string literals containing `//`, which
    /// is acceptable for this internal lint.
    fn strip_comments(src: &str) -> String {
        let mut out = String::with_capacity(src.len());
        let bytes = src.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            // Block comment
            if i + 1 < bytes.len() && bytes[i] == b'/' && bytes[i + 1] == b'*' {
                i += 2;
                while i + 1 < bytes.len() && !(bytes[i] == b'*' && bytes[i + 1] == b'/') {
                    i += 1;
                }
                i = (i + 2).min(bytes.len());
                continue;
            }
            // Line comment (preserves the newline so line counts stay
            // legible if a future test surfaces line numbers).
            if i + 1 < bytes.len() && bytes[i] == b'/' && bytes[i + 1] == b'/' {
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
                continue;
            }
            out.push(bytes[i] as char);
            i += 1;
        }
        out
    }

    #[test]
    fn evaluation_module_does_not_use_warn_or_error() {
        // Forbidden side-effecting logging + clock + I/O tokens. The pure
        // evaluator may use `tracing::debug!` and `tracing::info!` (per
        // the engine.rs docstring) but NOT `warn!` or `error!` because
        // operators must not see noise from the hot path, and the SDK
        // hot path must not allocate log fields.
        const FORBIDDEN_TOKENS: &[&str] = &[
            "tracing::warn!",
            "tracing::error!",
            "warn!(",
            "error!(",
            // I/O crate prefixes (catches `tokio::spawn`, `reqwest::Client`, etc.)
            "tokio::",
            "reqwest::",
            "sqlx::",
        ];
        for path in evaluation_module_files() {
            let raw = fs::read_to_string(&path)
                .unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
            let stripped = strip_comments(&raw);
            for token in FORBIDDEN_TOKENS {
                assert!(
                    !stripped.contains(token),
                    "evaluation purity violation: {} contains forbidden token `{}` (the unified evaluator must be pure — no side-effecting logging, no I/O). Move the offending call to the caller's side.",
                    path.display(),
                    token,
                );
            }
        }
    }

    #[test]
    fn evaluation_module_does_not_import_io_crates() {
        // Catch `use tokio::…`, `use reqwest::…`, `use sqlx::…` import
        // lines at module level. These would be transitive deps the
        // evaluation module pulls in even if no call site uses them.
        const FORBIDDEN_USE_PREFIXES: &[&str] = &[
            "use tokio::",
            "use reqwest::",
            "use sqlx::",
        ];
        for path in evaluation_module_files() {
            let raw = fs::read_to_string(&path)
                .unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
            let stripped = strip_comments(&raw);
            for prefix in FORBIDDEN_USE_PREFIXES {
                assert!(
                    !stripped.contains(prefix),
                    "evaluation purity violation: {} contains forbidden import `{}`. The evaluation module must not depend on tokio / reqwest / sqlx; assemble inputs at the caller's side and pass them in.",
                    path.display(),
                    prefix,
                );
            }
        }
    }
}
