# Spec: Admin UI — Multi-Tenant Routing & API Integration

## Overview

Restructure the existing Vite + React admin console (`admin/`) into two top-level
sections: `/superadmin` (system-org users) and `/org/:orgId` (per-org users).
Auth is a single JWT flow — `is_system` on the token determines which section a
user is routed to after login. All screens are wired to real gateway APIs.
A new `admin` Docker Compose service enables end-to-end local testing.

## Functional Requirements

### 1. Routing Architecture

**Public routes:**
- `/login` — unified login page (password / OIDC / SAML tabs, existing design)

**Superadmin section** (requires `is_system = true`):
- `/superadmin` — dashboard / landing
- `/superadmin/orgs` — org list; create org (`POST /v1/admin/orgs`)
- `/superadmin/orgs/:orgId/users` — seed first user (`POST /v1/admin/orgs/:orgId/users`)
- `/superadmin/orgs/:orgId` — org detail (projects, environments, members overview)

**Per-org section** (requires `is_system = false`, `org_id` matches token `tenant_id`):
- `/org/:orgId` — org dashboard
- `/org/:orgId/flags` — feature flags (wired to API)
- `/org/:orgId/flags/:flagId` — flag detail (wired to API)
- `/org/:orgId/segments` — segments (wired to API)
- `/org/:orgId/segments/:segmentId` — segment detail (wired to API)
- `/org/:orgId/experiments` — experiments (wired to API)
- `/org/:orgId/experiments/:experimentId` — experiment detail + results (wired to API)
- `/org/:orgId/events` — event definitions (wired to API)
- `/org/:orgId/environments` — environments & SDK keys (wired to API)
- `/org/:orgId/members` — members & roles
- `/org/:orgId/audit` — org-scoped audit log

### 2. Authentication & Authorization

- Login calls `POST /v1/auth/login` with `{ email, password, org_id? }`
- Response: `{ access_token, org_id, expires_in }` — stored in `localStorage`
- Token is decoded client-side (JWT payload) to read `is_system`
- Post-login redirect:
  - `is_system = true` → `/superadmin`
  - `is_system = false` → `/org/:orgId` (using `org_id` from response)
- 401 responses from any API call → redirect to `/login`
- 403 responses → show "Access Denied" inline (no redirect)

### 3. Org Switching

- Users with access to multiple orgs see an org switcher in the sidebar
- Switching re-calls `POST /v1/auth/login` with the selected `org_id`
- New token + new `org_id` replaces the current session; re-routes to `/org/:newOrgId`
- Org history tracked in localStorage (`stitchd_org_history: [{orgId, orgName}]`)
- Re-login modal prompts for email + password (no stored credentials)

### 4. Route Guards

- `<SuperAdminGuard>` — wraps `/superadmin/*`; redirects to `/login` if no token,
  to `/org/:orgId` if token exists but `is_system = false`
- `<OrgGuard>` — wraps `/org/:orgId/*`; redirects to `/login` if no token,
  to `/superadmin` if `is_system = true`
- Guards run client-side on every navigation

### 5. API Client Updates

- Axios instance base URL from `VITE_API_BASE_URL`
- `org_id` injected from current session context (not from URL) for API calls
- All org-scoped resource routes (`/v1/projects/:projectId/flags`, etc.) require
  a `project_id` / `env_id` — these are resolved from org context state
- Auth provider routes: `GET/POST /v1/orgs/:orgId/auth-providers` wired in org settings

### 6. Docker Compose Integration

- New `admin` service in `docker-compose.yml`:
  - Builds from `admin/Dockerfile` (multi-stage: `vite build` → nginx)
  - `VITE_API_BASE_URL` points to gateway service (nginx proxy handles `/api`)
  - Nginx proxies `/api` → `http://gateway:8080`; serves static build otherwise
  - Exposed on `localhost:5173`
  - Depends on `gateway` service (health-checked)
- `admin/Dockerfile` added (multi-stage: node build → nginx:alpine serve)

## Non-Functional Requirements

- TypeScript strict mode throughout
- All route guards enforce auth — no unguarded `/superadmin` or `/org` routes
- API error states (loading, error, empty) handled on every wired screen
- `VITE_API_BASE_URL` controls gateway endpoint; defaults to `/api` (nginx proxy)

## Acceptance Criteria

- [ ] Login with `is_system=true` credentials lands on `/superadmin`
- [ ] Login with `is_system=false` credentials lands on `/org/:orgId`
- [ ] Accessing `/superadmin/*` as an org user redirects to `/org/:orgId`
- [ ] Accessing `/org/:orgId/*` as a superadmin redirects to `/superadmin`
- [ ] Superadmin can create an org via `/superadmin/orgs`
- [ ] Superadmin can seed a user into an org
- [ ] Org user sees flags list loaded from `GET /v1/projects/:projectId/flags`
- [ ] Org user sees segments list loaded from `GET /v1/environments/:envId/segments`
- [ ] Org user sees experiments list loaded from `GET /v1/environments/:envId/experiments`
- [ ] Org switcher triggers re-login with new `org_id` and re-routes
- [ ] `docker compose up` starts full stack including admin UI on `localhost:5173`
- [ ] Full round-trip: create org (superadmin) → seed user → log in as that user → see org dashboard

## Out of Scope

- List-my-orgs API (org switcher uses locally tracked orgs from login history)
- Flag / segment / experiment CREATE or EDIT forms
- Mobile / responsive layout
- Unit or integration tests for React components
- Audit log wiring (mock data)
- Members / roles wiring (mock data)
