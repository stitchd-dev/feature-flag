# Track Learnings: microservices_20260421

Patterns, gotchas, and context discovered during implementation.

## Codebase Patterns (Inherited)

- **Vendored protoc:** `protoc-bin-vendored` as build dependency in `stitchd-proto`, set `PROTOC` in `build.rs`. No system install required.
- **Graceful shutdown:** `tokio::select!` over `ctrl_c()` + `SIGTERM` (`#[cfg(unix)]`), passed to server's graceful shutdown hook.
- **Prometheus metrics:** `PrometheusBuilder::new().install_recorder()` → `PrometheusHandle` as Axum/tonic `State`, rendered at `/metrics`.
- **API Error Mapping:** `IntoResponse` on custom `ApiError` enum mapping domain errors → HTTP status codes.
- **Axum 0.8 routing:** `{param}` path syntax (not `:param`).
- **SQLx Offline Compilation:** New queries need `cargo sqlx prepare` before CI passes in offline mode.
- **ID Newtypes:** `macro_rules!` for UUID-based newtypes with `sqlx::Type(transparent)`.
- **OTel version alignment:** Pin all opentelemetry crates to same minor version to avoid type incompatibilities.
- **Rust 2024:** `std::env::set_var` is unsafe — wrap in `unsafe {}` with `// SAFETY:` comment.

---

<!-- Learnings from implementation will be appended below -->
