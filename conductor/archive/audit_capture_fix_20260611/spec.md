# Spec: Fix audit capture — middleware must see RbacContext

**Track ID:** `audit_capture_fix_20260611`
**Type:** Bug fix (regression in audit_log_20260611, found via live verification)
**Beads:** epic TBD

## Problem (found live)
The `audit_middleware` (audit_log_20260611) is layered OUTSIDE `build_router`, so
it runs BEFORE `auth_middleware` (which inserts `RbacContext` inside the router).
At capture time `req.extensions().get::<RbacContext>()` is `None`, so the middleware
records nothing — the org-scoped Audit page is empty. (Live: a real login →
flag toggle produced no org-scoped audit rows; gateway debug confirmed "no
RbacContext".)

Separately confirmed live: the backend SERVICES already write rich audit rows
(resource_id + diff) via `PgAuditLogger`, but with `actor_id=NULL` and (new)
`org_id=NULL` — so they never appear on the org page either. Unifying those
service-side rows with actor/org propagation is a larger, separate effort
(filed as a follow-up); this track makes the gateway capture actually work so the
page is functional.

## Fix
Layer the audit middleware INNER to `auth_middleware` on the authenticated route
groups so it observes the injected `RbacContext`, sourcing the edge pool from
`GatewayState.audit_pool` (already present).

## Functional Requirements
- FR1: `audit_middleware` takes `State<Arc<GatewayState>>`, reads `state.audit_pool`
  (no-op when `None`); no separate writer Arc / outer layer.
- FR2: In `build_router`, layer it on the authed groups (resource, management,
  superadmin) positioned INNER to each group's `auth_middleware` (auth runs first
  → inserts `RbacContext` → audit reads it).
- FR3: Remove the outer audit layer + the now-unused `PgAuditWriter` Arc plumbing
  from `main.rs` (keep `PgAuditWriter` type for the write).
- FR4: Live: a real authed mutation records an org_id + actor_id audit row visible
  via `GET /v1/orgs/{org}/audit` and the Audit page.

## Acceptance Criteria
1. Live smoke: org-admin creates a project/env/flag + toggles → those appear in
   `GET /v1/orgs/{org}/audit` with the correct org_id + actor_id.
2. Gateway tests (incl. the end-to-end middleware #[sqlx::test]) green; clippy/fmt;
   sqlx-check; admin gate unaffected.

## Out of Scope (follow-up)
- Unifying service-side audit rows (richer diff/resource_id) by propagating
  actor+org to services and dropping the gateway capture.
