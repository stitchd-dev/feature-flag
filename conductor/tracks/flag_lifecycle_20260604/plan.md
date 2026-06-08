# Implementation Plan: Flag Lifecycle Automation

Spec: scheduled changes (flags/segments/experiments, one-shot + recurring/DST),
flag prerequisites (eval-time fallback gate), cross-entity dependency graph +
referential integrity (delete-blocking), Rust SDK support, full Admin UI.

**Architecture decision:** scheduling logic lives in a NEW `stitchd-schedule-service`
(gRPC-only scheduled consumer, mirroring `stitchd-stats-service`) that dispatches
due changes to each owning service's existing mutation/lifecycle RPC. Documented in
`tech-stack.md` (Phase 1) before implementation, per the workflow's tech-stack rule.

**Parallel wave map** (see annotations):
- Wave A → Phase 1
- Wave B → Phase 2 ∥ Phase 3
- Wave C → Phase 4 ∥ Phase 5
- Wave D → Phase 6 ∥ Phase 7
- Wave E → Phase 8
- Wave F → Phase 9

---

## Phase 1: Foundation — Deps, Schema, Proto, Core Types
<!-- execution: parallel -->
<!-- depends: -->

- [x] Task 1: Add `chrono-tz` + `rrule` to workspace `Cargo.toml`; document both deps
  and the new `stitchd-schedule-service` decision in `tech-stack.md`. TDD: tz/RRULE
  next-occurrence sanity test. (next-occurrence test lives in core task 4, where the deps are wired)
  <!-- files: Cargo.toml, conductor/tech-stack.md -->
- [x] Task 2: PG migration `20260604000001_lifecycle_automation.sql` — `scheduled_changes`
  (entity_type, entity_id, env_id, mutation_payload JSONB, schedule_kind, scheduled_at,
  rrule, tz, next_run_at, last_run_at, status), `scheduled_change_runs` (history),
  `flag_prerequisites` (+ `fallback_variant_id` on feature_flags), `entity_dependencies`
  edge table; partial index on `next_run_at WHERE status='active'`. TDD: repo smoke test.
  <!-- files: crates/stitchd-db/migrations/20260604000001_lifecycle_automation.sql -->
- [x] Task 3: Proto additions (backward-compatible) — `flag_sync.proto` (`FlagPrerequisite`
  + `repeated prerequisites` + `fallback_variant_key` on `FeatureFlag`); `flag_service.proto`
  (`SetPrerequisites`/`GetPrerequisites`); new `proto/schedule/v1/*.proto`; experiment +
  segment lifecycle/schedule messages; `DEPENDENCY_EXISTS` error code. Regenerate stubs.
  <!-- files: proto/flags/v1/flag_sync.proto, proto/flags/v1/flag_service.proto, proto/schedule/v1/schedule_service.proto, proto/experiments/v1/experiment_service.proto, proto/segments/v1/segment_service.proto -->
- [x] Task 4: Core domain types in `stitchd-core` — `PrerequisiteGate`/`FlagPrerequisite`,
  `ScheduledChange`/`RecurrenceSpec` (RRULE+tz next-occurrence, DST-aware),
  `EntityRef`/`DependencyEdge`. TDD: (de)serialization + recurrence-across-DST tests.
  <!-- files: crates/stitchd-core/src/schedule.rs, crates/stitchd-core/src/prerequisite.rs, crates/stitchd-core/src/lib.rs -->
  <!-- depends: task1 -->
- [ ] Task: Conductor - User Manual Verification 'Phase 1: Foundation' (Protocol in workflow.md)

## Phase 2: Flag Prerequisites — Eval-Time Gate (stitchd-core)
<!-- depends: phase1 -->

- [x] Task 1: (Red) Failing unit tests — gate returns fallback variant when a prerequisite
  is unmet; transitive chains; disabled prerequisite flag ⇒ unmet; missing prerequisite flag
  ⇒ unmet; trace names the failing prerequisite + fallback taken. (5349fff)
- [x] Task 2: (Green) Add prerequisites to the `Flag` aggregate; implement the gate in
  `engine.rs` before rule iteration; reuse the `evaluated_flags` map; extend
  `orchestrator.rs` topo-sort + cycle detection to include prerequisite edges. (99e9bed)
- [x] Task 3: Emit prerequisite decision into `EvaluationTrace`; refactor; verify ≥90%. (6e851c4)
  <!-- files: crates/stitchd-core/src/evaluation/engine.rs, crates/stitchd-core/src/rule_engine/orchestrator.rs, crates/stitchd-core/src/evaluation/types.rs, crates/stitchd-core/src/flag.rs -->
- [ ] Task: Conductor - User Manual Verification 'Phase 2: Prerequisites Core' (Protocol in workflow.md)

## Phase 3: Scheduler Core + Flag Scheduling (stitchd-schedule-service)
<!-- execution: parallel -->
<!-- depends: phase1 -->

- [x] Task 1: (Red→Green) `ScheduledChangeRepository` in `stitchd-db` — CRUD + due-query
  (`next_run_at <= now()`), run-history append, restart-safe state. TDD repo tests. (SHA 257a655)
  <!-- files: crates/stitchd-db/src/repository/pg/scheduled_changes.rs -->
- [x] Task 2: New `stitchd-schedule-service` binary — tokio interval loop (stats-service
  pattern); one-shot + recurring (RRULE+chrono-tz, DST, catch-up, idempotent); recompute
  `next_run_at`. TDD with a controllable clock. (SHA a818fc5)
  <!-- files: crates/stitchd-schedule-service/src/main.rs, crates/stitchd-schedule-service/src/scheduler.rs, crates/stitchd-schedule-service/Cargo.toml -->
  <!-- depends: task1 -->
- [x] Task 3: Flag apply path — dispatch a due flag change via canonical `MutateFlag`;
  honor the experiment lock (skip + record `failed`/`deferred` with reason); audit as
  system actor; version-bump. TDD incl. locked-flag skip. (SHA 4e1ee90)
  <!-- files: crates/stitchd-schedule-service/src/apply/flag.rs -->
  <!-- depends: task2 -->
- [ ] Task: Conductor - User Manual Verification 'Phase 3: Scheduler Core' (Protocol in workflow.md)

## Phase 4: Prerequisites — Persistence, Service, Gateway, Snapshot, Flag Delete-Block
<!-- depends: phase1, phase2 -->

- [x] Task 1: (Red→Green) Repository — prerequisites CRUD + `entity_dependencies` edge writes. (SHA 58ea071)
  <!-- files: crates/stitchd-db/src/repository/pg/prerequisites.rs -->
- [x] Task 2: flag-service — `SetPrerequisites`/`GetPrerequisites` with write-time cycle
  detection (reject `400` INVALID_ARGUMENT + cycle path); populate prerequisites + fallback into
  BOTH definition-sync and evaluate-preview snapshots. TDD. (SHA b115625)
  <!-- files: crates/stitchd-flag-service/src/service.rs, crates/stitchd-flag-service/src/prerequisites.rs -->
- [x] Task 3: Flag referential integrity — block flag delete/archive while referenced as a
  prerequisite (`409 DEPENDENCY_EXISTS` + dependents). TDD. (service-side SHA b115625; gateway
  decode SHA b8fefa2)
  <!-- files: crates/stitchd-flag-service/src/service.rs, crates/stitchd-gateway/src/error.rs -->
  <!-- depends: task1 -->
- [x] Task 4: Gateway `/v1/projects/{pid}/flags/{fid}/prerequisites` routes + OpenAPI. TDD. (SHA b8fefa2)
  <!-- files: crates/stitchd-gateway/src/routes/flags.rs, crates/stitchd-gateway/src/router.rs -->
  <!-- depends: task2 -->
- [ ] Task: Conductor - User Manual Verification 'Phase 4: Prerequisites Backend' (Protocol in workflow.md)

## Phase 5: Scheduler — Experiments + Segments + Routes + Experiment Start-Prereqs
<!-- depends: phase3 -->

- [x] Task 1: Experiment apply path — schedule start/pause/resume/stop/archive transitions;
  validate transition at fire time. TDD incl. invalid-transition skip. (SHA c223ba0)
  <!-- files: crates/stitchd-schedule-service/src/apply/experiment.rs -->
- [x] Task 2: Experiment **start-time prerequisites** (flag-in-variant / experiment-stopped) —
  enforced on manual AND scheduled start (`409`/reason if unmet). TDD. (SHA 1d7160a)
  <!-- files: crates/stitchd-experimentation-service/src/service.rs, crates/stitchd-experimentation-service/src/start_prerequisites.rs, crates/stitchd-db/migrations/20260604000002_experiment_start_prerequisites.sql -->
- [x] Task 3: Segment apply path — scheduled definition update / list-generation activation. TDD.
  (SHA fca5f66; list-generation NOT supported — no activation RPC; rejected w/ reason)
  <!-- files: crates/stitchd-schedule-service/src/apply/segment.rs -->
- [x] Task 4: Gateway `/schedules` sub-routes for flags, segments, experiments + OpenAPI. TDD.
  (SHA 12f6f18)
  <!-- files: crates/stitchd-gateway/src/routes/schedules.rs, crates/stitchd-gateway/src/router.rs -->
- [ ] Task: Conductor - User Manual Verification 'Phase 5: Scheduling — All Entities' (Protocol in workflow.md)

## Phase 6: Cross-Entity Dependency Graph + Referential Integrity (Segments/Experiments)
<!-- depends: phase4 -->

- [x] Task 1: Segment & experiment delete/archive blocking (`409 DEPENDENCY_EXISTS`) when
  referenced (flag→segment, segment→segment, experiment→flag). TDD. (SHA dea4283)
  <!-- files: crates/stitchd-segmentation-service/src/grpc/service.rs, crates/stitchd-segmentation-service/src/dependency_scan.rs, crates/stitchd-experimentation-service/src/service.rs, crates/stitchd-experimentation-service/src/start_prerequisites.rs -->
- [x] Task 2: Dependency-graph read API (upstream/downstream for an entity) — gateway
  orchestration over services. TDD. (SHA d751524)
  <!-- files: crates/stitchd-gateway/src/routes/dependencies.rs, crates/stitchd-gateway/src/router.rs, crates/stitchd-gateway/src/openapi.rs -->
- [ ] Task: Conductor - User Manual Verification 'Phase 6: Dependency Integrity' (Protocol in workflow.md)

## Phase 7: SDK Support (Rust)
<!-- depends: phase2, phase4 -->

- [x] Task 1: (Red) SDK failing tests — fallback on unmet prerequisite; transitive chain;
  disabled prerequisite; prerequisite flag absent from snapshot. (af91d9d)
- [x] Task 2: (Green) SDK snapshot carries prerequisites + fallback; supply all prerequisite
  flag definitions for transitive local resolution; confirm parity with preview via shared
  `evaluate_flag`. Verify ≥90%. (c320a18)
  <!-- files: sdks/rust/src/snapshot.rs, sdks/rust/src/client.rs, sdks/rust/tests/prerequisites.rs -->
- [ ] Task: Conductor - User Manual Verification 'Phase 7: SDK' (Protocol in workflow.md)

## Phase 8: Admin UI — Full Parity
<!-- execution: parallel -->
<!-- depends: phase4, phase5, phase6 -->

- [x] Task 1: API client + types for schedules, prerequisites, dependency graph. (102c455)
  <!-- files: admin/src/lib/api.ts, admin/src/lib/validation/lifecycle.ts -->
- [x] Task 2: Schedule builder (flag/segment/experiment pages) — one-shot + recurring, IANA
  tz picker, pending/active list, cancel/pause/resume, diff preview, run status. Formik+Yup, vitest. (c4231b8, 6bee41a)
  <!-- files: admin/src/pages/flags/ScheduleBuilder.tsx, admin/src/components/schedule -->
  <!-- depends: task1 -->
- [x] Task 3: Prerequisites editor (flag page) — add/remove (flag, required variant), fallback
  picker, live cycle warning. Formik+Yup, vitest. (47e5aa9)
  <!-- files: admin/src/pages/flags/PrerequisitesEditor.tsx -->
  <!-- depends: task1 -->
- [x] Task 4: Dependency-graph visualization (recharts) + delete-blocked UX (`409` surfacing)
  + badges + preview-trace prerequisite surfacing. vitest. (fd1ae2a)
  <!-- files: admin/src/components/dependency/DependencyGraph.tsx, admin/src/pages/flags/FlagDetail.tsx -->
  <!-- depends: task1 -->
- [ ] Task: Conductor - User Manual Verification 'Phase 8: Admin UI' (Protocol in workflow.md)

## Phase 9: Docs, CI, Final Integration
<!-- depends: phase5, phase6, phase7, phase8 -->

- [x] Task 1: Update `product.md` (modules + status row), `tech-stack.md` (schedule-service,
  tables, deps, env-vars). Add `stitchd-schedule-service` to docker-compose + CI build/test
  matrix + E2E where applicable. (5ee323d) — CI already covers the crate via the
  workspace-wide build/test/coverage jobs (no per-crate matrix exists); no self-seeding
  live-CH tests so the stats-service `--test` list is untouched.
  <!-- files: conductor/product.md, conductor/tech-stack.md, docker-compose.yml, .github/workflows/ci.yml -->
- [x] Task 2: `cargo sqlx prepare` offline cache (zero `.sqlx/` drift — runtime queries);
  `cargo xtask docs` idempotent (clean 2nd-run diff; schedule README generated);
  schedule-service coverage 82.18%→92.59% (≥90%); full gates green — clippy -D, fmt,
  workspace tests (2625 pass; only env-only `eval_preview_clickhouse` needs a live
  daemon), contract, admin tsc/lint/vitest 924/build. (docs e855658, tests d0ae7dc, fmt 1bf240c)
- [ ] Task: Conductor - User Manual Verification 'Phase 9: Docs & CI' (Protocol in workflow.md)

## Phase 10: Follow-Up Completions (Revision #1)
<!-- depends: phase9 -->
<!-- Added by revision #1 (2026-06-05): completes three items the initial implementation
     deferred as fail-closed / definition-only. Sequential — tasks 1 & 2 both touch
     stitchd-experimentation-service. -->

- [x] Task 1: Flag-service variant-UUID exposure (closes `feature-flag-bun`). Add variant UUIDs
  to flag-service `GetFlag`/`FeatureFlag` (backward-compatible proto + populate), then make the
  experimentation-service `flag_variant` start-prerequisite compare the actual served variant UUID
  exactly — replacing today's fail-closed behaviour. TDD: a MET `flag_variant` prereq now allows
  start; unmet still refuses (manual + scheduled). (SHA a75f790)
  <!-- files: proto/flags/v1/flag_service.proto, crates/stitchd-flag-service/src/service.rs, crates/stitchd-experimentation-service/src/start_prerequisites.rs -->
- [ ] Task 2: Experiment start-prerequisite read RPC (closes `feature-flag-coe`). Add a
  read RPC (e.g. `GetExperimentStartPrerequisites`, or fold into `GetExperiment`) + gateway wiring
  so the dependency-graph API's experiment branch is populated (currently a `note`), and surface
  configured start-prereqs in the Admin UI experiment page. TDD.
  <!-- files: proto/experiments/v1/experiment_service.proto, crates/stitchd-experimentation-service/src/service.rs, crates/stitchd-gateway/src/routes/dependencies.rs, admin/src/pages/experiments -->
- [ ] Task 3: Segment list-generation activation RPC. Add a segmentation-service RPC to activate a
  prepared list-segment generation, and wire schedule-service `apply/segment.rs` to use it for
  `list_generation` payloads — replacing today's reject-with-reason so scheduled list-segment
  generation swaps actually fire. TDD.
  <!-- files: proto/segments/v1/segment_service.proto, crates/stitchd-segmentation-service/src/service.rs, crates/stitchd-schedule-service/src/apply/segment.rs -->
- [ ] Task: Conductor - User Manual Verification 'Phase 10: Follow-Up Completions' (Protocol in workflow.md)
