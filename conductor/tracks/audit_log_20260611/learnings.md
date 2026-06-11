# Track Learnings: audit_log_20260611

## Audit
- PgAuditLogger (db/repository/pg/audit.rs) is constructed ONLY in tests; no
  prod service calls .log(); audit_log table is EMPTY. "Backend writes audit
  rows" (discovery) was wrong — audit is scaffolding-only.
- Gateway propagates only x-env-id downstream (NOT the actor). So per-service
  audit writes would need new actor propagation at every mutation.
- RbacContext (stitchd_proto::auth::v1) on every authed request: subject=actor
  user_id, tenant_id=ORG id, environment_id, roles, permissions, is_system.
  Extracted via req.extensions().get::<RbacContext>().
- Idempotency middleware is the precedent for gateway-edge PgPool state: opt-in
  via STITCHD_DATABASE_URL, layered OUTSIDE build_router in main.rs, self-filters
  to mutation methods, fails open. Audit mirrors this exactly.
- audit_log cols: id, actor_id?, resource_type, resource_id, action, diff,
  created_at — NO org_id (added by this track's migration).
- DB live: STITCHD_DATABASE_URL=postgres://stitchd:stitchd@localhost:5432/stitchd
  (docker compose). sqlx CLI at ~/.cargo/bin/sqlx. SQLX_OFFLINE=true + .sqlx
  cache + CI `cargo sqlx prepare --check` → regenerate cache after new query!.

## Design decision
Gateway-centric capture (no per-service actor threading). Lossy-but-honest v1:
resource_type+action from MatchedPath+method via explicit map; resource_id =
best-effort last UUID path segment else NULL (creates → NULL; created-id +
field diffs are follow-ups). NEVER fabricate.

<!-- impl notes below -->

## Design refinement (Phase 1)
Gateway depends on sqlx but NOT stitchd-db, and owns its edge-state SQL
(idempotency uses sqlx::query! directly via its pool). So ALL audit read+write
lives in the gateway crate via its pool — the stitchd-db AuditLogger trait stays
untouched (test-only scaffolding; no prod churn). Migration (org_id) lives in
stitchd-db/migrations (canonical). Local builds: DATABASE_URL set + SQLX_OFFLINE
unset → query! validates live; run `cargo sqlx prepare --workspace` before commit
so CI (SQLX_OFFLINE=true + prepare --check) passes.

## Verification (Phase 5)
- End-to-end middleware test (#[sqlx::test]): PUT a mapped flag route with an
  injected RbacContext → an audit_log row lands with org/actor/resource_type
  ('flag')/action ('flag.update')/resource_ref ('checkout'). Write is spawned →
  poll-with-retry in the test.
- Gate GREEN (DB up): full gateway cargo test (incl. idempotency pg_store tests
  which need live PG), audit 12 unit + 2 read #[sqlx::test] + 1 middleware
  #[sqlx::test], openapi contract; clippy -D warnings + fmt; `cargo sqlx prepare
  --workspace --check` passes offline; admin tsc/lint(0 err)/vitest 1076/build;
  cargo xtask docs idempotent (no tracked-doc changes).
- Pool unified: created once in main (edge_pool), attached to GatewayState
  (audit_pool, used by read handler) AND the audit+idempotency middleware. Added
  audit_pool:None to from_channels/connect + the events.rs test literal.

## Follow-ups (filed)
- Field-level diffs (capture request/response body changes).
- Capture created-resource id from POST response bodies (currently NULL on creates).
- CSV export; per-entity audit timelines on detail pages.
- Eventually move capture into services for richer diffs (needs actor propagation).
