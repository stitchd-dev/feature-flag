# Track Learnings: platform_hardening_20260608

Patterns, gotchas, and context discovered during implementation.

## Codebase Patterns (Inherited)

From `conductor/patterns.md` — directly relevant to this track:

- **REST pagination is currently `page`+`per_page` (1-based) → `PaginatedResponse<T>`
  (`{items,total,page,per_page}`)** via shared `gateway::pagination`; internal CH list
  RPCs use `offset`+`limit` at the proto level. **Phase 4 supersedes this pattern** —
  update `patterns.md` line ~214 when the cursor contract lands. (from: domain_boundaries_20260530)
- **Pagination total without second query:** `COUNT(*) OVER()` window function returns
  total in every row. Keyset/cursor pagination drops the total — Phase 4 must decide
  whether `total` is still surfaced or removed from the envelope. (from: db_optim_20260516)
- **Gateway = pure REST↔gRPC translation + cross-cutting, ZERO domain logic.** Idempotency
  middleware (Phase 1) is a legitimate cross-cutting gateway concern (like auth/quota/tracing) —
  it belongs in the gateway, not a service. (from: domain_boundaries_20260530)
- **Migration baseline synthesis: `IF NOT EXISTS` throughout, drop-aware.** Relevant to
  Phase 5 DB-reset tooling and the Phase 1 `idempotency_keys` migration — fresh deploys must
  apply cleanly from the V1 baseline. (from: schema_cutover_20260525)
- **Single-command doc pipeline + idempotency CI gate.** New env var
  (`STITCHD_GATEWAY_IDEMPOTENCY_TTL_SECS`) must flow through the env-vars scraper; run
  `cargo xtask docs && git diff --exit-code`. (from: docs_refresh_20260522)
- **sqlx offline cache discipline:** after adding/removing `query!`s, run
  `SQLX_OFFLINE=false cargo sqlx prepare --workspace -- --all-targets --features
  stitchd-sdk-rust/test-util`. (from: workflow.md)
- **Live-CH stats tests use an EXPLICIT CI `--test` list** in `.github/workflows/ci.yml`
  (Coverage job, "Live-ClickHouse integration tests (stats-service)" step). Phase 3's new
  self-seeding test file MUST be added to that list or CI goes red on next push, invisible
  to local `cargo test --workspace`. (from: seqtest_20260603 / ci-live-ch memory)

## Track-Specific Context

- **feature-flag-uga** (Phase 3): on-demand `run_recompute` (`grpc/service.rs:185`) drives
  per-experiment stats + bandit reallocation via the `ExperimentRecomputer` trait but never
  calls `run_interaction_sweep` (`interaction_compute.rs:934`). The on-demand recomputer
  lacks the CH reader/writer + interaction repo the scheduled tick (`main.rs:422`) holds.
- **feature-flag-7rp** (Phase 5): dev Postgres drifted — baseline checksum mismatch +
  unapplied pendings. CI runs against a fresh-from-scratch DB, so local verification must
  match that to catch issues CI catches.
- **Idempotency privacy (NFR-1):** store hashes the request body for fingerprinting (one-way,
  raw body NOT persisted) and stores the response body (already private-param-free). No new
  `privateParameters` leak surface.

---

<!-- Learnings from implementation will be appended below -->

## [2026-06-08] Phase 5: Fresh-DB Reset Tooling [e604ccb, 84bb55b]
- **Implemented:** scripts/reset_dev_db.sh (PG drop/create/migrate; --all adds CH+Scylla) + `cargo xtask ch-migrate`.
- **Learnings:**
  - Dev PG drift = "different checksum" on V1 baseline (edited after apply) → `sqlx migrate run` halts, later migrations pending. Only fix is full DROP+recreate (no in-place re-checksum). `sqlx database drop -y` is non-interactive.
  - ClickHouse Replicated*MergeTree leaves replica registrations in Keeper after a plain DROP DATABASE → recreate fails REPLICA_ALREADY_EXISTS (Code 253). Use `DROP DATABASE … SYNC` AND sweep orphans: enumerate `system.zookeeper WHERE path='/clickhouse/tables/<db>'` and `SYSTEM DROP REPLICA '<replica>' FROM ZKPATH …`. Replica name = `getMacro('replica')` (here "localhost").
  - CH HTTP rejects anonymous DDL (403); dev creds are stitchd:stitchd (curl --user).
  - Canonical CH migrator is `stitchd_event_writer::migrations::run` (2 embedded migrations); the clickhouse-migrations/ dirs are legacy.

## [2026-06-08] Phase 3: On-Demand Interaction Recompute [f29420d]
- **Implemented:** PerExperimentRecomputer.recompute → refresh_interactions → pub(crate) sweep_for_experiment_env → run_interaction_sweep, scoped to the target experiment's env.
- **Learnings:**
  - On-demand recompute (`grpc/service.rs` run_recompute → ExperimentRecomputer trait) drove only per-exp bandit reallocation; interactions only refreshed on the 60-min tick. `run_bandit_reallocation` returns Ok(NotApplicable) for non-bandit exps (no error) so appending the sweep is safe for all experiments.
  - `run_interaction_sweep` groups by environment internally, so passing the env-subset is equivalent to a global sweep but cheaper.
  - GOTCHA: a true live-CH e2e through recompute() is infeasible as a self-seeding #[ignore] test because it calls fetch_running_experiments over the experimentation-service gRPC (not available in self-seeding tests). Tested the env-scoping seam at unit level with fake reader/writer/repos instead → no ci.yml `--test` list change needed.
  - `stitchd_db::RepositoryError` is the re-export (NOT `stitchd_db::repository::RepositoryError`, which is private).

## [2026-06-08] Phase 1: Idempotency-Key Middleware [a00c79a]
- **Implemented:** gateway/idempotency.rs (IdempotencyStore trait + PgIdempotencyStore + axum middleware + sweeper), migration 20260608000003, main.rs wiring.
- **Learnings:**
  - The gateway had ZERO DB access (pure REST↔gRPC). Idempotency needs durable state → gateway gains a narrowly-scoped PgPool (documented tech-stack deviation). Layered in main.rs (NOT build_router) so it applies globally without touching build_router's many test callers; disabled when STITCHD_DATABASE_URL unset (fail-safe + backward compat).
  - Scope from SHA-256(Authorization header) avoids any middleware-ordering dependency (works regardless of whether auth ran first).
  - axum body buffering: `req.into_parts()` → `axum::body::to_bytes(body, LIMIT)` → fingerprint → `Request::from_parts(parts, Body::from(bytes))`; same for the response to capture+replay.
  - Concurrency via `INSERT … ON CONFLICT (scope,key) DO NOTHING` (rows_affected==1 ⇒ we own it); in-flight (NULL status) → 409; completed → replay; different request_hash → 422.
  - GOTCHA (docs scraper): `cargo xtask docs` env-var scraper only matches LITERAL `env::var("STITCHD_…")` / `env_or(...)`, NOT `env::var(CONST)`. Inline the literal (kept const + debug_assert_eq) so the var lands in env-vars.md. (Existing const-declared STITCHD_EVENT_QUOTA_PER_SEC is undocumented for this reason.)
  - GOTCHA (#![deny(warnings, clippy::all)] in gateway): clippy::type_complexity errors on a Mutex<HashMap<(String,String),(String,Option<_>)>> — factor into `type` aliases.
  - Deviation: response_body stored as BYTEA + content_type (not jsonb) for content-agnostic byte-exact replay.

## [2026-06-08] Phase 2: SDK/Event-Ingest Idempotency [f8d6d91]
- **Implemented:** SDK stamps Idempotency-Key header on both event POST paths; server-side dedup IS the Phase 1 gateway middleware (no proto change, no new store).
- **Learnings:**
  - Two SDK event paths: EventBuffer (metric events → POST /v1/events/track, inline retry loop replays identical body → fresh v4 UUID per batch reused across retries = fully exactly-once) and HttpEventSink (flag-eval → POST /v1/sdk/events:batch, re-enqueue model → content-derived uuid v5 key dedups exact re-sends only).
  - Reusing Phase 1's middleware avoided proto/schema changes entirely — the SDK just adds a header. SDK uses x-sdk-key (no Authorization) → shares the middleware "anon" scope; safe because keys are globally unique UUIDs.
  - GOTCHA: uuid::Uuid::new_v5 needs the `v5` feature (workspace uuid only had v4/serde); add `features=["v5"]` on the crate's dep line.
  - GOTCHA (clippy): MutexGuard held across .await — clone-and-drop the guard in a block before awaiting.
  - GOTCHA (shell): backticks inside a double-quoted `git commit -m "..."` are command-substituted by zsh — corrupted "`v5`"→"". Use single quotes or avoid backticks in -m.
  - Eval log is plain MergeTree (no evaluation_id — retired in schema cutover), so per-event CH dedup would need a schema change; batch-level header dedup chosen instead.

## [2026-06-08] Phase 4: Cursor Pagination — FOUNDATION ONLY [67c76f8]
- **Implemented:** Task 1 (tech-stack cursor contract) + Task 2 (gateway::pagination cursor primitives: CursorParams/CursorPage/encode_cursor/decode_cursor + 6 tests). Additive — page-based types untouched.
- **Tasks 3-5 NOT done (scoped, deferred):** the breaking full-stack sweep.
- **Learnings / why deferred:**
  - Surface measured: 78 `_paginated` repo methods across 30 files, 5 proto files, 8 gateway route files, 24 Admin UI files + shared Pagination.tsx/usePaginatedList.ts.
  - Because the gateway is a pure proxy, cursor pagination is NOT gateway-local: keyset logic lives in each service's repo, passed via proto, so the sweep needs proto cursor fields + every service repo's keyset query + every route + all UI — it's a coordinated breaking change.
  - It REVERSES domain_boundaries_20260530's deliberate page-based canonical and removes Admin UI page-number navigation (keyset can't random-access an arbitrary page) — a UX regression the team had deliberately chosen against. This warrants explicit review, not an autonomous rush.
  - Decision: deliver the correct, tested, additive foundation (breaks nothing, CI-green) and recommend the breaking sweep as a dedicated reviewed effort. Rushing 78 repo methods + 5 protos + 24 UI files half-broken would violate the keep-CI-green / don't-ship-broken principle.
  - Opaque token = base64url(JSON(keyset)); `from_overfetch` (fetch limit+1, surplus row ⇒ next_cursor) is the repo idiom for the sweep.

## [2026-06-08] Phase 4 COMPLETE: Cursor Pagination [734ff03,229c0ab,66b9568]
- **Implemented:** cursor REST contract end-to-end. Gateway: CursorParams::offset()/CursorPage::from_offset (opaque encoded-offset over existing offset RPCs). 8 top-level list routes migrated. Admin UI: usePaginatedList + Pagination rewritten cursor-style + 6 views + api clients. product.md + tech-stack.md updated.
- **Learnings:**
  - The real surface was ~10 endpoints / 6 repo methods / 11 RPCs / a SHARED Admin UI hook+component driving 6 views — NOT 78 distinct rewrites (the grep count was inflated by callers/tests). Always measure distinct work units before estimating.
  - Chose opaque-encoded-offset at the gateway (zero proto/repo churn) over true keyset (6 repos + 11 protos) — delivers the guidelines-mandated cursor CONTRACT with bounded risk + CI-green; true keyset internals filed as follow-up feature-flag-cj5 (changes only the token payload, not the contract).
  - GOTCHA (cross-layer consistency): the route agent migrated list_iterations + list_exposures to cursor, but the UI agent kept those detail sub-lists page-based (they need `total` for the exposure-count stat; cursor omits total). tsc + mocked vitest do NOT catch UI↔backend contract mismatches. Resolved by reverting those 2 detail endpoints to page-based — cursor applies to top-level resource collections; detail sub-lists stay page-based by design.
  - OpenAPI contract-check (scripts/check_openapi_contract.py) only checks (METHOD, path) pairs — query-param changes (page/per_page→cursor/limit) don't break it. openapi.json is gitignored/ephemeral.
  - Parallel delegation worked well: one agent for the gateway route layer (9 handlers + tests, 288 green), one for the Admin UI (15 files, 994 vitest green); orchestrator did shared primitives + cross-layer reconciliation.

## [2026-06-08] REVISION #1
- **Type:** Spec (FR-4 cursor pagination)
- **Trigger:** FR-4.3's per-repo keyset rewrite (6 repos + 11 protos) was too large/risky for one CI-green pass and reverses domain_boundaries; detail sub-lists need `total` (cursor envelope omits it).
- **Learning:**
  - Gotcha: a "migrate to keyset" requirement conflates the user-facing CONTRACT (opaque cursor → {items,next_cursor}) with an internal OPTIMIZATION (drop OFFSET). Separating them lets you ship the mandated contract immediately (encoded-offset cursor) and defer the perf win (true keyset) as a contract-preserving follow-up.
  - Pattern: when a spec mandates a paradigm change, deliver the CONTRACT first over existing internals; treat the internal rewrite as a separate, scoped, deferrable optimization. And scope "all list endpoints" to top-level resource collections — detail sub-lists with count/total dependencies are legitimately excluded.

## [2026-06-09] REVISION #2 — true keyset implemented (feature-flag-cj5)
- **Type:** Spec + Plan + code (10 commits)
- **Learning:**
  - Gotcha: a keyset cutover SILENTLY removes any "page:0 = give me everything" mode. Grep EVERY caller of the changed list RPC — internal consumers (e.g. dependency-graph) that relied on the unbounded list must be converted to page through the cursor, or they truncate at the default limit.
  - Gotcha: keyset needs a UNIQUE total order — `ORDER BY created_at` alone is ambiguous; add `, id`. CRUCIAL: keyset the cursor on the SAME columns the list is already ORDERed by, NOT a blanket `created_at`. org-users was email-ordered → it keysets on `(email, id)` via a dedicated `EmailKeysetCursor` (email is globally unique ⇒ stable); defaulting it to `(created_at, id)` would have silently re-sorted the list. Always check each list's existing ORDER BY before picking the keyset columns.
  - Gotcha: `limit as usize` trips clippy::cast_possible_truncation under -D warnings — use `usize::try_from(limit).unwrap_or(usize::MAX)`.
  - Gotcha (env): a CH `DROP DATABASE` (reset_dev_db.sh --all) can leave Keeper replica state inconsistent → later self-seeding analytics tests fail KEEPER_EXCEPTION on Replicated*MergeTree MVs. Re-running the reset (SYNC drop + orphan sweep) clears it; CI (fresh CH) is unaffected.
  - Pattern: opaque cursor tokens make the keyset cutover REST-contract-preserving — the UI/clients never change when offset→keyset internals swap. Own the token format in the repo (next to the SQL), forward it untouched through proto + gateway.
  - Pattern: verify keyset correctness with a multi-page test that pages through N>2 pages and asserts every row visited exactly once (no gaps/dupes) — catches off-by-one boundary + tiebreaker bugs that single-page tests miss.
