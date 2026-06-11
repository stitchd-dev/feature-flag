# Spec: Audit Log — gateway-centric capture + read + UI

**Track ID:** `audit_log_20260611`
**Type:** Feature (capability gap; audit was scaffolding-only)
**Beads:** `feature-flag-bpc` (wisp `feature-flag-wisp-99m`)

## Overview

Discovery claimed "backend writes audit rows". Reality: `PgAuditLogger`
(`crates/stitchd-db/src/repository/pg/audit.rs`) is constructed **only in
tests** — no production service ever calls `.log()`, and `audit_log` has **0
rows**. There is no read API, and the admin `/org/:orgId/audit` page renders a
**fabricated** audit trail from `mockData.AUDIT` (deceptive).

`AuditLogger::log` takes `(actor_id, resource_type, resource_id, action, diff)`.
The gateway only propagates `x-env-id` to services — it does **not** propagate
the actor — so wiring writes per-service would require new actor propagation
across the trust boundary at every mutation.

**Design — capture at the gateway edge** (same class as the idempotency
middleware: a narrowly-scoped `PgPool`, opt-in via `STITCHD_DATABASE_URL`,
layered outside the router). The gateway already holds an `RbacContext`
(`stitchd_proto::auth::v1::RbacContext`) on every authenticated request:
`subject` = actor user_id, `tenant_id` = org id, `is_system`. So the gateway can
record every successful mutation (actor + org + resource + action) with **no
service changes and no actor threading**. The `audit_log` table gains an
`org_id` column so the read is org-scoped.

This delivers a real, honest audit trail end-to-end and replaces the mock page.

## Functional Requirements

### FR1 — Migration: org-scope the audit table
Add a new dated migration: `audit_log.org_id uuid` (nullable — system/no-org
actions) + a keyset-friendly index `(org_id, created_at DESC, id DESC)`.
Validate against the live dev DB; regenerate the sqlx offline cache.

### FR2 — Gateway audit capture (edge middleware)
- New `audit` module + `audit_middleware` (mirrors `idempotency`): enabled only
  when `STITCHD_DATABASE_URL` is set; layered outside the router; **fail-open**
  (never break the request on an audit-write error).
- Self-filter to mutation methods (`POST`/`PUT`/`PATCH`/`DELETE`) that returned a
  2xx AND carry an `RbacContext` extension (skip SDK-key + unauthenticated +
  read traffic).
- Derive `resource_type` + `action` from the matched route template
  (`axum::extract::MatchedPath`) + method via an explicit route→resource map
  (flag/segment/experiment/member/sdk_key/auth_provider/metric/event/exclusion_group/
  schedule/bandit_campaign/project/environment…). `resource_id` = best-effort
  last UUID path segment, else NULL (creates carry the id in the body — a
  field-level diff + created-id capture are explicit follow-ups; do NOT
  fabricate). `actor_id` = `subject`, `org_id` = `tenant_id` from `RbacContext`.
- Persist via a `PgAuditWriter` using the gateway pool (INSERT into audit_log
  incl. org_id). Unmapped mutation routes are skipped (logged at debug), never
  recorded with a guessed type.

### FR3 — Read endpoint
`GET /v1/orgs/{org_id}/audit?cursor=&limit=&resource_type=&action=` →
keyset-paginated `{ items: AuditEntryJson[], next_cursor }`, newest first.
- `AuditEntryJson { id, actor_id?, actor_email?, resource_type, resource_id?,
  action, created_at }` (resolve actor_id→email best-effort via a join; NULL
  when system/unknown).
- Authorize: caller's `RbacContext.tenant_id` must equal `{org_id}` (or
  `is_system`). Optional `resource_type` / `action` filters. Uses the gateway
  pool.

### FR4 — Admin client + real UI
- `listAuditLog(orgId, { cursor, limit, resource_type, action })` client fn.
- Replace the mock `AuditLog` page: real, paginated, filterable table (When /
  Actor / Action / Resource type / Resource); loading / empty / error states.
  Empty state explains audit capture began when this landed (no back-fill).
- Remove the fabricated `mockData.AUDIT` + `AuditEntry`.

### FR5 — Dead-code purge (same surface)
- Delete the **unrouted** `EventsRegistry` + `SuperAdmin` stub components in
  `pages/stubs.tsx` (superseded by the real EventsList + superadmin pages) and
  the now-unused `mockData.EVENTS` + `Event`. If `mockData.ts` ends up empty,
  delete it.

## Non-Functional Requirements

- **TDD:** gateway unit tests (route→resource mapping is pure + testable;
  middleware filter logic; read handler authz) + a DB-backed integration test
  for the read query; admin client + UI tests (node-env conventions).
- **Full gate (DB up):** migrate + `cargo sqlx prepare --workspace --check`;
  `cargo test -p stitchd-gateway -p stitchd-db` green; clippy `-D warnings` +
  fmt; admin `tsc`/`lint`/vitest/`build`; `cargo xtask docs` idempotent.
- Fail-open audit writes; the audit path must never add latency-critical work to
  the request (best-effort INSERT; consider spawning).

## Acceptance Criteria

1. A real mutation through the gateway (e.g. toggle a flag) inserts an
   `audit_log` row with the actor's user_id + org_id + resource_type + action
   (verified against the live DB).
2. `GET /v1/orgs/{org_id}/audit` returns those rows, org-scoped, keyset-paginated,
   filterable; cross-org access is refused.
3. The admin Audit page shows real entries (no `mockData.AUDIT`); dead stubs +
   unused mock removed.
4. Full gate green (cargo + sqlx-check + admin + docs idempotent).

## Out of Scope (explicit follow-ups, filed in beads)
- Field-level diffs + capturing the created-resource id from response bodies.
- Back-filling historical audit (capture starts at deploy).
- CSV export; per-resource audit timelines on entity pages.
- Auditing SDK-key / unauthenticated traffic (no human actor).
