# Track Learnings: members_roles_20260610

Patterns, gotchas, and context discovered during implementation.

## Codebase Patterns (Inherited)

- **ID type name:** org identifier is `OrganisationId` (not `OrgId`). (auth_20260421)
- **Enum privilege ordering:** `OrgMember=0, OrgAdmin=1` → `OrgAdmin > OrgMember`. (auth_20260421)
- Admin UI cursor pagination uses opaque tokens; UI passes them through unchanged. (platform_hardening_20260608)

## Backend audit (Phase 1) — to be filled

- Management member routes confirmed in `crates/stitchd-gateway/src/routes/management.rs`:
  - `GET  /v1/management/orgs/{org_id}/users` → `CursorPage<OrgUserJson{user_id,email,display_name,role,created_at}>`
  - `POST /v1/management/orgs/{org_id}/users` → `CreateUserBody{email,display_name,password,org_role?}`
  - `DELETE /v1/management/orgs/{org_id}/users/{user_id}` → 204
- Auth-provider CRUD already wrapped in `admin/src/lib/api.ts` (listAuthProviders, createAuthProvider, updateAuthProvider, deleteAuthProvider, getSamlSpMetadata) but unused by any page.

<!-- Learnings from implementation will be appended below -->

## Phase 1 findings (2026-06-10)

ManagementService RPCs (proto/management/v1) for users are ONLY:
`CreateUser`, `ListOrgUsers`, `RemoveOrgUser`. There is:
- **No role-change RPC** (UpdateUser/ChangeRole absent). `org_role` is set at
  creation only → role rendered **read-only**; no inline "change role" control.
- **No pending-invite store / invite flow.** `CreateUser` directly provisions a
  credentialed user (email + display_name + password + org_role). It is NOT an
  email invite → the action is labelled **"Add member"**, not "Invite", to avoid
  implying an email is sent. The "Pending invites" tab is **removed**.
- **No custom-role-definition API.** RBAC is the fixed `org_admin`/`org_member`
  enum (stitchd-db role.rs; OrgAdmin>OrgMember). The fake "custom role:
  payments-write" card is **removed**. The "Roles" tab is replaced with an
  honest static description of the two-role model (real info, not a placeholder).
- **No bulk-invite endpoint** → the "Bulk invite" button is **removed**.

Follow-up candidates (NOT built this track): role-change RPC, email-invite flow,
custom role definitions, MFA status. File in beads if product wants them.
