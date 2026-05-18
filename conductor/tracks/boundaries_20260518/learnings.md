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

## [2026-05-19 00:00] Wave 1 — Phase 1 first wave + Phase 6 (8 parallel workers)

**Tasks completed:** 1.1, 1.2, 1.3, 1.7, 6.1, 6.2, 6.3, 6.4 (8 of 8).

**Worker commits (all merged into `track/boundaries_20260518`):**
- 1.1 analytics gRPC: `cf91164` → reconciled with Worker 3's canonical trait at merge commit `6f58019`
- 1.2 experimentation gRPC: `3ef8b4a`
- 1.3 ClickHouse repo: `81ef9cd`
- 1.7 Scylla audit: `2ebee8b`
- 6.1 archived spec: `16fa0a8`
- 6.2 LRU spec: `d3ec5de`
- 6.3 naming spec: `0fd638d`
- 6.4 SDK README: `8ada1cf`

Workspace `cargo check --workspace` clean after all 8 merges.

**Patterns / gotchas:**

- **Two workers, two traits, one canonical.** When two parallel workers (a handler-side worker and a repo-side worker) both define a "canonical" trait for the same domain, the **repo-side worker** should own the trait — it controls storage semantics. The handler-side worker's trait + types are removed during merge integration. Cosmetic naming differences (`WriteResultInput` vs `WriteResultRow`) are easy; method-signature differences (single-row vs batch; return-row vs `()`) must be aligned at the **proto schema** level — that's the natural integration point. Resolution involved adding `env_id` + `variant_key` to every Experiment-Results proto message and renumbering field tags.

- **`bd close --continue` cascade hazard.** With 8 workers closing simultaneously, the `--continue` flag aggressively claimed Phase 2 + Phase 3 tasks into `in_progress` even though those phases were blocked behind the Phase 1 milestone in the dep graph. Beads' molecule auto-advance does not honour cross-task / cross-phase dependencies when those dependencies are between milestones, only between direct task-to-task `bd dep add`. **Mitigation:** orchestrator must verify no cross-phase tasks got auto-claimed and `bd update ... --status open --assignee ""` to reset. Consider `bd close --no-auto` next time.

- **Worker beads-close can miss.** Worker 1's commit landed but its `bd close` didn't run (likely an agent-runtime cutoff). **Mitigation:** orchestrator verifies every worker's beads task state matches its commit during result aggregation; closes manually with `bd note` + `bd close` if missed.

- **Worker isolation via separate `git worktree add`** works cleanly with `bd worktree` not needed for branch-from-track-branch case. Each worktree gets its own `target/` (slow first build, full isolation). No `CARGO_TARGET_DIR` sharing needed. Doc-only workers: 1–3 min; backend workers: 5–10 min.

- **`git checkout --ours <path>`** during a merge keeps HEAD content but the file remains in unmerged state until `git add` is run.

- **`--no-ff` merge + `git branch -d`** refuses because the worker branch is not in a fast-forward ancestor relationship. Use `-D` once the worker SHAs are confirmed in the target branch history (`git log --oneline track/... ^main`).

- **ScyllaDB containment correction**: the plan stated "only `stitchd-segmentation-service` and `xtask` have direct `scylla` deps". Actually `xtask` is compliant via **transitive** dep through `stitchd-db`; direct deps are only `stitchd-db` (library home) + `stitchd-segmentation-service` (binary consumer). This is captured in the new `SCYLLA_OWNERSHIP.md`.

---
