# Plan — Boundary Hardening Refactor

## Phase 1: DDD Service Boundary Restoration (FR1)
<!-- execution: parallel -->

- [x] Task 1: Add gRPC RPCs to `analytics-service` for experiment results [cf91164 → integrated at merge 6f58019]
  <!-- files: proto/analytics/v1/analytics.proto, crates/stitchd-analytics-service/src/grpc/ -->
  - [x] Add `WriteExperimentResults`, `ListExperimentResults`, `GetExperimentResult` proto methods
  - [x] Write failing unit tests for handlers
  - [x] Implement handlers (uses Worker 3's canonical `ExperimentResultsRepository` trait)
  - [x] Run tests, confirm green

- [x] Task 2: Add gRPC RPCs to `experimentation-service` consumed by stats-service [3ef8b4a]
  <!-- files: proto/experiments/v1/experimentation_service.proto, crates/stitchd-experimentation-service/src/grpc/ -->
  - [x] Add `ListRunningExperiments`, `GetExperimentIteration`, `UpdateIterationLastComputed`
  - [x] Write failing tests
  - [x] Implement handlers
  - [x] Run tests (9 new tests; 46 total green)

- [x] Task 3: Create ClickHouse `experiment_results` schema in analytics-service [81ef9cd]
  <!-- files: crates/stitchd-analytics-service/clickhouse-migrations/, crates/stitchd-analytics-service/src/repo/experiment_results.rs -->
  - [x] Add migration `0001_experiment_results.sql` (MergeTree on `(env_id, experiment_id, variant_key, metric_key, iteration_id)`)
  - [x] Write integration tests for read/write (9 unit tests)
  - [x] Implement `ExperimentResultsRepository` trait + `ClickHouseExperimentResultsRepository`
  - [x] Wire into FR1.2 handlers via main.rs AppState

- [ ] Task 4: Refactor `stats-service` to consume both services via gRPC
  <!-- files: crates/stitchd-stats-service/src/scheduler.rs, crates/stitchd-stats-service/src/results_writer.rs, crates/stitchd-stats-service/src/main.rs, crates/stitchd-stats-service/Cargo.toml -->
  <!-- depends: task1, task2, task3 -->
  - [ ] Write failing mock-client tests
  - [ ] Refactor `scheduler.rs` to call `experimentation-service.ListRunningExperiments`
  - [ ] Refactor `results_writer.rs` to call `analytics-service.WriteExperimentResults`
  - [ ] Remove unused PG pool deps
  - [ ] Run tests + clippy

- [ ] Task 5: Refactor `experimentation-service.GetExperimentResults` handler to read from analytics-service
  <!-- files: crates/stitchd-experimentation-service/src/grpc/results.rs, crates/stitchd-experimentation-service/Cargo.toml -->
  <!-- depends: task1, task3 -->
  - [ ] Write failing test asserting analytics-service gRPC call
  - [ ] Implement; remove `experiment_results` PG repo usage
  - [ ] Run tests

- [ ] Task 6: Drop `experiment_results` PostgreSQL table + repo
  <!-- files: crates/stitchd-db/migrations/, crates/stitchd-db/src/experiment_results.rs, crates/stitchd-db/src/lib.rs -->
  <!-- depends: task4, task5 -->
  - [ ] Add drop migration
  - [ ] Delete repo module + trait export
  - [ ] `cargo sqlx prepare --workspace`; `cargo test --workspace`

- [x] Task 7: Verify ScyllaDB containment [2ebee8b]
  <!-- files: crates/*/Cargo.toml -->
  - [x] Audit every crate's `Cargo.toml`
  - [x] Found: only `stitchd-db` (library home) + `stitchd-segmentation-service` (binary consumer) have direct `scylla` deps. `xtask` uses transitive dep via `stitchd-db` only — fully compliant.
  - [x] Added CI guard `scripts/check_scylla_containment.sh` + `crates/stitchd-segmentation-service/SCYLLA_OWNERSHIP.md`

- [ ] Task: Conductor - User Manual Verification 'Phase 1 — DDD Service Boundaries' (Protocol in workflow.md)
  <!-- depends: task4, task5, task6, task7 -->

---

## Phase 2: Canonical URL Space + Admin Client Rerouting (FR2)
<!-- execution: sequential -->

- [ ] Task 1: Document the canonical URL space in `crates/stitchd-gateway/URL_SPACE.md`
- [ ] Task 2: Rewrite gateway route definitions (router.rs + every routes/*.rs)
- [ ] Task 3: Retire legacy SDK REST routes (delete `routes/sdk.rs`)
- [ ] Task 4: Rename `/v1/admin/*` → `/v1/superadmin/*`
- [ ] Task 5: Version health/metrics under `/v1`
- [ ] Task 6: Add `#[utoipa::path(tag = "...")]` consistently
- [ ] Task 7: Rewrite `admin/src/lib/api.ts` against new URL space
- [ ] Task 8: Regenerate OpenAPI spec + update contract baseline
- [ ] Task: Conductor - User Manual Verification 'Phase 2 — URL Canonical Rewrite' (Protocol in workflow.md)

---

## Phase 3: Naming Convention Standardization + Cleanup (FR3 + FR4)
<!-- execution: parallel -->

- [ ] Task 1: Rename crate `stitchd-events` → `stitchd-event-writer`
  <!-- files: crates/stitchd-events/, crates/stitchd-event-writer/, Cargo.toml, crates/stitchd-analytics-service/Cargo.toml -->

- [ ] Task 2: Rename Rust SDK crate `stitchd-sdk` → `stitchd-sdk-rust`
  <!-- files: sdks/rust/Cargo.toml, Cargo.toml, sdks/rust/README.md -->
  <!-- depends: task1 -->

- [ ] Task 3: Rename segmentation-service `ServiceError` → `SegmentationServiceError`
  <!-- files: crates/stitchd-segmentation-service/src/ -->

- [ ] Task 4: Apply `STITCHD_` prefix to all env vars + port suffix standardization
  <!-- files: crates/*/src/main.rs, docker-compose.yml, dev.sh, .env.local.example, .env.example, .github/workflows/, docs/src/env/ -->

- [ ] Task 5: Rename ScyllaDB keyspace `stitchd` → `stitchd_segments`
  <!-- files: crates/stitchd-db/src/scylla/mod.rs, crates/stitchd-db/scylla-migrations/, docker-compose.yml, .env.local.example -->
  <!-- depends: task4 -->

- [ ] Task 6: Standardize Rust test file naming `*_tests.rs`
  <!-- files: crates/*/src/**/*_test.rs -->

- [ ] Task 7: Standardize frontend test file naming (PascalCase to match component)
  <!-- files: admin/src/**/*.test.ts, admin/vitest.config.ts -->

- [ ] Task 8: Rename admin package `"name": "@stitchd/admin"`
  <!-- files: admin/package.json -->

- [ ] Task 9: Cleanup — delete dormant `FlagJson` struct
  <!-- files: crates/stitchd-gateway/src/routes/flags.rs -->

- [ ] Task 10: Cleanup — audit for retired event-service references
  <!-- files: crates/, docs/ -->

- [ ] Task: Conductor - User Manual Verification 'Phase 3 — Naming Standardization' (Protocol in workflow.md)
  <!-- depends: task1, task2, task3, task4, task5, task6, task7, task8, task9, task10 -->

---

## Phase 4: Admin UI Pattern Consolidation (FR5)
<!-- execution: parallel -->

- [ ] Task 1: Build `<Dropdown>` primitive
  <!-- files: admin/src/components/Dropdown.tsx, admin/src/components/Dropdown.test.tsx -->
  - [ ] Failing tests → implement → green

- [ ] Task 2: Refactor `OrgSwitcher`, `ProjectPicker`, `EnvSwitcher` to use `<Dropdown>`
  <!-- files: admin/src/shell/OrgSwitcher.tsx, admin/src/shell/ProjectPicker.tsx, admin/src/shell/EnvSwitcher.tsx -->
  <!-- depends: task1 -->

- [ ] Task 3: Build `<Modal>` primitive
  <!-- files: admin/src/components/Modal.tsx, admin/src/components/Modal.test.tsx -->

- [ ] Task 4: Refactor existing modals to use `<Modal>`
  <!-- files: admin/src/components/ConfirmDialog.tsx, admin/src/pages/flags/CreateFlagModal.tsx, admin/src/pages/segments/CreateSegmentModal.tsx, admin/src/pages/segments/EditSegmentModal.tsx, admin/src/pages/segments/DeleteSegmentModal.tsx -->
  <!-- depends: task3 -->

- [ ] Task 5: Build state components `<LoadingSpinner>`, `<ErrorBanner>`, `<EmptyState>`
  <!-- files: admin/src/components/LoadingSpinner.tsx, admin/src/components/ErrorBanner.tsx, admin/src/components/EmptyState.tsx + tests -->

- [ ] Task 6: Adopt state components in list pages
  <!-- files: admin/src/pages/flags/FlagsList.tsx, admin/src/pages/segments/SegmentsList.tsx, admin/src/pages/experiments/ExperimentsList.tsx, admin/src/pages/orgs/OrgsList.tsx -->
  <!-- depends: task5 -->

- [ ] Task 7: Build `usePaginatedList` hook
  <!-- files: admin/src/hooks/usePaginatedList.ts, admin/src/hooks/usePaginatedList.test.ts -->

- [ ] Task 8: Adopt `usePaginatedList` in list pages
  <!-- files: admin/src/pages/flags/FlagsList.tsx, admin/src/pages/segments/SegmentsList.tsx, admin/src/pages/experiments/ExperimentsList.tsx -->
  <!-- depends: task7 -->

- [ ] Task 9: Build `<PermissionGate>` wrapper
  <!-- files: admin/src/components/PermissionGate.tsx + test -->

- [ ] Task 10: Adopt `<PermissionGate>` consistently
  <!-- files: admin/src/pages/flags/FlagsList.tsx, admin/src/pages/segments/SegmentsList.tsx, etc. -->
  <!-- depends: task9 -->

- [ ] Task 11: Consolidate inline styles into CSS classes
  <!-- files: admin/src/styles/components.css, admin/src/shell/*.tsx -->

- [ ] Task 12: Consolidate domain types into `admin/src/lib/types.ts`
  <!-- files: admin/src/lib/types.ts, admin/src/pages/segments/types.ts, admin/src/pages/experiments/ExperimentsList.tsx -->

- [ ] Task 13: Build `extractErrorMessage(err)` helper
  <!-- files: admin/src/lib/errors.ts + test, admin/src/**/*.tsx (adoption) -->

- [ ] Task: Conductor - User Manual Verification 'Phase 4 — Admin UI Primitives' (Protocol in workflow.md)
  <!-- depends: task2, task4, task6, task8, task10, task11, task12, task13 -->

---

## Phase 5: Formik + Yup Form Migration (FR6)
<!-- execution: parallel -->

- [ ] Task 1: Add Formik + Yup dependencies
  <!-- files: admin/package.json, admin/package-lock.json -->

- [ ] Task 2: Build form primitives at `admin/src/components/form/`
  <!-- files: admin/src/components/form/FormField.tsx, FormSelect.tsx, FormCheckbox.tsx, FormTextarea.tsx, FormSubmit.tsx, FormErrorBanner.tsx + tests -->
  <!-- depends: task1 -->

- [ ] Task 3: Create validation schemas directory
  <!-- files: admin/src/lib/validation/flagSchema.ts, segmentSchema.ts, experimentSchema.ts, eventDefinitionSchema.ts, sdkKeySchema.ts, authSchema.ts, orgSchema.ts, authProviderSchema.ts -->
  <!-- depends: task1 -->

- [ ] Task 4: Migrate Auth forms (Login, MFA, password reset, switch-org)
  <!-- files: admin/src/pages/auth/ -->
  <!-- depends: task2, task3 -->

- [ ] Task 5: Migrate Flag forms (CreateFlagModal, variant editor, rule builder, edit) + async key uniqueness validation
  <!-- files: admin/src/pages/flags/ -->
  <!-- depends: task2, task3 -->

- [ ] Task 6: Migrate Segment forms (Create, Edit, entry import, condition tree editor)
  <!-- files: admin/src/pages/segments/ -->
  <!-- depends: task2, task3 -->

- [ ] Task 7: Migrate Experiment forms
  <!-- files: admin/src/pages/experiments/ -->
  <!-- depends: task2, task3 -->

- [ ] Task 8: Migrate Event Definition forms
  <!-- files: admin/src/pages/events/ -->
  <!-- depends: task2, task3 -->

- [ ] Task 9: Migrate Environment forms
  <!-- files: admin/src/pages/environments/ -->
  <!-- depends: task2, task3 -->

- [ ] Task 10: Migrate SDK Key form
  <!-- files: admin/src/pages/sdk-keys/ -->
  <!-- depends: task2, task3 -->

- [ ] Task 11: Migrate Org / User / Auth Provider forms
  <!-- files: admin/src/pages/orgs/, admin/src/pages/superadmin/ -->
  <!-- depends: task2, task3 -->

- [ ] Task 12: Write `admin/src/components/form/README.md`
  <!-- files: admin/src/components/form/README.md -->
  <!-- depends: task2 -->

- [ ] Task 13: Run full admin test+lint+typecheck+build; inspect bundle-size delta
  <!-- depends: task4, task5, task6, task7, task8, task9, task10, task11 -->

- [ ] Task: Conductor - User Manual Verification 'Phase 5 — Formik Migration' (Protocol in workflow.md)
  <!-- depends: task13 -->

---

## Phase 6: SDK Spec ↔ Implementation Alignment (FR7)
<!-- execution: parallel -->
<!-- depends: -->

- [x] Task 1: Document archived flag lifecycle in `sdks/spec/docs/01-overview.md` [16fa0a8]
  <!-- files: sdks/spec/docs/01-overview.md -->

- [x] Task 2: Document LRU recency note in `sdks/spec/docs/03-caching.md` [d3ec5de]
  <!-- files: sdks/spec/docs/03-caching.md -->

- [x] Task 3: Add Crate Naming Convention at `sdks/spec/00-naming.md` [0fd638d]
  <!-- files: sdks/spec/00-naming.md -->

- [x] Task 4: Refresh `sdks/rust/README.md` + crate-level doc comment [8ada1cf]
  <!-- files: sdks/rust/README.md, sdks/rust/src/lib.rs -->

- [ ] Task 5: Run SDK conformance suite
  <!-- files: sdks/rust/tests/conformance.rs (read-only) -->
  <!-- depends: task4 -->

- [ ] Task: Conductor - User Manual Verification 'Phase 6 — SDK Alignment' (Protocol in workflow.md)
  <!-- depends: task1, task2, task3, task5 -->

---

## Phase 7: Final Documentation + End-to-End Smoke
<!-- execution: sequential -->
<!-- depends: phase1, phase2, phase3, phase4, phase5, phase6 -->

- [ ] Task 1: Update `conductor/tech-stack.md` (crates, env vars, URL space, Formik, Scylla keyspace)
- [ ] Task 2: Update `conductor/product.md` (Implementation Status row)
- [ ] Task 3: Update `docs/` mdBook chapters (URL reference, env-var reference, architecture)
- [ ] Task 4: Run end-to-end smoke (`./dev.sh start` → admin UI flow → SDK eval → ClickHouse)
- [ ] Task 5: Run full workspace tests (cargo clippy/test, admin lint+typecheck+test+build)
- [ ] Task: Conductor - User Manual Verification 'Phase 7 — End-to-End Verification' (Protocol in workflow.md)
