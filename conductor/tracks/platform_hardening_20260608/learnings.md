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
