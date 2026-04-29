# Plan: Admin UI — Multi-Tenant Routing & API Integration
# Track: admin_ui_multitenant_20260428
# Status: COMPLETE — merged to main @ a13077a, post-merge fixes through 4adde54

---

## Phase 1: Auth Foundation & Route Restructure

- [x] Task 1.1: Extend `auth.ts` — JWT decode utility, session storage for `org_id` + `is_system` [3cb9025]
- [x] Task 1.2: Update `api.ts` login flow — store full session post-login [3cb9025]
- [x] Task 1.3: Route guards — `SuperAdminGuard` and `OrgGuard` components [c7ef460]
- [x] Task 1.4: Restructure `App.tsx` routing [c7ef460]
- [x] Task 1.5: Update `LoginPage` — post-login redirect based on `is_system` [c7ef460]
- [x] Verification: Login routes correctly by is_system flag [c7ef460]

---

## Phase 2: Superadmin Section

- [x] Task 2.1: Superadmin shell layout — shared Sidebar with `isOrgSection` context detection [8f392eb]
- [x] Task 2.2: `OrgsList.tsx` — create org via `POST /v1/admin/orgs`, localStorage cache [8f392eb]
- [x] Task 2.3: `SeedUser.tsx` — two-mode form: add existing user or create new [9d58ead]
- [x] Task 2.4: `OrgDetail.tsx` — org overview + switch-to-org confirmation modal [ca12744]
- [x] Fix: Superadmin sidebar showing org nav on `/superadmin/orgs/:orgId` [ca12744]
- [x] Fix: `SeedUserBody.display_name` / `.password` made optional in gateway (422 fix) [6777eaa]
- [x] Fix: Same user seedable into multiple orgs (find-or-create in management service) [9fd0477]

---

## Phase 3: Org Section — Route Restructure & API Wiring

- [x] Task 3.1: `OrgContext.tsx` — React context with orgId, projectId, envId [8f392eb]
- [x] Task 3.2: `App.tsx` — org routes under `/org/:orgId/*` with `OrgShell` wrapper [8f392eb]
- [x] Task 3.3: `Sidebar.tsx` — org-prefixed nav links, superadmin nav detection via pathname [8f392eb + ca12744]
- [x] Task 3.4: `FlagsList` / `FlagDetail` wired to `GET /v1/projects/:projectId/flags[/:key]` [8f392eb]
- [x] Task 3.5: `SegmentsList` / `SegmentDetail` wired to `GET /v1/environments/:envId/segments[/:key]` [8f392eb]
- [x] Task 3.6: `ExperimentsList` / `ExperimentDetail` wired to experiments API + results [8f392eb]
- [x] Fix: OidcCallback updated to use `auth.setSession()` (removed dead `setToken`) [cbc6ef5]

---

## Phase 4: Org Switcher

- [x] Task 4.1: `OrgSwitcher.tsx` — dropdown reading server-fetched org list (no history) [7792235]
- [x] Task 4.2: Seamless switch via `POST /v1/auth/switch-org` — no password re-entry [7792235]
- [x] Backend: `SwitchOrg` + `ListUserOrgs` RPCs added to auth service proto + impl [7792235]
- [x] Gateway: `GET /v1/auth/me/orgs` + `POST /v1/auth/switch-org` routes [7792235]
- [x] Login: fetches org list post-login and stores in session [7792235]
- [x] Fix: `SwitchOrgResponse` missing `user_id` caused session corruption on switch [242441a]

---

## Phase 5: Docker Compose Integration

- [x] Task 5.1: `admin/Dockerfile` — multi-stage node builder → nginx server [995be38]
- [x] Task 5.2: `admin/nginx.conf` — `/api/` proxy to gateway, SPA fallback [995be38]
- [x] Task 5.3: `admin` service added to `docker-compose.yml` [995be38]
- [x] Fix: `docker/stats-service/Dockerfile` created (was missing, blocked docker builds) [b361d2d]
- [x] Fix: Prometheus port conflicts in flag-service (9052) + experimentation-service (9055) [b361d2d]
- [x] `scripts/dev-start.sh` — convenience script for local development [b361d2d + 4adde54]

---

## Post-Track Fixes (on main after merge)

- [x] Multi-org user seeding — management service find-or-create semantics [9fd0477]
- [x] Gateway `SeedUserBody` optional fields [6777eaa]
- [x] Superadmin sidebar context fix [ca12744]
- [x] OrgSwitcher session corruption fix [242441a]
- [x] dev-start.sh AUTH_ENCRYPTION_KEY default [4adde54]
