# Track Learnings: audit_capture_fix_20260611

## Root cause (live)
audit_middleware (audit_log_20260611) was layered OUTSIDE build_router via
app.layer(...) in main.rs. The auth_middleware that inserts RbacContext is INSIDE
build_router on the authed route groups. Outer layers run BEFORE inner ones
inbound, and a request's extensions inserted by an inner layer are NOT visible to
an outer layer that already moved `req` into next.run. So audit captured
RbacContext=None → wrote nothing → org-scoped Audit page empty.

## Also confirmed live (separate, follow-up)
Backend services DO write rich audit rows (resource_id + diff) via PgAuditLogger
wired into repos (flag/analytics/management/...), but pass actor_id=None
(gateway never propagated the actor) and don't set org_id. So those rows are
invisible on the org page too. Unifying (propagate actor+org to services, drop
gateway capture) is a larger separate effort — filed as follow-up.

## Fix
Re-layer audit INNER to auth on the authed groups; audit reads RbacContext +
state.audit_pool. Tower: group.layer(audit).layer(auth) → auth outer (inserts
ctx) → audit inner (reads ctx) → handler.

## Live verification (PASS)
After re-layering: real org-admin login → create project/env/flag + toggle →
GET /v1/orgs/{org}/audit returned 4 org-scoped rows (project.create,
environment.create, flag.create, flag.update) each with actor=v@verify.org
(email resolved) and resource_ref captured (flag.update→verify-flag). Cross-org
read → HTTP 401. resource_type=flag filter → 2 rows. Gateway tests 15 audit +
full suite green; clippy -D warnings + fmt; sqlx-check offline.

## Follow-up filed
Unify service-side audit (richer resource_id + diff, currently written with NULL
actor/org and invisible on the org page) by propagating actor+org to services
and making them the sole audit writer — larger cross-service refactor.
