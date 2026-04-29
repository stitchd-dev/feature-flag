# Implementation Plan: env_sdk_rbac_20260429

## Phase 1: Backend — Proto Extensions & Management Service
<!-- execution: sequential -->
<!-- depends: -->

- [x] Task 1: Extend `management_service.proto` — add 8 new request/response
  message types and RPC definitions (ListProjects, ListEnvironments,
  ListSdkKeys, RevokeSdkKey, RenameProject, DeleteProject, RenameEnvironment,
  DeleteEnvironment)
  <!-- files: proto/management/v1/management_service.proto --> [38e37f3]

- [x] Task 2: Write failing tests for the 4 List + RevokeSdkKey RPCs in the
  management service crate
  <!-- files: crates/stitchd-auth-service/tests/management_list.rs --> [d1ab9ae]

- [x] Task 3: Implement ListProjects, ListEnvironments, ListSdkKeys,
  RevokeSdkKey against the existing DB layer
  <!-- files: crates/stitchd-auth-service/src/management.rs --> [8f8e407]

- [x] Task 4: Write failing tests for RenameProject, DeleteProject,
  RenameEnvironment, DeleteEnvironment RPCs
  <!-- files: crates/stitchd-auth-service/src/management.rs --> [fc405cb]

- [x] Task 5: Implement RenameProject, DeleteProject, RenameEnvironment,
  DeleteEnvironment (enforce constraints: min-1-active SDK key, no delete
  of environment with active experiments)
  <!-- files: crates/stitchd-auth-service/src/management.rs --> [8f8e407]

- [x] Task: Conductor - User Manual Verification 'Backend — Proto Extensions & Management Service' (Protocol in workflow.md)

## Phase 2: Backend — Gateway Routes & Permissions API
<!-- execution: sequential -->
<!-- depends: phase1 -->

- [x] Task 1: Write failing tests for all 8 new management route handlers
  <!-- files: crates/stitchd-gateway/src/routes/management.rs --> [cc5bb7e]

- [x] Task 2: Implement 8 new handlers in management.rs
  (GET projects, PATCH/DELETE project, GET environments, PATCH/DELETE
  environment, GET sdk-keys, DELETE sdk-key)
  <!-- files: crates/stitchd-gateway/src/routes/management.rs --> [cc5bb7e]

- [x] Task 3: Write failing test for GET /v1/auth/me/permissions
  <!-- files: crates/stitchd-gateway/src/routes/auth.rs --> [cc5bb7e]

- [x] Task 4: Implement GET /v1/auth/me/permissions — extract RbacContext from
  request extension, return roles + permissions as JSON
  <!-- files: crates/stitchd-gateway/src/routes/auth.rs --> [cc5bb7e]

- [x] Task 5: Register all new routes in router.rs under mgmt_routes
  <!-- files: crates/stitchd-gateway/src/router.rs --> [cc5bb7e]

- [x] Task: Conductor - User Manual Verification 'Backend — Gateway Routes & Permissions API' (Protocol in workflow.md)

## Phase 3: Frontend — Permissions Layer
<!-- execution: sequential -->
<!-- depends: -->

- [x] Task 1: Extend Session type and decodeJwtPayload to extract and store
  roles + permissions claims from JWT on login
  <!-- files: admin/src/lib/auth.ts --> [ac47dcd]

- [x] Task 2: Add typed API functions for all new endpoints (listProjects,
  listEnvironments, listSdkKeys, createEnvironment, renameEnvironment,
  deleteEnvironment, createSdkKey, revokeSdkKey, renameProject, deleteProject,
  getMyPermissions)
  <!-- files: admin/src/lib/api.ts --> [ac47dcd]

- [x] Task 3: Define permission constant strings and Action type
  (e.g. "project:create", "project:rename", "project:delete",
  "environment:create", "environment:rename", "environment:delete",
  "sdk_key:create", "sdk_key:revoke", "environment:read")
  <!-- files: admin/src/lib/permissions.ts --> [ac47dcd]

- [x] Task 4: Implement usePermissions() hook — seeds from Session on mount,
  fetches /v1/auth/me/permissions for authoritative view, exposes can(action)
  <!-- files: admin/src/hooks/usePermissions.ts --> [ac47dcd]

- [x] Task: Conductor - User Manual Verification 'Frontend — Permissions Layer' (Protocol in workflow.md)

## Phase 4: Frontend — Project Picker Component
<!-- execution: sequential -->
<!-- depends: phase3 -->

- [x] Task 1: Build ProjectPicker component — lists all org projects, highlights
  selected, inline create/rename/delete with RBAC gating and confirmation dialogs
  <!-- files: admin/src/shell/ProjectPicker.tsx --> [09482ef]

- [x] Task 2: Wire ProjectPicker into the shell Sidebar alongside the org switcher;
  update OrgContext to load the project list on mount
  <!-- files: admin/src/shell/Sidebar.tsx, admin/src/context/OrgContext.tsx --> [09482ef]

- [x] Task: Conductor - User Manual Verification 'Frontend — Project Picker Component' (Protocol in workflow.md)

## Phase 5: Frontend — Environments Page & RBAC UI
<!-- execution: sequential -->
<!-- depends: phase2, phase4 -->

- [x] Task 1: Extract Environments into its own file; remove all mock-data imports
  <!-- files: admin/src/pages/environments/Environments.tsx, admin/src/pages/stubs.tsx --> [adde4ac]

- [x] Task 2: Wire environment list to real API; implement inline rename and
  delete with confirmation dialog
  <!-- files: admin/src/pages/environments/Environments.tsx --> [adde4ac]

- [x] Task 3: Wire SDK keys table to real API; implement create-key flow
  (show raw key once on creation) and revoke with confirmation
  <!-- files: admin/src/pages/environments/Environments.tsx --> [adde4ac]

- [x] Task 4: Apply RBAC disabled states with tooltips to all mutating actions
  (New environment, Rename, Delete, New key, Revoke) using usePermissions()
  <!-- files: admin/src/pages/environments/Environments.tsx --> [adde4ac]

- [x] Task 5: Implement section-level lock overlay for users with no
  environment:read permission
  <!-- files: admin/src/pages/environments/Environments.tsx --> [adde4ac]

- [x] Task 6: Empty state — no project selected; show prompt with
  ProjectPicker CTA and inline create-project shortcut
  <!-- files: admin/src/pages/environments/Environments.tsx --> [adde4ac]

- [x] Task 7: Update App.tsx to import from new file path
  <!-- files: admin/src/App.tsx --> [adde4ac]

- [x] Task: Conductor - User Manual Verification 'Frontend — Environments Page & RBAC UI' (Protocol in workflow.md)
