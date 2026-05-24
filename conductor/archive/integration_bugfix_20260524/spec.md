# Spec: Integration Bug Hunt & Fix — Full Stack Functional Coverage

## Overview

A comprehensive integration test pass against the live Stitchd stack to surface and fix bugs across all functional areas. Infrastructure databases run in Docker (Postgres, ClickHouse, ScyllaDB); every application service (six gRPC microservices + REST gateway) and the Admin UI dev server are started in-process. All known functional use cases are exercised manually and programmatically. Discovered bugs are triaged and documented in Phase 1; fixed in Phase 2.

## Functional Requirements

### Phase 1 — Bug Discovery

**Stack Setup**
- Only Docker services: `postgres`, `clickhouse`, `scylladb` (via `docker compose up`)
- All six gRPC services + gateway built with `cargo build --workspace` and launched as local processes (not containers)
- Admin UI started with `npm run dev` in `admin/`
- Migrations run clean against a fresh database before the test pass begins

**Scope — All four priority areas covered:**

**Area 1: Auth + Org Management**
- Superadmin login, org creation, superadmin routes (`/superadmin/*`)
- Org user login (password), password reset, user invite flow
- OIDC/SAML provider setup and login, MFA enrollment + verification
- RBAC: role assignment, permission checks across UI screens
- SDK key create/revoke, rotation (min-1-active rule), key scoping per environment
- Token expiry, refresh, logout
- Environment CRUD

**Area 2: Flags + Segments (CRUD + Rule Builder + Preview)**
- Flag CRUD for all five types (int, double, bool, string, json)
- Variant management: add/edit/delete variants, type enforcement
- Rule builder: AND/OR/NOT condition trees, segment picker, "Is in Segment" rule, "Flag evaluated with variant X" rule
- Hash-input selector list: cross-context selectors, drag/keyboard reorder, live worked-example banner, context-type + parameter autocomplete sourced from the registry
- Percentage rollout: allocation math, default-rule distribution
- CRITICAL: cross-context key+param combination testing — mix selectors from 2+ context types in one rule (e.g. `user.key + user.params.tier + device.params.os`); verify hash bucket is stable and matches evaluate-preview output for the same context bundle
- Evaluate-preview test panel: rule trace output, rollout debug info, OR/AND missing-context resolution
- Segment CRUD: rule-based condition builder, list-based key management (include/exclude), ScyllaDB-backed persistence
- Flag archive/restore; whole-flag lock behaviour when experiment is running
- Pagination: flag list, segment list — URL-driven `?page=N`

**Area 3: Events + Metrics + Experiments**
- Event registration (all metric_types), JSON-schema validation (valid + invalid), archive (410)
- EventDetail: recent firings sparkline, TestEventWidget, back-link to metrics
- Metric CRUD: aggregation (all aggregators, JsonLogic where_clause, on_field), ratio (numerator/denominator/min_denominator), funnel (steps + window_seconds)
- Metric preview sparkline (ClickHouse-backed, 7-day default)
- Experiment creation: flag_rule_id binding + targets_default_rule binding, XOR enforcement, unit context types, guardrail metrics, pre_period_days
- Experiment lifecycle: start → pause → resume → stop; whole-flag lock enforcement at each transition
- Experiment results: Frequentist t-test + two-proportion Z, Bayesian posteriors + PtBC, CUPED, SRM chi-square, guardrails
- Per-context-type tab strip; on-demand recompute

**Area 4: SDK End-to-End + UI/UX Polish**
- Rust SDK: `SdkClient::init` blocks until first definition sync; `evaluate()` for all five flag types
- SDK evaluation parity with Admin UI evaluate-preview for same flag + context
- Cross-context key+param combinations: user + device contexts with mixed hash selectors — verify bucket assignment is identical across SDK and preview
- List-segment LFU cache: pre-warm, batch refresh, membership accuracy
- SDK key auth: scoped to project + environment, rejection on wrong env
- UI/UX: visual consistency, empty-state messaging, loading skeletons, error toasts/messages, pagination controls, form validation feedback, table column alignment, button/link affordance clarity, destructive action confirmations, breadcrumb/navigation consistency, mobile responsiveness basics

### Phase 2 — Bug Fixes

- Each bug documented in `bugs.md` is fixed in order of severity (critical first)
- Fix includes: code change, targeted unit/integration test, and a smoke-check verification against the running stack
- UI fixes follow the existing Formik + Yup form layer and component patterns in `admin/src/components/form/` and `admin/src/lib/validation/`

## Non-Functional Requirements

- Backend services must start without errors or panics; any startup failure is itself a bug to fix before continuing the test pass
- All fixes must pass existing CI gates: `cargo clippy -D warnings`, `cargo fmt`, `cargo test --workspace`, `node_modules/.bin/tsc --noEmit`, `npm run lint`
- No new Docker application containers introduced — databases-only Docker constraint is permanent for this track

## Acceptance Criteria

- All backend services and Admin UI start cleanly from the in-process setup
- All functional use cases in the four priority areas have been exercised
- Every discovered bug is documented in `bugs.md` with: reproduction steps, expected vs. actual behaviour, severity (critical/high/medium/low), and likely responsible component
- All documented bugs are fixed (or explicitly deferred with justification)
- CI gates pass after fixes are applied

## Out of Scope

- New features or behaviour changes beyond fixing observed bugs
- Client-side (browser/mobile) SDK
- Infrastructure changes beyond the Docker databases-only constraint
- Warehouse-backed event ingestion, multi-armed bandit, sequential testing
