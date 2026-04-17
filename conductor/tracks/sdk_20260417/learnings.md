# Track Learnings: sdk_20260417

Patterns, gotchas, and context discovered during implementation.

## Codebase Patterns (Inherited)

- **Prometheus metrics:** Use `PrometheusBuilder::new().install_recorder()` → pass handle as Axum `State`.
- **Graceful shutdown:** `tokio::select!` over `ctrl_c()` + `SIGTERM` → `axum::serve(...).with_graceful_shutdown(...)`.
- **Vendored protoc:** `protoc-bin-vendored` as build-dep in `stitchd-proto`; set `PROTOC` in `build.rs`.
- **API Error Mapping:** `IntoResponse` for `ApiError` enum maps internal errors → HTTP status codes.
- **Axum 0.8 Routing:** `{param}` syntax (not `:param`) for path captures.
- **SQLx Offline Compilation:** New queries break offline mode until `cargo sqlx prepare` is run.
- **Database Extension Dependencies:** Call `pg_partman` functions via plain `sqlx::query` (not macro).
- **OpenTelemetry version alignment:** Pin all OTel crates to same minor version.
- **`opentelemetry_sdk 0.28`:** Use `Resource::builder()` (not `Resource::new()`).
- **Recursive Types:** Recursive enums need `Box<T>` on recursive variants.
- **ID Newtypes:** `macro_rules!` for UUID-based newtypes with `sqlx::Type(transparent)`.
- **Wildcard Matching:** `strip_suffix('*')` for prefix-based wildcard matching.
- **Isolated DB Testing:** `#[sqlx::test(migrations = "./migrations")]` for auto-migrated isolated tests.
- **`clickhouse` crate v0.13:** No `derive` feature — use `uuid`, `time`, `lz4` features.
- **Integer Truncation:** Avoid `as i32` on `usize`; use `try_into()`.
- **`std::env::set_var`:** Wrap in `unsafe {}` with `// SAFETY:` comment in Rust 2024.
- **HashMap iteration:** Use `.keys()` / `.values()` to avoid `clippy::for_kv_map`.

---

<!-- Learnings from implementation will be appended below -->
