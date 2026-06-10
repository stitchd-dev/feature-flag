# Spec: Members & Roles — Real Data

**Track ID:** `members_roles_20260610`
**Type:** Feature (UI ↔ backend wiring; close UI-lags-backend deviation)
**Beads:** `feature-flag-kui` (discovered via wisp `feature-flag-wisp-99m`)

## Overview

The admin UI `Members & Roles` page (`/org/:orgId/members`, rendered by
`admin/src/pages/stubs.tsx::Members`) is entirely mock-driven. It imports
`MEMBERS` from `admin/src/lib/mockData.ts`, hardcodes tab counts, shows a fake
"custom role" card, and renders a "Coming soon" empty state for the Roles,
Pending invites, and SSO tabs.

The backend is **ahead** of this UI. The gateway already exposes org-scoped
management endpoints that this page should consume:

- `GET    /v1/management/orgs/{org_id}/users` → `CursorPage<OrgUserJson{user_id,email,display_name,role,created_at}>`
- `POST   /v1/management/orgs/{org_id}/users` → body `{email,display_name,password,org_role?}`
- `DELETE /v1/management/orgs/{org_id}/users/{user_id}` → 204

(These are distinct from the **superadmin** `/v1/superadmin/orgs/...` routes the
API client currently wraps — those require a System org and are wrong for the
org-admin Members page.)

The backend also exposes a complete, **already-wrapped-but-unused** Auth Provider
CRUD surface in `admin/src/lib/api.ts`:

- `GET/POST /v1/orgs/{org_id}/auth-providers`, `GET/PUT/DELETE .../{id}`
- `GET /v1/orgs/{org_id}/auth-providers/{id}/saml/metadata`

This track makes the Members page real: live member list, invite, role change,
remove, and a working SSO providers tab backed by the auth-provider API. Tabs
that have no backing capability are honestly removed rather than left as
"coming soon".

## Functional Requirements

### FR1 — Member management API client
Add org-scoped management client functions to `admin/src/lib/api.ts`:
- `listOrgMembers(orgId, cursor?, signal?)` → paginated `OrgUserSummary[]` via
  `GET /v1/management/orgs/{org_id}/users` (cursor pagination, opaque token).
- `createOrgMember(orgId, {email, display_name, password, org_role})` via
  `POST /v1/management/orgs/{org_id}/users`.
- `removeOrgMember(orgId, userId)` via `DELETE /v1/management/orgs/{org_id}/users/{user_id}`.
These MUST NOT reuse the superadmin-scoped `listOrgUsers`/`removeOrgUser`/`seedUser`
functions (different route tier + auth requirement).

### FR2 — Members tab (real data)
- Replace the mock `MEMBERS` table with a live fetch via `listOrgMembers(orgId)`.
- Each row shows display name, email, role badge (`org_admin`/`org_member`),
  joined date. Derive initials/avatar deterministically from name/email (no mock
  `color`/`initials`/`projects`/`mfa`/`last` fields — those have no backend source
  and must go).
- Loading, empty, and error states per existing UI conventions (spinner label,
  empty card, error banner).
- Tab count reflects the real member count.

### FR3 — Invite / create member
- "Invite member" opens a modal/form: email (required, validated),
  display name (required), initial password (required for this credential-based
  backend), role select (`org_admin` | `org_member`, default `org_member`).
- On submit, call `createOrgMember`; on success refresh the list and toast;
  on error surface the gateway message (e.g. duplicate email).
- Form validation client-side (Yup, matching the project's existing form style).
- Remove the non-functional "Bulk invite" button (no backend bulk endpoint),
  OR wire it to repeated single creates — prefer removal to avoid a fake control.

### FR4 — Remove member
- Row action to remove a member, behind a confirmation dialog (matching the
  project's destructive-action pattern).
- Guard: do not allow removing the only org_admin / the current user where the
  backend would 4xx — surface the error gracefully if it occurs.

### FR5 — Role display & change
- Render the real `role` value with a human-readable label and badge.
- Inline role change is OUT unless a management RPC exists for it; if no
  `UpdateOrgUserRole`/equivalent exists in the backend, the Roles tab and any
  "change role" control must reflect that honestly (see FR7). Verify backend
  surface during Phase 1 and record the finding.

### FR6 — SSO providers tab (real data)
- Replace "Coming soon" with a live SSO tab backed by the existing auth-provider
  client functions: list providers (`listAuthProviders`), create
  (`createAuthProvider`), edit (`updateAuthProvider`), delete (`deleteAuthProvider`),
  and for SAML providers a "Download SP metadata" action (`getSamlSpMetadata`).
- Support OIDC (issuer/client id/secret/scopes) and SAML (metadata URL/XML,
  NameID format) provider config forms per the `AuthProviderSummary` shape and
  the backend `AuthProviderService` contract.
- Loading/empty/error states.

### FR7 — Honest tabs
- For any tab without a backend capability (e.g. Pending invites if no invite
  table exists; custom Roles if no role-definition API exists), either remove the
  tab or replace its content with a clear, non-deceptive explanation — never a
  "coming soon" placeholder pretending a feature is imminent. Decide per the
  Phase-1 backend audit and record decisions in `learnings.md`.

### FR8 — Decommission mock data
- Remove `MEMBERS` (and any now-unused mock) from `admin/src/lib/mockData.ts`
  once the page no longer imports it. Do not leave dead mock exports.

## Non-Functional Requirements

- **TDD:** Vitest component/unit tests for the new client functions and the
  Members/SSO tab behaviors (loading/empty/error/success), written before impl.
- **Type safety:** No `any` for API responses; typed interfaces in `api.ts`.
- **Parity with conventions:** Match existing page patterns (PageHeader, cards,
  tabs, toasts, confirm dialogs, PermissionGate where writes are gated).
- **Permissions:** Member writes gated client-side by the same permission
  framework used elsewhere; server remains the source of truth.
- **No backend change expected.** If a genuine gap is found (e.g. role-change
  RPC missing and required), STOP and record it; do not fabricate UI for a
  non-existent endpoint.

## Acceptance Criteria

1. Members page lists real org users for the current org via the management
   endpoint; no import of `MEMBERS` mock remains.
2. Inviting a member creates a real user and the new member appears after refresh.
3. Removing a member calls the management DELETE and the row disappears.
4. SSO tab lists/creates/edits/deletes real auth providers and downloads SAML SP
   metadata for SAML providers.
5. No "Coming soon" placeholder remains on the Members page; non-backed tabs are
   removed or honestly explained.
6. `admin/src/lib/mockData.ts` no longer exports member mock data (or it is
   unreferenced and deleted).
7. `npm run` gates pass: `tsc` typecheck, eslint, vitest (all green), `build`.
8. Manual verification: load `/org/:orgId/members`, confirm live members, invite,
   remove, and SSO CRUD all work against a running gateway.

## Out of Scope

- New backend endpoints (member bulk invite, pending-invite store, custom
  role-definition CRUD, MFA status) — flag as follow-ups if needed, do not build.
- Audit log page (separate track, beads `feature-flag-bpc`).
- Superadmin org/user provisioning page (already real, untouched here).
