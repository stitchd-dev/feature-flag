# Track Learnings: boundaries_20260518

Patterns, gotchas, and context discovered during implementation.

## Codebase Patterns (Inherited)

### From `gateway_lean_20260518` (most relevant — recent boundary track)
- **Gateway lean principle:** When the gateway accumulates direct DB clients or domain logic beyond auth+routing, extract those into a dedicated service. The gateway should only hold gRPC client channels. Direct DB deps in the gateway signal a service boundary violation.
- **Fire-and-forget gRPC for analytics:** Use `tokio::spawn` for non-critical analytics/telemetry gRPC calls. Log errors but don't propagate them — callers should never block on telemetry side-effects.
- **Shared `require_permission` in `routes/mod.rs`:** Extract repeated permission-checking helpers to the route module root. All sub-modules use `super::require_permission` — prevents copy-paste drift.

### From `sdk_rewrite_20260516` (SDK + boundary patterns)
- **Gateway is the sole SDK trust boundary:** Backend services NEVER validate SDK keys — they trust the `x-env-id` gRPC metadata header propagated by the gateway. Backends must reject requests missing this header with `Unauthenticated`.
- **gRPC service registration gotcha:** Implementing a tonic service trait is not sufficient — the service MUST be registered via `.add_service(XxxServiceServer::new(impl_))` in `main.rs`. Unregistered services return `Unimplemented` with no startup warning.
- **ClickHouse credentials required at startup:** Services that write to ClickHouse must be started with `CLICKHOUSE_USER=stitchd CLICKHOUSE_PASSWORD=stitchd CLICKHOUSE_DB=stitchd`. The ClickHouse-rs client defaults to `user=default` with no password, which fails auth silently with 502.
- **Stale worktree binary on shared port:** When restarting services for testing, verify with `ps -o comm=` that the binary serving a port is from the current worktree. Old binaries from previous tracks may still be listening and silently lack new gRPC methods.

### From `flags_crud_20260512` (domain model + admin patterns)
- **Domain model change order:** When adding a field to a domain type, always follow the chain: `stitchd-core` structs → DB repo queries → flag/domain service → proto definition → proto mapping (`mapping.rs`) → gateway handler. Skipping steps causes compile errors deep in the chain.
- **Admin vs SDK response shape:** Always define a separate `AdminFooJson` struct in the gateway for admin UI responses (full data: name, description, variants, rules, version, timestamps). The SDK-facing `FooJson` must stay minimal for performance. Never bloat the SDK response to satisfy UI needs.

### From `db_optim_20260516` (DB + pagination)
- **`CREATE INDEX CONCURRENTLY` inside a transaction:** sqlx migrations wrap each file in a transaction. `CREATE INDEX CONCURRENTLY` cannot run inside a transaction. Split into its own migration file or add `-- migrate:noTransaction`.
- **`serde_urlencoded` + `#[serde(flatten)]` + `u32`:** Axum's `Query<T>` extractor uses `serde_urlencoded`, which passes query param values as strings. A `u32` field inside a `#[serde(flatten)]` struct will fail with "invalid type: string '1', expected u32". Add a custom visitor that calls `deserialize_any`.
- **In-process cache primitive:** Use `moka::future::Cache<K, V>` with `time_to_live` for in-process caching. Call `.get_or_try_insert_with(key, loader)` — concurrent callers for the same key coalesce to a single loader invocation.
- **AggregatingMergeTree insert/read combiners:** When writing to ClickHouse AggregatingMergeTree, use `*State` combiners (`sumState`, `countState`); when reading, use `*Merge` combiners. Not `finalizeAggregation`.

### From `segment_scylla_20260516` (ScyllaDB)
- **CQL TIMESTAMP type mapping:** Use `scylla::value::CqlTimestamp(millis_i64)` for TIMESTAMP columns — NOT raw `i64` or `chrono::DateTime`.
- **Random generation IDs prevent CAS collisions:** In generation-swap CAS patterns, use a random i64 rather than `current_gen + 1`. Sequential IDs cause silent data merging.

### From `env_sdk_rbac_20260429` (RBAC + admin UI)
- **Cargo must run from the worktree root:** Running `cargo test/clippy` from the main repo root compiles the main branch, silently ignoring worktree changes. Always `cd .worktrees/<track_id>/` or pass `-C <worktree_path>` before any Cargo command when working in a worktree.
- **RBAC UI gating pattern:** Use `disabled` + `style={{ opacity: 0.35 }}` (never `display:none`) for actions the user lacks permission for. For zero read access, render a full `LockOverlay` over the section.
- **Sidebar picker pattern:** All sidebar entity pickers share one visual pattern: trigger button using `.org-switcher` + `.org-avatar` + `.org-meta` + outside-click-to-close via `useRef` + `mousedown` listener. Phase 4 will consolidate these into a shared `<Dropdown>` primitive.

### From `admin_ui_27260427` (admin baseline)
- **`verbatimModuleSyntax`:** Vite + TypeScript projects with `verbatimModuleSyntax: true` require `import type { Foo }` for any type-only import.
- **TypeScript CLI:** Never run `npx tsc` — it resolves to a stray 2.0.x package. Always use `node_modules/.bin/tsc --noEmit -p tsconfig.app.json`.
- **Vite dev proxy:** Admin UI uses `vite.config.ts` server proxy: `/api → http://localhost:8080` with `changeOrigin: true` and path rewrite stripping the `/api` prefix.

---

<!-- Learnings from implementation will be appended below -->
