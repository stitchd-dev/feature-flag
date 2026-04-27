# Spec: Stitchd Admin UI

## Overview

A standalone Vite + React single-page application providing the admin console for
the Stitchd feature flag and experimentation platform. Lives in `admin/` within
this repository. Communicates with the existing `stitchd-gateway` REST API.
Pixel-perfect implementation of the design prototype in `Stitchd Admin.html`.

## Functional Requirements

### Application Shell
- Vite + React 18 + TypeScript, client-side routing via React Router v6
- Three nav modes: Sidebar / Rail / Top bar (user-configurable via Tweaks panel)
- ⌘K command palette for quick navigation
- Light / dark theme toggle (CSS custom properties, `data-theme` on `<html>`)
- Density toggle: comfortable / compact
- Accent color picker (6 presets)
- Persistent Tweaks state in `localStorage`
- Fonts: Archivo (display), Inter (UI), JetBrains Mono (code/keys)
- Org switcher + environment switcher in sidebar

### Authentication (fully wired to gateway)
- Login screen: Password / OIDC / SAML tabs
  - Password: `POST /auth/login` → JWT stored in `localStorage`
  - OIDC: gateway PKCE redirect flow (`/auth/oidc/authorize` → callback)
  - SAML: gateway redirect (`/auth/saml/initiate`)
- GitHub OAuth + Magic link buttons (UI only in v1)
- JWT attached as `Authorization: Bearer <token>` on all API requests
- 401 responses redirect to `/login`

### Screens & Routes

#### Core screens — wired to real gateway APIs

**`/flags`** — Feature Flags list
- Table / Cards / Grouped-by-segment layout switcher
- Search/filter by key, owner, segment
- Toggle flag on/off inline
- `GET /flags?env=production`

**`/flags/:key`** — Flag Detail
- Tabs: Targeting, Variants, Evaluations (sparkline chart), Experiments, SDK Snippet, History
- Stacked / Side-by-side layout via Tweaks
- Rule targeting: context type(s) pill(s) + recursive AND/OR/NOT condition tree
  - Multi-context rules: clause carries a `ctx:` qualifier tag
  - Leaf clauses: `attribute comparator value`
  - Segment references (colored chip)
  - NOT groups (red left border)
- Variant bar (proportional allocation)
- Staged changes banner when unsaved edits exist
- `GET /flags/:key`, `PUT /flags/:key`

**`/segments`** — Segments list
- `GET /segments`

**`/segments/:key`** — Segment Detail
- Rule-based: condition tree editor (read-only)
- List-based: include/exclude key lists, CSV upload
- `GET /segments/:key`

**`/experiments`** — Experiments list
- State filter: running / draft / stopped / completed
- `GET /experiments`

**`/experiments/:key`** — Experiment Detail
- Frequentist: CI band, p-value, statistical power per arm
- Bayesian: posterior curves, P(best), expected loss per arm
- Multi-variant (3+ arms): pairwise comparison matrix + joint posterior bars
- Auto-selects viz based on experiment model; Tweaks can override
- "Ready to ship" banner when experiment crosses 95% confidence
- `GET /experiments/:key`, `GET /experiments/:key/results`

#### Secondary screens — mock data only

**`/`** — Dashboard: stat cards, sparklines, recent flags table, experiments sidebar, service health list
**`/events`** — Events Registry: event key, type, volume, schema
**`/environments`** — Environments & SDK Keys: env list, key create/revoke
**`/members`** — Members & Roles: user list, role badges, MFA status
**`/audit`** — Audit Log: actor, action, resource, timestamp
**`/super-admin`** — Super Admin: org provisioning, datastores, auth providers

### API Client
- Axios wrapper with base URL from `VITE_API_BASE_URL` env var
- Auth header injected via request interceptor
- 401 interceptor → redirect to `/login`
- Typed response models matching gateway OpenAPI types

## Non-Functional Requirements

- TypeScript strict mode
- ESLint + Prettier configured
- No test infrastructure required in v1 (design-track focus)
- Vite dev proxy to gateway (`/api → http://localhost:8080`)
- Production build: `vite build` → static files in `admin/dist/`

## Acceptance Criteria

- [ ] All 13 screens render with correct layout matching the design prototype
- [ ] Login with password flow completes and stores JWT; subsequent API calls include it
- [ ] OIDC redirect flow initiates (callback handling implemented)
- [ ] Flags list loads from `GET /flags`, supports table/cards/grouped layouts
- [ ] Flag detail loads from `GET /flags/:key`; rule targeting tree renders correctly for single- and multi-context rules
- [ ] Segments list and detail load from gateway
- [ ] Experiments list and detail load from gateway; both Frequentist and Bayesian viz render correctly for 2-arm and 3-arm experiments
- [ ] ⌘K command palette opens and navigates
- [ ] Light/dark theme persists across page refresh
- [ ] `VITE_API_BASE_URL` controls gateway endpoint

## Out of Scope

- Flag / segment / experiment CREATE or EDIT forms (read + toggle only in v1)
- Client-side SDKs, streaming updates (SSE/websockets)
- GitHub OAuth and Magic link auth flows (buttons visible, not wired)
- Mobile / responsive layout
- Unit or integration tests
- Deployment / CI pipeline
- ClickHouse query optimisations (tracked separately in product.md)
