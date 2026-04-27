# Plan: Stitchd Admin UI

## Phase 1: Project Scaffold & Design System

- [x] Task: Scaffold Vite + React 18 + TypeScript project in `admin/` (29cfc8d)
  - Sub: `npm create vite@latest admin -- --template react-ts`
  - Sub: Install deps: `react-router-dom`, `axios`
  - Sub: Configure `vite.config.ts` with dev proxy `/api → http://localhost:8080`
  - Sub: Set up `.env.example` with `VITE_API_BASE_URL`
  - Sub: Copy font imports (Archivo, Inter, JetBrains Mono) from prototype

- [x] Task: Implement CSS design system from `styles.css` prototype (29cfc8d)
  - Sub: Port all CSS custom properties (colors, spacing, typography, shadows) to `src/styles/tokens.css`
  - Sub: Port component classes (card, table, badge, btn, input, toggle, mono-key, type-pill, variant-bar, sparkline, sidebar, topbar) to `src/styles/`
  - Sub: Implement `data-theme="light|dark"` switching on `<html>`
  - Sub: Implement `data-density="comfortable|compact"` body attribute
  - Sub: Verify tokens match the prototype's light and dark palettes exactly

- [x] Task: Port shared UI primitives to TypeScript components (29cfc8d)
  - Sub: `Icon` + `I` icon map → `src/components/icons.tsx`
  - Sub: `StitchdMark` brand component
  - Sub: `Sparkline` SVG component
  - Sub: `VariantBar` component
  - Sub: `PageHeader` component (breadcrumbs, title, subtitle, actions slot)

- [x] Task: Conductor - User Manual Verification 'Project Scaffold & Design System' (Protocol in workflow.md)

## Phase 2: App Shell & Navigation

- [x] Task: Implement Tweaks system (1f86aca)
  - Sub: `useTweaks` hook with `localStorage` persistence
  - Sub: `TweaksPanel` component (theme, nav style, flags layout, flag detail layout, exp viz, density, accent)
  - Sub: Wire `data-theme` and `--accent` CSS var to tweaks state

- [x] Task: Implement Sidebar navigation component (1f86aca)
  - Sub: Brand mark + org switcher + env pill
  - Sub: Search input (focuses ⌘K)
  - Sub: Project nav items (Dashboard, Flags, Segments, Experiments, Events)
  - Sub: Admin nav items (Environments, Members, Audit, Super Admin)
  - Sub: User footer (avatar, name, email, notifications)

- [x] Task: Implement Rail and Top bar nav variants (1f86aca)
  - Sub: Rail: icon-only sidebar, tooltip labels on hover
  - Sub: Topbar: full horizontal nav bar
  - Sub: `data-nav="sidebar|rail|topbar"` on `.app-shell` drives layout via CSS

- [x] Task: Implement ⌘K Command Palette (1f86aca)
  - Sub: `CommandPalette` component (overlay + input + grouped results)
  - Sub: Keyboard shortcut listener (⌘K / Ctrl+K, Escape)
  - Sub: Navigation items + flag quick-jump

- [x] Task: Set up React Router with all 13 routes (1f86aca)
  - Sub: `/login`, `/`, `/flags`, `/flags/:key`
  - Sub: `/segments`, `/segments/:key`
  - Sub: `/experiments`, `/experiments/:key`
  - Sub: `/events`, `/environments`, `/members`, `/audit`, `/super-admin`
  - Sub: Protected route wrapper (redirects to `/login` if no JWT)

- [x] Task: Conductor - User Manual Verification 'App Shell & Navigation' (Protocol in workflow.md)

## Phase 3: Authentication

- [x] Task: Implement API client with auth (071fcbf)
  - Sub: `src/lib/api.ts` — axios instance with `VITE_API_BASE_URL` base URL
  - Sub: Request interceptor: inject `Authorization: Bearer <token>` from `localStorage`
  - Sub: Response interceptor: on 401, clear token and redirect to `/login`

- [x] Task: Implement Login screen (071fcbf)
  - Sub: Port `LoginScreen` from prototype to TypeScript
  - Sub: Left panel: brand + tagline + docker compose code block
  - Sub: Right panel: card with Password / OIDC / SAML tab switcher

- [x] Task: Wire Password login to gateway (071fcbf)
  - Sub: `POST /auth/login` with email + password
  - Sub: Store JWT in `localStorage` on success
  - Sub: Navigate to `/` on success, show error on failure

- [x] Task: Wire OIDC login flow (071fcbf)
  - Sub: `GET /auth/oidc/authorize` → redirect to IdP
  - Sub: `/auth/callback` route: exchange code for JWT, store, redirect to `/`

- [x] Task: Implement SAML initiation (redirect only; callback handled by IdP) (071fcbf)

- [x] Task: Conductor - User Manual Verification 'Authentication' (Protocol in workflow.md)

## Phase 4: Flags Screens
<!-- depends: -->

- [ ] Task: Implement Flags list screen (`/flags`)
  <!-- files: src/pages/flags/FlagsList.tsx, src/pages/flags/FlagCard.tsx, src/pages/flags/FlagsGrouped.tsx -->
  - Sub: Port table, cards, and grouped-by-segment layouts from prototype
  - Sub: Layout switcher (table/cards/grouped icons)
  - Sub: Search/filter input
  - Sub: On/off toggle per flag (optimistic UI)
  - Sub: `GET /flags?env=production` — wire to API
  - Sub: Loading + empty states

- [ ] Task: Implement Flag Detail screen (`/flags/:key`)
  <!-- files: src/pages/flags/FlagDetail.tsx, src/pages/flags/tabs/ -->
  - Sub: Port tab structure: Targeting, Variants, Evaluations, Experiments, SDK Snippet, History
  - Sub: `GET /flags/:key` — wire to API
  - Sub: Stacked / Side-by-side layout from Tweaks
  - Sub: Staged changes banner

- [ ] Task: Implement Rule Targeting tree
  <!-- files: src/components/targeting/RuleTree.tsx, src/components/targeting/RuleHeader.tsx -->
  - Sub: Recursive `RuleTree` component (AND/OR/NOT groups + clause leaves + segment refs)
  - Sub: Context type header pills (single- and multi-context with MULTI-CONTEXT badge)
  - Sub: Clause `ctx:` qualifier tag for multi-context rules
  - Sub: Color-coded group borders (blue AND / purple OR / red NOT)

- [ ] Task: Implement Variants tab + Evaluations sparkline tab
  <!-- files: src/pages/flags/tabs/VariantsTab.tsx, src/pages/flags/tabs/EvaluationsTab.tsx -->

- [ ] Task: Conductor - User Manual Verification 'Flags Screens' (Protocol in workflow.md)

## Phase 5: Segments & Experiments Screens
<!-- depends: -->

- [ ] Task: Implement Segments list screen (`/segments`)
  <!-- files: src/pages/segments/SegmentsList.tsx -->
  - Sub: Port table from prototype
  - Sub: `GET /segments` — wire to API

- [ ] Task: Implement Segment Detail screen (`/segments/:key`)
  <!-- files: src/pages/segments/SegmentDetail.tsx -->
  - Sub: Rule-based view: condition tree (read-only)
  - Sub: List-based view: include/exclude key lists
  - Sub: `GET /segments/:key` — wire to API

- [ ] Task: Implement Experiments list screen (`/experiments`)
  <!-- files: src/pages/experiments/ExperimentsList.tsx -->
  - Sub: Port table with state filter tabs (running / draft / stopped / completed)
  - Sub: `GET /experiments` — wire to API

- [ ] Task: Implement Experiment Detail screen (`/experiments/:key`)
  <!-- files: src/pages/experiments/ExperimentDetail.tsx, src/components/experiments/FrequentistViz.tsx, src/components/experiments/BayesianViz.tsx, src/components/experiments/PairwiseMatrix.tsx -->
  - Sub: `GET /experiments/:key` + `GET /experiments/:key/results` — wire to API
  - Sub: Frequentist viz: CI band chart, p-value, power per arm
  - Sub: Bayesian viz: posterior curves, P(best), expected loss per arm
  - Sub: Multi-variant (3+ arms): pairwise matrix + joint posterior bars
  - Sub: Auto viz selection (experiment model field); Tweaks override
  - Sub: "Ready to ship" banner at ≥95% confidence

- [ ] Task: Conductor - User Manual Verification 'Segments & Experiments Screens' (Protocol in workflow.md)

## Phase 6: Secondary Screens & Final Polish
<!-- depends: phase4, phase5 -->

- [ ] Task: Implement Dashboard screen (`/`) — mock data
  <!-- files: src/pages/Dashboard.tsx -->
  - Sub: Stat cards with sparklines (evaluations, active flags, experiments, p95 latency)
  - Sub: Recent flags table + experiments sidebar + service health list

- [ ] Task: Implement secondary screens — mock data
  <!-- files: src/pages/EventsRegistry.tsx, src/pages/Environments.tsx, src/pages/Members.tsx, src/pages/AuditLog.tsx, src/pages/SuperAdmin.tsx -->
  - Sub: Events Registry (`/events`)
  - Sub: Environments & SDK Keys (`/environments`)
  - Sub: Members & Roles (`/members`)
  - Sub: Audit Log (`/audit`)
  - Sub: Super Admin (`/super-admin`)

- [ ] Task: Final integration pass
  <!-- files: src/lib/api.ts, src/App.tsx -->
  - Sub: Verify all API-wired screens handle loading, error, and empty states
  - Sub: Confirm JWT flow end-to-end (login → protected route → 401 refresh)
  - Sub: `vite build` produces clean `dist/` with no TypeScript errors
  - Sub: ESLint + Prettier pass with zero errors

- [ ] Task: Conductor - User Manual Verification 'Secondary Screens & Final Polish' (Protocol in workflow.md)
