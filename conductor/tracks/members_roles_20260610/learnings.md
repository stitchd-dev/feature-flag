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
