# Implementation Plan: Audit Log (gateway-centric)

Track: `audit_log_20260611` · Beads epic: TBD · Branch: `track/audit_log_20260611`

Methodology: TDD. DB is live (`STITCHD_DATABASE_URL=postgres://stitchd:stitchd@localhost:5432/stitchd`).
Gates: migrate + `cargo sqlx prepare --check`; `cargo test -p stitchd-gateway -p stitchd-db`;
clippy `-D warnings` + fmt; admin `tsc`/`lint`/vitest/`build`; `cargo xtask docs` idempotent.

## Phase 1: Migration — org_id on audit_log

- [x] Task 1.1: Add `crates/stitchd-db/migrations/2026061100xxxx_audit_org_id.sql`
      (ADD COLUMN org_id uuid NULL + index (org_id, created_at DESC, id DESC)).
      Apply to dev DB; verify with `\d audit_log`.
- [x] Task 1.2: Add a keyset `list_audit_log` repo query + extend `AuditLogger::log`
      to accept `org_id` (update the in-tree callers = tests). TDD: a DB-backed
      integration test in `crates/stitchd-db/tests/audit_extended.rs` (insert
      rows w/ org_id, list keyset-paginated + filtered). `cargo sqlx prepare`.
      <!-- files: crates/stitchd-db/src/repository/pg/audit.rs, crates/stitchd-db/src/repository/mod.rs, crates/stitchd-db/tests/audit_extended.rs -->
- [x] Task: Conductor - User Manual Verification 'Phase 1' (Protocol in workflow.md)

## Phase 2: Gateway audit capture middleware

- [x] Task 2.1 (TDD): Pure tests for the route→resource map + action derivation
      (`audit::resource_for(matched_path, method)` → Option<(resource_type, action)>)
      and the should-record filter (method/status/RbacContext presence).
      <!-- files: crates/stitchd-gateway/src/audit.rs -->
- [x] Task 2.2 (Green): Implement `audit` module — `PgAuditWriter` (INSERT via
      pool), `resource_for` map, `audit_middleware` (fail-open, mutation+2xx+RbacContext).
      Layer it in `main.rs` alongside idempotency (same STITCHD_DATABASE_URL gate).
      <!-- files: crates/stitchd-gateway/src/audit.rs, crates/stitchd-gateway/src/main.rs, crates/stitchd-gateway/src/lib.rs -->
- [x] Task: Conductor - User Manual Verification 'Phase 2' (Protocol in workflow.md)

## Phase 3: Read endpoint

- [x] Task 3.1 (TDD): Gateway tests — `GET /v1/orgs/{org_id}/audit` authz
      (tenant match / is_system), filters, keyset shape (stub or pool-backed
      smoke 200/empty).
      <!-- files: crates/stitchd-gateway/src/routes/audit.rs -->
- [x] Task 3.2 (Green): `audit::list_audit` handler + `AuditEntryJson` +
      `CursorPage`, route in `router.rs`, register in `openapi.rs`. Actor email
      resolved via join. `cargo test -p stitchd-gateway` + openapi contract green.
      <!-- files: crates/stitchd-gateway/src/routes/audit.rs, crates/stitchd-gateway/src/router.rs, crates/stitchd-gateway/src/openapi.rs -->
- [x] Task: Conductor - User Manual Verification 'Phase 3' (Protocol in workflow.md)

## Phase 4: Admin client + real UI + dead-code purge

- [x] Task 4.1 (TDD): `listAuditLog` client test; AuditLog page source-contract
      (no mockData, no fabricated rows) + presentational render test.
      <!-- files: admin/src/lib/api.audit.test.ts, admin/src/pages/audit/AuditLog.test.tsx -->
- [x] Task 4.2 (Green): `listAuditLog` in api.ts; real `AuditLog` page
      (`pages/audit/AuditLog.tsx`) — paginated/filterable table, states; route it
      in App.tsx. Delete dead `EventsRegistry`+`SuperAdmin` stubs; remove
      `mockData.AUDIT`/`EVENTS`/`Event`/`AuditEntry`; delete `mockData.ts` if empty.
      <!-- files: admin/src/lib/api.ts, admin/src/pages/audit/AuditLog.tsx, admin/src/App.tsx, admin/src/pages/stubs.tsx, admin/src/lib/mockData.ts -->
- [x] Task: Conductor - User Manual Verification 'Phase 4' (Protocol in workflow.md)

## Phase 5: Full verification

- [x] Task 5.1: End-to-end check against the live stack — perform a mutation,
      confirm an audit_log row, fetch it via the read endpoint. Full gate:
      sqlx-check, cargo test (gateway+db), clippy/fmt, admin gate, docs idempotent.
      Update learnings; file follow-ups (diffs, created-id capture, CSV export).
- [x] Task: Conductor - User Manual Verification 'Phase 5' (Protocol in workflow.md)
