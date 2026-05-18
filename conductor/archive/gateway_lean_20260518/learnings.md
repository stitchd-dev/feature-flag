# Track Learnings: gateway_lean_20260518

Patterns, gotchas, and context discovered during implementation.

## Codebase Patterns (Inherited)

- **Prometheus metrics:** Use `PrometheusBuilder::new().install_recorder()` to get a `PrometheusHandle`. Pass it as Axum `State` and call `handle.render()` in the `/metrics` route handler. (from: scaffold_20260411)
- **Graceful shutdown:** Use `tokio::select!` over `ctrl_c()` + `SIGTERM` (gated `#[cfg(unix)]`) as the shutdown signal. Pass to `axum::serve(...).with_graceful_shutdown(...)`. (from: scaffold_20260411)
- **Vendored protoc:** Add `protoc-bin-vendored` as a build dependency in `stitchd-proto` and set `PROTOC` env var in `build.rs`. Eliminates system `protoc` requirement. (from: scaffold_20260411)
- **In-process cache primitive:** Use `moka::future::Cache<K, V>` with `time_to_live` for in-process caching. Call `.get_or_try_insert_with(key, loader)` — concurrent callers coalesce to a single loader. (from: db_optim_20260516)
- **gRPC service registration gotcha:** Implementing a tonic service trait is not sufficient — the service MUST also be registered via `.add_service(XxxServiceServer::new(impl_))` in `main.rs`. Unregistered services return `Unimplemented` with no startup warning. (from: sdk_rewrite_20260516)
- **Gateway is the sole SDK trust boundary:** Backend services NEVER validate SDK keys — they trust the `x-env-id` gRPC metadata header propagated by the gateway. (from: sdk_rewrite_20260516)
- **ClickHouse credentials required at startup:** Services that write to ClickHouse must be started with `CLICKHOUSE_USER=stitchd CLICKHOUSE_PASSWORD=stitchd CLICKHOUSE_DB=stitchd`. (from: sdk_rewrite_20260516)
- **Admin vs SDK response shape:** Always define a separate `AdminFooJson` struct in the gateway for admin UI responses. The SDK-facing `FooJson` must stay minimal. Never bloat the SDK response to satisfy UI needs. (from: flags_crud_20260512)
- **Gateway JSON/payload body size limit:** actix-web defaults to ~256 KB for JSON request bodies. Bulk import endpoints must raise the limit via `web::PayloadConfig::default().limit(bytes)`. (from: segment_scylla_20260516)
- **AggregatingMergeTree insert/read combiners:** Use `*State` combiners in INSERT…SELECT, `*Merge` combiners in GROUP BY reads. (from: db_optim_20260516)

---

<!-- Learnings from implementation will be appended below -->
