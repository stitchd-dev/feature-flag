# Track Learnings: segment_scylla_20260516

Patterns, gotchas, and context discovered during implementation.

## Codebase Patterns (Inherited)

From `conductor/patterns.md`:

- **In-process cache primitive:** Use `moka::future::Cache<K, V>` with `time_to_live`; `.get_or_try_insert_with(key, loader)` coalesces concurrent callers for the same key. Useful candidate for the per-request `current_gen` resolution cache. (from: db_optim_20260516)
- **SQLx Offline Compilation:** `sqlx::query!` macros require a live DB or up-to-date `.sqlx` cache. After Phase 3 cleanup, run `SQLX_OFFLINE=false cargo sqlx prepare --workspace` and commit the refreshed cache. (from: segmentation_20260412)
- **Database Extension Dependencies:** Use plain `sqlx::query` (non-macro) for `pg_partman` calls to avoid macro-time compilation errors when extensions aren't installed locally. Relevant for the Phase 3 drop migration. (from: segmentation_20260412)
- **`CREATE INDEX CONCURRENTLY` inside a transaction:** sqlx wraps each migration file in a transaction by default. Drop-table migrations are fine, but any `CONCURRENTLY` operation needs its own file or `-- migrate:noTransaction`. (from: db_optim_20260516)
- **OpenTelemetry version alignment:** Pin all OTel crates to the same minor version (`tracing-opentelemetry 0.29` ↔ `opentelemetry 0.28`). Verify the `scylla` driver's OTel hooks (if any) align with the pinned version before wiring spans. (from: scaffold_20260411)
- **Vendored protoc:** Already configured in `stitchd-proto/build.rs` — adding new RPCs (`AddEntries`, `RemoveEntries`) needs no extra build setup. (from: scaffold_20260411)
- **Prometheus metrics:** `PrometheusBuilder::new().install_recorder()` → pass `PrometheusHandle` as Axum `State` → `/metrics` route renders. Use the same pattern for Scylla driver metrics export. (from: scaffold_20260411)

---

<!-- Learnings from implementation will be appended below -->
