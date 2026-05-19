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

## [2026-05-19 00:50] Wave 2 — Phase 1 tasks 1.4, 1.5 + Phase 6 task 6.5 (3 parallel workers)

**Tasks completed:** 1.4, 1.5, 6.5 (3 of 3).

**Worker commits:**
- 1.4 stats-service gRPC refactor: `a024312`
- 1.5 experimentation GetExperimentResults via analytics: `f2b6a6a`
- 6.5 SDK conformance verified: no commit (working tree clean — 8/8 fixtures green)

Workspace `cargo check --workspace` clean after both merges.

**Patterns / gotchas:**

- **`bd close --no-auto` is the correct default for parallel waves.** Wave 1 used `--continue` and the cascade auto-claimed Phase 2 + Phase 3 tasks across milestone boundaries — required manual reset. Wave 2 used `--no-auto`; no cascade, no resets needed. Use `--no-auto` whenever the orchestrator (not beads) controls wave advancement.

- **In-process tonic mocks via `TcpListenerStream`.** Worker 9 needed to test stats-service against mocked experimentation-service + analytics-service gRPC. Pattern: `tokio::net::TcpListener::bind("127.0.0.1:0")` to claim a random port; wrap with `tokio_stream::wrappers::TcpListenerStream`; pass to `tonic::transport::Server::builder().serve_with_incoming(...)` running in a `tokio::spawn`. Client connects to `format!("http://{}", listener.local_addr()?)`. No external mocking lib; no port conflicts. Reusable pattern for cross-service-gRPC integration tests.

- **`*ServiceClient` lives in `*_service_client` sub-module of generated proto code.** When importing tonic-generated clients, the path is `stitchd_proto::analytics::v1::analytics_service_client::AnalyticsServiceClient` (not re-exported at v1::). Similarly for service traits used by hand-rolled servers: they live under `*_service_server::*ServiceTrait`.

- **`Arc<Mutex<Client>>` for tonic gRPC clients shared across tasks.** Worker 9 found that the `tonic::Client` type doesn't impl `Clone`-with-shared-state cleanly; wrapping in `Arc<Mutex<...>>` lets multiple async tasks send requests through the same channel. (Alternative: the underlying `Channel` is `Clone` and cheap to clone — clone the Channel and create per-task clients. Either pattern works.)

- **Proto JSON-string fields require explicit `serde_json::from_str` at consumer boundaries.** Worker 10 found that `ExperimentResult.variant_stats` / `frequentist_result` / `bayesian_result` are wire-level `String`, not `serde_json::Value`. Aggregation logic that previously worked off a `serde_json::Value` from PG must call `serde_json::from_str` first when reading via the new gRPC path.

- **Port assignment plan vs reality**: Worker 9 used default ports `EXPERIMENTATION_SERVICE_GRPC_URL=http://localhost:50054` and `ANALYTICS_SERVICE_GRPC_URL=http://localhost:50055`. Worker 10 used `ANALYTICS_SERVICE_GRPC_URL` with default `http://localhost:50054` (conflict — analytics-service vs experimentation-service ports). Phase 3 (task 3.6 port-suffix standardization + 3.4 STITCHD_ prefix) needs to reconcile actual port assignments per-service and document in docker-compose. Note as a follow-up.

- **`--features test-util` required for SDK clippy `--all-targets`**. The conformance test uses test-only helpers behind a feature flag. Without `--features test-util`, `cargo clippy -p stitchd-sdk --all-targets` fails with unresolved imports. Document this in the SDK README's testing section eventually.

---

## [2026-05-19 01:05] Wave 3 — Phase 1 task 1.6 (1 worker)

**Tasks completed:** 1.6 (drop PG experiment_results table + repo). Single worker, single commit `25779a9`.

665-line `experiment_results.rs` deleted; drop migration `20260519000001_drop_experiment_results.sql` added; `.sqlx/` cache was already clean (no stale entries referenced the dropped table).

**Patterns / gotchas:**

- **`.sqlx/` offline cache can be "clean" even when a repo is deleted.** The deleted `experiment_results.rs` had `sqlx::query!`/`query_as!` macros but no cached entries in `.sqlx/` referenced the `experiment_results` table — likely because the repo's queries were never compiled in offline mode (or were generated using `sqlx::query_as::<_, Row>(r"...")` raw strings instead of macros). Useful: don't assume cache regeneration is mandatory on repo deletion — grep `.sqlx/` first.

- **Discovered work pattern: file as bug with `discovered-from`-style note.** Worker 12 found a pre-existing `clippy::type_complexity` failure in `stitchd-gateway/src/grpc_server.rs:149` (function `stub_clients()`). Filed as separate beads bug `feature-flag-ysh` rather than in-scope fix, because (a) it's pre-existing on the base branch, and (b) Phase 2 (URL canonical rewrite) will likely touch `grpc_server.rs` significantly and can fold the fix in. Pattern: **when a worker finds work that's clearly out-of-scope but should not be lost, file a new beads bug with priority 2 and reference it from the report-back.**

**Phase 1 + Phase 6 are now CODE-COMPLETE.** Only the two user-manual-verification tasks remain (`feature-flag-mwk.1.8` and `feature-flag-mwk.6.6`). No more parallel waves until user verifies.

---

## [2026-05-19 02:00] Discovered work — workspace clippy cleanup (`feature-flag-mwk.8`)

**Out-of-scope but user-requested.** During Phase 1+6 verification, `cargo clippy --workspace --all-targets -- -D warnings` revealed **51 pre-existing clippy violations across 8 crates** (none introduced by the boundary refactor; refactor-touched crates were already clean). User requested fix-and-reverify before moving to Phase 2. Worker 13 closed at commit `15bb6df`.

**Scylla perf benchmark marked `#[ignore]`:** `perf_40_segments_1m_entries_each` in `crates/stitchd-db/tests/scylla_perf_e2e.rs` writes **40 million rows** (40 segments × 1M entries) to ScyllaDB — minutes per run. Now `#[ignore]`d by default; opt in with `cargo test -p stitchd-db --test scylla_perf_e2e -- --ignored`.

**Per-crate clippy: 51 → 0.** `cargo clippy --workspace --all-targets -- -D warnings` exits 0 across:
- stitchd-core (2 → 0)
- stitchd-db (3 → 0; scylla `assert_eq!(x, bool_literal)` → `assert!(x)` / `assert!(!x)`)
- stitchd-events (2 → 0; `useless_format` in clickhouse_views test)
- stitchd-auth-service (10 → 0; `cast u32 to u64`, missing `# Errors`, `#[must_use]`)
- stitchd-flag-service (6 → 0; `result_large_err`, `approx_constant`)
- stitchd-segmentation-service (25 → 0; largest; 469-line refactor of sweeper test module restructuring inline test mod into top-level `#[tokio::test]` fns)
- stitchd-gateway (2 → 0; `stub_clients()` `type_complexity` resolved via type alias — closes the originally-filed `feature-flag-ysh`)
- stitchd-sdk (1 → 0; only triggers without `--features test-util`)

**Patterns / gotchas:**

- **One `cargo clippy --workspace --all-targets --fix --allow-dirty -- -D warnings` pass resolved everything.** Auto-fix is remarkably effective for the lint families that dominate stale codebases: `useless_format`, `approx_constant`, `cast_lossless`, `assert_eq!(x, bool_literal)`, `result_large_err` (it adds `#[allow]` with comment when boxing would change public API). Worth keeping in the dev workflow.

- **`#[allow(clippy::result_large_err)]` is the right escape hatch for tonic returns.** `stitchd-flag-service/src/sdk_backend.rs::env_id_from_metadata()` returns `tonic::Status` (external type, large variant); boxing it would change the public gRPC API surface. The `#[allow]` with a one-line justification comment is the correct fix — auto-fix even applies it correctly with the comment included.

- **`cargo clippy --fix` chooses `.to_string()` over `.to_owned()` for `useless_format` literal conversions.** Both are idiomatic; the auto-fix picks `.to_string()`. Acceptable.

- **`stitchd-segmentation-service/src/sweeper/tests.rs` 469-line restructure.** Auto-fix triggered on `useless_let_if_seq` / `needless_pass_by_ref_mut` and restructured an inline test module into top-level `#[tokio::test]` fns. Large diff but mechanical — behaviour preserved. Worth a code review if you care about the test layout style.

- **Pre-existing E2E infra-dependent failure (NOT a regression):** `stitchd-flag-service::evaluate_preview_writes_rows_to_clickhouse` requires a running flag-service daemon on `:50052`. `FLAG_SERVICE_ADDR` is set in `.env.local`, which prevents the test from self-skipping when no daemon is actually running. This is pre-existing behaviour. Two clean fixes either: (a) auto-skip when the daemon isn't reachable, or (b) wrap in an `#[ignore = "needs running flag-service daemon"]` like the perf bench. Filing as a separate follow-up beads issue is reasonable.

- **Per-crate test invocation discipline.** Per user preference, every verification ran `cargo test -p <crate>` instead of `cargo test --workspace`. Slightly slower wall-clock (no parallel test execution across crates), but much clearer signal when a single crate breaks.

---
