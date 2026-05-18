# Spec — Boundary Hardening Refactor

## Overview

Pre-launch architectural refactor that eliminates all boundary violations and pattern drift identified during the audit, across six dimensions: DDD service boundaries, REST URL conventions, naming consistency, cleanup, admin UI pattern consolidation (including form-state migration to Formik), and SDK spec ↔ impl alignment. Because the system is not yet live, every change ships in a single track with no backwards-compatibility shims, deprecation periods, or migration tooling.

## Functional Requirements

### FR1 — DDD Service Boundary Restoration (P0)
- **FR1.1** Move `experiment_results` table from PostgreSQL to ClickHouse. Owned by `stitchd-analytics-service` (schema + migrations + repository).
- **FR1.2** Add gRPC RPCs on `analytics-service`: `WriteExperimentResults`, `ListExperimentResults`, `GetExperimentResult` (in `proto/analytics/v1/`).
- **FR1.3** Add gRPC RPCs on `experimentation-service` that `stats-service` needs: `ListRunningExperiments`, `GetExperimentIteration`, `UpdateIterationLastComputed` (in `proto/experiments/v1/`).
- **FR1.4** Refactor `stitchd-stats-service`:
  - `results_writer.rs` → calls `analytics-service.WriteExperimentResults` via gRPC.
  - `scheduler.rs` → calls `experimentation-service.ListRunningExperiments` via gRPC.
  - Remove `sqlx::PgPool` from stats-service except for stats-owned tables (`stats_schedule`, `stats_jobs`).
- **FR1.5** `experimentation-service.GetExperimentResults` handler now reads from `analytics-service` via gRPC.
- **FR1.6** Drop `experiment_results` PG table (migration + repo + `crates/stitchd-db/src/experiment_results.rs`).
- **FR1.7** ScyllaDB code stays in `crates/stitchd-db/src/scylla/`; verify no production binary other than segmentation-service links scylla.

### FR2 — Canonical URL Space + Admin Client Rerouting (P1)
- **FR2.1** New URL scheme — all resource paths nest under org → project → env:
  - `/v1/orgs/{org_id}/projects/{project_id}/environments/{environment_id}/{resource}` for flags / segments / experiments / event-definitions / sdk-keys
- **FR2.2** Path-param names standardized to `snake_case` full-form: `{org_id}`, `{project_id}`, `{environment_id}`, `{flag_id}`, `{segment_id}`, `{experiment_id}`, `{event_definition_id}`, `{sdk_key_id}`, `{user_id}`, `{auth_provider_id}`, `{job_id}`. No `{id}`, no `{def_id}`, no `{env_id}`.
- **FR2.3** Retire legacy SDK routes: `/v1/environments/{env_id}/evaluate`, `/events`, `/events/batch`, `/segments/list-check`, `/segments/list-check/batch`.
- **FR2.4** Unify segments under environment path.
- **FR2.5** Superadmin routes: `/v1/admin/*` → `/v1/superadmin/*`.
- **FR2.6** Versioned health/metrics: `/v1/health`, `/v1/metrics`.
- **FR2.7** Add `#[utoipa::path(tag = "...")]` consistently — one tag per route module.
- **FR2.8** Rewrite `admin/src/lib/api.ts` to call the new URL space. Every `list*`/`get*`/`create*`/`update*`/`delete*` function updated.
- **FR2.9** Rust SDK URLs unchanged — already aligned.

### FR3 — Naming Convention Standardization (P2)
- **FR3.1** Rename `stitchd-events` → `stitchd-event-writer` (library role).
- **FR3.2** Rename Rust SDK crate `stitchd-sdk` → `stitchd-sdk-rust`.
- **FR3.3** Rename segmentation-service `ServiceError` → `SegmentationServiceError`.
- **FR3.4** Apply `STITCHD_` prefix to **every** env var:
  - PostgreSQL: `DATABASE_URL` → `STITCHD_DATABASE_URL`
  - ClickHouse: `CLICKHOUSE_URL/USER/PASSWORD/DB` → `STITCHD_CLICKHOUSE_*`
  - ScyllaDB: `SCYLLA_URI/KEYSPACE/CQL_PORT` → `STITCHD_SCYLLA_*`
  - Segmentation sweeper: `SWEEPER_*` → `STITCHD_SEGMENTATION_SWEEPER_*`
  - Service ports: `STITCHD_{SERVICE}_GRPC_PORT`, `STITCHD_{SERVICE}_METRICS_PORT`, `STITCHD_GATEWAY_HTTP_PORT`
  - All auth/email/SMTP/OIDC/SAML config: `STITCHD_` prefix
  - `RUST_LOG`: untouched
- **FR3.5** ScyllaDB keyspace `stitchd` → `stitchd_segments`.
- **FR3.6** Standardize port suffixes — every service uses `_GRPC_PORT` + `_METRICS_PORT`; gateway adds `_HTTP_PORT`.
- **FR3.7** Rust test file naming: standardize to `*_tests.rs` inside `src/`; `tests/` for integration tests only.
- **FR3.8** Frontend test files match component case (PascalCase).
- **FR3.9** Admin UI `package.json` `"name": "admin"` → `"name": "@stitchd/admin"`.
- **FR3.10** Update `docker-compose.yml`, `dev.sh`, `.env.local.example`, `.env.example`, every service's `main.rs`/config loader, mdBook env-var doc pages.

### FR4 — Cleanup (P3)
- **FR4.1** Delete dormant `FlagJson` struct from `crates/stitchd-gateway/src/routes/flags.rs`.
- **FR4.2** Audit gateway routes for any leftover references to retired event-service.
- **FR4.3** Update `conductor/tech-stack.md`, `conductor/product.md`, `docs/` mdBook chapters for new URL space, crate names, env vars, Scylla keyspace, and Formik adoption.
- **FR4.4** Regenerate OpenAPI spec; update `scripts/check_openapi_contract.py` baseline.

### FR5 — Admin UI Pattern Consolidation
- **FR5.1** Extract shared `<Dropdown>` primitive. Refactor `OrgSwitcher`, `ProjectPicker`, `EnvSwitcher`. Single source for trigger button, dropdown panel, click-outside, Escape key, focus management.
- **FR5.2** Extract shared `<Modal>` primitive. Refactor `ConfirmDialog`, `CreateFlagModal`, `CreateSegmentModal`, `EditSegmentModal`, `DeleteSegmentModal` and any other modals.
- **FR5.3** Extract reusable state components: `<LoadingSpinner>`, `<ErrorBanner message icon>`, `<EmptyState icon title desc action>`.
- **FR5.4** Extract `usePaginatedList(endpoint, deps)` hook. Adopt in all list pages.
- **FR5.5** Extract `<PermissionGate permission fallback>` wrapper.
- **FR5.6** Consolidate inline button-reset / dropdown styles into `.button-reset`, `.dropdown`, `.dropdown-item` CSS classes.
- **FR5.7** Move per-page domain types into `admin/src/lib/types.ts`.
- **FR5.8** Standardize API client error mapping — single `extractErrorMessage(err)` helper.

### FR6 — Admin UI Form Migration to Formik
- **FR6.1** Add Formik (`formik` ^2.x) and a validation library (Yup ^1.x — Formik's de facto pairing) to `admin/package.json`. Run `npm install`.
- **FR6.2** Establish shared form primitives at `admin/src/components/form/`:
  - `<FormField name label hint>` — wraps Formik `<Field>`, renders label + input + error + hint with a single consistent layout.
  - `<FormSelect name label options>` — for `<select>` inputs.
  - `<FormCheckbox name label>` — for booleans.
  - `<FormTextarea name label>` — for multiline.
  - `<FormSubmit label loadingLabel>` — submit button that reads `isSubmitting` from Formik context.
  - `<FormErrorBanner>` — surfaces top-level submit errors (network/server).
- **FR6.3** Define a shared validation-schema directory: `admin/src/lib/validation/` (`flagSchema.ts`, `segmentSchema.ts`, `experimentSchema.ts`, `eventDefinitionSchema.ts`, `sdkKeySchema.ts`, etc.). All validation goes through Yup schemas; remove inline `validateVariantValue()` and similar functions.
- **FR6.4** Migrate every existing form to Formik. Inventory (verify against actual codebase during plan execution):
  - **Auth:** Login form, MFA challenge, password reset, switch-org.
  - **Flags:** `CreateFlagModal` (name/key/type/variants), variant editor, rule builder form, `flag/edit` description.
  - **Segments:** `CreateSegmentModal`, `EditSegmentModal`, segment-entry import form, rule-based-segment condition tree editor.
  - **Experiments:** Experiment create/edit form (metrics, variants, duration).
  - **Event definitions:** Create/edit forms.
  - **Environments:** Create/rename, env-key creation.
  - **SDK keys:** Create/revoke confirmation (revoke has just a confirm; not a Formik target).
  - **Org / Users / Auth providers:** Org create, user invite, OIDC/SAML provider configuration forms.
- **FR6.5** Replace ad-hoc form state (`[name, setName]`, `[error, setError]`, `[saving, setSaving]`) with Formik's `<Formik initialValues onSubmit validationSchema>` wrapper. Field-level errors via `<ErrorMessage>` rendered inside `<FormField>`.
- **FR6.6** Submit-error pattern: `onSubmit` calls API; on failure use `setStatus({ error: extractErrorMessage(err) })` + render via `<FormErrorBanner>` reading `status.error`. `setSubmitting(false)` always called in `finally`.
- **FR6.7** Async validation hook-in: for fields like "flag key must be unique within project", use Formik's `validateField` + debounced async validation against the API. Provide one canonical example in `flagSchema.ts` + `CreateFlagModal`.
- **FR6.8** Update existing tests (`admin/src/**/*.test.ts`) for migrated forms — Formik form testing uses different patterns (`waitFor` + `userEvent.type` over Formik field name). Provide a `tests/setup.ts` helper if useful.
- **FR6.9** Document Formik adoption in a new `admin/src/components/form/README.md` covering: how to build a form, validation patterns, submit-error pattern, async validation example, testing patterns.

### FR7 — SDK Spec ↔ Implementation Alignment
- **FR7.1** SDK URLs require no changes — already aligned to `/v1/sdk/*` + gRPC.
- **FR7.2** Crate rename `stitchd-sdk` → `stitchd-sdk-rust` (covered by FR3.2).
- **FR7.3** SDK spec gap: document `archived` flag status in `sdks/spec/01-overview.md`. Rust impl treats `archived == true` as `FlagNotFound`; spec must define this contract.
- **FR7.4** SDK spec gap: document LRU recency-promotion trade-off in `sdks/spec/03-caching.md`.
- **FR7.5** SDK spec gap: add "Crate naming convention" noting `stitchd-sdk-{lang}` pattern.
- **FR7.6** Refresh `sdks/rust/README.md` + crate-level doc comment.
- **FR7.7** Run SDK conformance suite (`cargo test -p stitchd-sdk-rust`) post-rename; expect zero regressions.

## Non-Functional Requirements
- **NFR1** `cargo clippy --workspace --all-targets -- -D warnings` clean.
- **NFR2** `cargo test --workspace` passes; per-crate coverage ≥90%.
- **NFR3** `npm run lint` + `node_modules/.bin/tsc --noEmit -p tsconfig.app.json` + `npm test` all pass in `admin/`.
- **NFR4** `cargo run -p xtask -- docs` produces an up-to-date mdBook site.
- **NFR5** Local stack via `./dev.sh start` boots end-to-end with new env vars + Scylla keyspace.
- **NFR6** Admin UI: no regressions in any existing page (every list view + every Formik-migrated form tested manually post-refactor).
- **NFR7** Bundle size: Formik + Yup add ~30 KB gzipped — acceptable for the consistency gains. Verify with `npm run build` and inspect `dist/` size delta.

## Acceptance Criteria
- [ ] `grep -rE 'experiments|experiment_iterations|experiment_results' crates/stitchd-stats-service/src/ | grep -iE 'sqlx|query!|query_as'` returns zero hits.
- [ ] No HTTP route in gateway matches `/v1/environments/{env_id}/(evaluate|events|segments/list-check)`.
- [ ] No flat segments routes; all under `/v1/orgs/{org_id}/projects/{project_id}/environments/{environment_id}/segments`.
- [ ] `grep -rE '\{id\}|\{def_id\}|\{env_id\}' crates/stitchd-gateway/src/routes/ proto/ admin/src/` returns zero in route definitions.
- [ ] Every entry in `.env.local.example`/`.env.example` starts with `STITCHD_` (except `RUST_LOG`).
- [ ] `crates/stitchd-events` does not exist; `crates/stitchd-event-writer` does.
- [ ] `sdks/rust/Cargo.toml` `name = "stitchd-sdk-rust"`; SDK conformance suite passes.
- [ ] ScyllaDB keyspace defaults to `stitchd_segments`.
- [ ] Only `stitchd-segmentation-service` and `xtask` have `scylla` in their `Cargo.toml`.
- [ ] `admin/src/components/` contains `Modal.tsx`, `Dropdown.tsx`, `EmptyState.tsx`, `ErrorBanner.tsx`, `LoadingSpinner.tsx`, `PermissionGate.tsx`.
- [ ] `admin/src/hooks/usePaginatedList.ts` exists and is used by ≥3 list pages.
- [ ] grep for `position: 'fixed'` in `admin/src/pages/` returns zero hits.
- [ ] `admin/package.json` lists `formik` and `yup` as runtime dependencies.
- [ ] `admin/src/components/form/` exists with at minimum `FormField`, `FormSelect`, `FormCheckbox`, `FormTextarea`, `FormSubmit`, `FormErrorBanner`, `README.md`.
- [ ] `admin/src/lib/validation/` contains one Yup schema file per migrated form domain.
- [ ] grep for `useState<string>('')` immediately followed within ~5 lines by an `<input>` and an `onSubmit` in `admin/src/pages/` returns zero hits (all forms use Formik).
- [ ] grep for `formik` or `useFormik` or `<Formik` in `admin/src/pages/` returns hits in ≥10 form-containing files.
- [ ] `sdks/spec/` documents archived flag lifecycle + LRU recency note + crate naming convention.
- [ ] End-to-end smoke test: admin UI loads → create flag → create segment → eval via SDK → results in ClickHouse.
- [ ] `cargo test --workspace` + clippy clean + admin lint+typecheck+tests pass.

## Out of Scope
- Adding new product features beyond what the move requires.
- Production data migration (project not live).
- Performance tuning, schema indexes beyond what FR1 requires.
- Reorganizing services beyond the named renames.
- New SDK languages (only Rust SDK is updated; spec doc may mention future languages).
- RBAC or auth-provider behavioural changes.
- Replacing axios with another HTTP client.
- OpenAPI codegen for admin TypeScript types (continue manual `lib/types.ts`).
- Storybook setup, visual regression testing, accessibility audit (separate tracks).
- Adopting react-hook-form or other form libraries beyond Formik+Yup (decision locked).
