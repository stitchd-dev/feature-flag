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
