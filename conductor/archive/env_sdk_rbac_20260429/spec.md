# Spec: Environments & SDK Keys — Full-Stack Functional UI with RBAC

## Overview

The Environments & SDK Keys page is currently a static stub using mock data.
This track makes it fully functional: adds backend LIST/REVOKE/DELETE/RENAME
endpoints for projects, environments, and SDK keys; adds a permissions API +
JWT-claim extraction for RBAC; and wires the Admin UI to real data with
RBAC-compliant greying/locking of actions.

A project picker (create + select) is promoted to a first-class UI element
alongside the org switcher, so users always have full context about which
project they're operating in.

## Functional Requirements

### Backend
1. **Proto additions** — add to `management_service.proto`:
   - `ListProjects(ListProjectsRequest{org_id})` → `ListProjectsResponse{projects[]}`
   - `ListEnvironments(ListEnvironmentsRequest{project_id})` → `ListEnvironmentsResponse{environments[]}`
   - `ListSdkKeys(ListSdkKeysRequest{environment_id})` → `ListSdkKeysResponse{sdk_keys[]}`
   - `RevokeSdkKey(RevokeSdkKeyRequest{sdk_key_id})` → `RevokeSdkKeyResponse{}`
   - `RenameProject(RenameProjectRequest{project_id, name})` → `RenameProjectResponse{}`
   - `DeleteProject(DeleteProjectRequest{project_id})` → `DeleteProjectResponse{}`
   - `RenameEnvironment(RenameEnvironmentRequest{environment_id, name})` → `RenameEnvironmentResponse{}`
   - `DeleteEnvironment(DeleteEnvironmentRequest{environment_id})` → `DeleteEnvironmentResponse{}`
2. **Management service** — implement all eight new RPCs against the existing DB layer.
3. **Gateway routes** — add routes under `mgmt_routes`:
   - `GET    /v1/management/orgs/{org_id}/projects`
   - `PATCH  /v1/management/projects/{project_id}`
   - `DELETE /v1/management/projects/{project_id}`
   - `GET    /v1/management/projects/{project_id}/environments`
   - `PATCH  /v1/management/environments/{env_id}`
   - `DELETE /v1/management/environments/{env_id}`
   - `GET    /v1/management/environments/{env_id}/sdk-keys`
   - `DELETE /v1/management/sdk-keys/{sdk_key_id}`
4. **Permissions API** — add `GET /v1/auth/me/permissions` returning the
   user's effective roles and permissions for the currently-scoped org.

### Frontend
5. **Session enrichment** — on login, `decodeJwtPayload` extracts `roles` and
   `permissions` claims and stores them in the `Session` object.
6. **Permissions hook** — `usePermissions()` hydrates from Session on mount,
   then fetches `/v1/auth/me/permissions` for the authoritative server-side
   view. Exposes `can(action)` helper (`"environment:create"`, `"environment:rename"`,
   `"environment:delete"`, `"sdk_key:create"`, `"sdk_key:revoke"`,
   `"project:create"`, `"project:rename"`, `"project:delete"`, etc.).
7. **Project picker** — a `ProjectPicker` component in the top shell alongside
   the org switcher. Lists all projects for the current org, allows selecting
   one (persisted in `OrgContext`), and supports inline "New project" +
   rename + delete actions (all RBAC-gated).
8. **Environments page** — fully wired:
   - Lists real environments for the selected project.
   - Lists real SDK keys per environment.
   - "New environment", rename, delete, "New key", "Revoke" actions call real
     API endpoints.
   - Delete actions include a confirmation dialog.
   - Empty state (no project selected) prompts user to select or create a
     project via the `ProjectPicker`.
9. **RBAC UI rules**:
   - Any mutating action the user lacks permission for: button disabled +
     tooltip (`"Requires project_admin or higher"`).
   - If the user has **zero** read access to the Environments page: render a
     section-level lock overlay with "Contact your admin to request access."

## Non-Functional Requirements

- New gateway routes follow the existing pattern: Axum handlers, utoipa
  annotations, `mgmt_routes` tree (JWT + `require_non_system_org`).
- `usePermissions` falls back gracefully if the API call fails (uses
  JWT-decoded claims only).
- No new external dependencies; reuse existing design tokens and primitives.
- Delete operations enforce backend constraints (e.g. min-1-active SDK key
  per environment; cannot delete an environment with active experiments).

## Acceptance Criteria

- [ ] `GET    /v1/management/orgs/:org_id/projects` returns real project list.
- [ ] `PATCH  /v1/management/projects/:id` renames project.
- [ ] `DELETE /v1/management/projects/:id` deletes project (with constraints).
- [ ] `GET    /v1/management/projects/:project_id/environments` returns real envs.
- [ ] `PATCH  /v1/management/environments/:id` renames environment.
- [ ] `DELETE /v1/management/environments/:id` deletes environment (with constraints).
- [ ] `GET    /v1/management/environments/:env_id/sdk-keys` returns real keys.
- [ ] `DELETE /v1/management/sdk-keys/:id` revokes a key (enforces min-1-active).
- [ ] `GET    /v1/auth/me/permissions` returns roles + permissions for current user.
- [ ] Session stores `roles`/`permissions` extracted from JWT on login.
- [ ] `usePermissions()` hook returns correct `can()` results for both admin and
  member roles.
- [ ] Project picker in shell: create, select, rename, delete projects.
- [ ] Environments page shows real data when a project is selected.
- [ ] Environments support inline rename and delete with confirmation dialog.
- [ ] Mutating buttons are disabled + have tooltip when user lacks permission.
- [ ] Lock overlay appears when user has no read access to environments.
- [ ] No mock data used anywhere on the Environments page.

## Out of Scope

- Per-environment rule/segment editing (handled by other tracks).
- SDK key rotation UI (create new + revoke old is sufficient).
- Mobile/responsive layout improvements.
