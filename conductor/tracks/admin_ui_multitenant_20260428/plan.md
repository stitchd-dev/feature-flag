# Plan: Admin UI — Multi-Tenant Routing & API Integration
# Track: admin_ui_multitenant_20260428

---

## Phase 1: Auth Foundation & Route Restructure

- [ ] Task 1.1: Extend `auth.ts` — JWT decode utility, session storage for `org_id` + `is_system`
  - Decode JWT payload client-side (base64, no verify — verification is server-side)
  - `auth.setSession({ token, orgId, isSystem })` — stores token + org_id + is_system in localStorage
  - `auth.getSession()` → `{ token, orgId, isSystem } | null`
  - `auth.clearSession()`

- [ ] Task 1.2: Update `api.ts` login flow — store full session post-login
  - Update `loginWithPassword` caller to call `auth.setSession()` with decoded payload
  - Add `getOrgId()` + `isSystem()` helpers exported from `auth.ts`
  - Update 401 interceptor to call `auth.clearSession()` before redirect

- [ ] Task 1.3: Route guards — `SuperAdminGuard` and `OrgGuard` components
  - `SuperAdminGuard`: no token → `/login`; token + `is_system=false` → `/org/:orgId`
  - `OrgGuard`: no token → `/login`; token + `is_system=true` → `/superadmin`
  - Both live in `admin/src/shell/guards.tsx`

- [ ] Task 1.4: Restructure `App.tsx` routing
  - `/superadmin/*` nested under `<SuperAdminGuard>`
  - `/org/:orgId/*` nested under `<OrgGuard>`
  - `/login` + `/auth/callback` remain public
  - Flat routes (`/flags`, `/segments`, etc.) removed

- [ ] Task 1.5: Update `LoginPage` — post-login redirect based on `is_system`
  - After successful login + `auth.setSession()`, read `isSystem`
  - `isSystem=true` → `navigate('/superadmin')`
  - `isSystem=false` → `navigate('/org/${orgId}')`

- [ ] Task: Conductor - User Manual Verification 'Phase 1: Auth Foundation & Route Restructure' (Protocol in workflow.md)

---

## Phase 2: Superadmin Section
<!-- execution: sequential -->
<!-- depends: phase1 -->

- [ ] Task 2.1: Superadmin shell layout
  - `admin/src/shell/SuperAdminShell.tsx` — sidebar with links to orgs, platform audit
  - Reuses existing nav/theme/tweaks infrastructure
  - Route: `/superadmin` redirects to `/superadmin/orgs`

- [ ] Task 2.2: Orgs list screen (`/superadmin/orgs`)
  - `admin/src/pages/superadmin/OrgsList.tsx`
  - "Create Org" form → `POST /v1/admin/orgs` → refresh list
  - Org list seeded from created orgs stored in localStorage
  - Each org row links to `/superadmin/orgs/:orgId`

- [ ] Task 2.3: Seed user screen (`/superadmin/orgs/:orgId/users`)
  - `admin/src/pages/superadmin/SeedUser.tsx`
  - Form: email, display_name, password, org_role
  - Calls `POST /v1/admin/orgs/:orgId/users`
  - Success → show created user details

- [ ] Task 2.4: Org detail screen (`/superadmin/orgs/:orgId`)
  - `admin/src/pages/superadmin/OrgDetail.tsx`
  - Shows org name, ID; links to seed user
  - "Login as this org" shortcut → opens login modal pre-filled with `org_id`

- [ ] Task: Conductor - User Manual Verification 'Phase 2: Superadmin Section' (Protocol in workflow.md)

---

## Phase 3: Org Section — Route Restructure & API Wiring
<!-- execution: sequential -->
<!-- depends: phase1 -->

- [ ] Task 3.1: Org context provider
  - `admin/src/context/OrgContext.tsx` — React context holding `{ orgId, projectId, envId }`
  - `orgId` from URL param (`:orgId`)
  - `projectId` + `envId` seeded from localStorage; environment switcher updates `envId`
  - `useOrgContext()` hook consumed by all org-section screens

- [ ] Task 3.2: Restructure org routes in `App.tsx`
  - Move all existing pages under `/org/:orgId/*`
  - `/org/:orgId` → `<OrgDashboard />`
  - `/org/:orgId/flags`, `/org/:orgId/flags/:flagId`
  - `/org/:orgId/segments`, `/org/:orgId/segments/:segmentId`
  - `/org/:orgId/experiments`, `/org/:orgId/experiments/:experimentId`
  - `/org/:orgId/events`, `/org/:orgId/environments`, `/org/:orgId/members`, `/org/:orgId/audit`

- [ ] Task 3.3: Update sidebar links to use org-prefixed routes
  - `Sidebar.tsx` — all nav links prefixed with `/org/:orgId/`
  - `useOrgContext()` supplies the `orgId` for link construction

- [ ] Task 3.4: Wire flags screens to API
  - `FlagsList.tsx` — `GET /v1/projects/:projectId/flags` via `useOrgContext().projectId`
  - `FlagDetail.tsx` — `GET /v1/projects/:projectId/flags/:flagId`
  - Loading / error / empty states on both screens

- [ ] Task 3.5: Wire segments screens to API
  - `SegmentsList.tsx` — `GET /v1/environments/:envId/segments`
  - `SegmentDetail.tsx` — `GET /v1/environments/:envId/segments/:segmentId`

- [ ] Task 3.6: Wire experiments screens to API
  - `ExperimentsList.tsx` — `GET /v1/environments/:envId/experiments`
  - `ExperimentDetail.tsx` — `GET /v1/environments/:envId/experiments/:experimentId`
  - Results tab — `GET /v1/environments/:envId/experiments/:experimentId/results`

- [ ] Task 3.7: Wire events + environments screens to API
  - `EventsRegistry` stub → `GET /v1/environments/:envId/event-definitions`
  - `Environments` stub → create project, environment, SDK key via management APIs

- [ ] Task: Conductor - User Manual Verification 'Phase 3: Org Section' (Protocol in workflow.md)

---

## Phase 4: Org Switcher
<!-- execution: sequential -->
<!-- depends: phase3 -->

- [ ] Task 4.1: Org switcher component in sidebar
  - `admin/src/shell/OrgSwitcher.tsx`
  - Reads org history from localStorage (`stitchd_org_history`)
  - Dropdown lists previously accessed orgs + "Add org" option

- [ ] Task 4.2: Re-login flow for org switch
  - Selecting a different org → email/password re-entry modal
  - Calls `loginWithPassword(email, password, newOrgId)`
  - On success: `auth.setSession()` + `navigate('/org/:newOrgId')`
  - Updates `stitchd_org_history` with new org entry

- [ ] Task: Conductor - User Manual Verification 'Phase 4: Org Switcher' (Protocol in workflow.md)

---

## Phase 5: Docker Compose Integration
<!-- execution: sequential -->
<!-- depends: -->

- [ ] Task 5.1: `admin/Dockerfile`
  - Stage 1 (`builder`): `node:22-alpine`, `npm ci`, `npm run build`
  - Stage 2 (`server`): `nginx:alpine`, copy `dist/` to `/usr/share/nginx/html`

- [ ] Task 5.2: `admin/nginx.conf`
  - Serve static files from `/usr/share/nginx/html`
  - `location /api` → `proxy_pass http://gateway:8080`
  - SPA fallback: all unmatched → `index.html`

- [ ] Task 5.3: Add `admin` service to `docker-compose.yml`
  - Build context: `./admin`
  - Port: `5173:80`
  - `depends_on: gateway` with health check condition

- [ ] Task: Conductor - User Manual Verification 'Phase 5: Docker Compose Integration' (Protocol in workflow.md)
