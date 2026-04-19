# Plan: Experimentation Module — Experiment CRUD
Track: experimentation_20260419

## Phase 1: Database Migrations & Domain Types [checkpoint: 8c79067]
<!-- execution: parallel -->

- [x] Task 1: PostgreSQL migrations [4cd3ac7]
  <!-- files: crates/stitchd-db/migrations/ -->
  - [x] Sub-task: Add `frozen` boolean column to `flag_rules` table
  - [x] Sub-task: Create `experiments` table (id, env_id, flag_rule_id, name,
        description, hypothesis, status, metric_keys[], traffic_allocation,
        min_sample_size, scheduled_start_at, scheduled_end_at, version,
        created_at, updated_at, deleted_at)
  - [x] Sub-task: Create `experiment_iterations` table (id, experiment_id,
        iteration_number, started_at, ended_at, metric_keys[],
        traffic_allocation, min_sample_size)
  - [x] Sub-task: Unique partial index: one running/paused experiment
        per flag_rule_id

- [x] Task 2: Domain types in `stitchd-core` [0c2b614]
  <!-- files: crates/stitchd-core/src/experimentation/ -->
  - [x] Sub-task: Write failing tests for ExperimentStatus transitions
        (valid and invalid)
  - [x] Sub-task: ExperimentId newtype, ExperimentIterationId newtype
  - [x] Sub-task: ExperimentStatus enum + allowed_transitions() method
  - [x] Sub-task: Experiment struct + ExperimentIteration struct
  - [x] Sub-task: Transition validation logic (mutation guard, uniqueness
        guard, creates_iteration predicate)
  - [x] Sub-task: Pass tests

- [x] Task: Conductor - User Manual Verification 'Phase 1: Database Migrations & Domain Types' (Protocol in workflow.md) [8c79067]

## Phase 2: Repository Layer [checkpoint: b6ccf63]

- [x] Task 1: ExperimentRepository trait + sqlx implementation [e7dd2a8]
  - [x] Sub-task: Write failing tests (sqlx::test) for create, get, list,
        soft-delete
  - [x] Sub-task: Implement create / get / list / update / soft-delete
  - [x] Sub-task: Pass tests

- [x] Task 2: Transition repository operations [1a66e42]
  - [x] Sub-task: Write failing tests for transition + iteration creation
        (paused→running creates new iteration; stopped→running creates next)
  - [x] Sub-task: Implement apply_transition: update status, freeze/unfreeze
        flag rule, insert iteration row if transitioning into running
  - [x] Sub-task: Write failing tests for uniqueness guard (one
        running/paused per rule → Err)
  - [x] Sub-task: Implement uniqueness check in apply_transition
  - [x] Sub-task: Pass all tests

- [x] Task: Conductor - User Manual Verification 'Phase 2: Repository Layer' (Protocol in workflow.md) [b6ccf63]

## Phase 3: REST API Layer [checkpoint: 018af3c]

- [x] Task 1: Request/Response types & utoipa schemas [d946dac]
  - [x] Sub-task: Write failing tests for request deserialization and
        validation (metric_keys empty → error)
  - [x] Sub-task: CreateExperimentRequest, UpdateExperimentRequest,
        TransitionRequest, response types with utoipa annotations
  - [x] Sub-task: Pass tests

- [x] Task 2: Route handlers [881f54a]
  - [x] Sub-task: Write failing integration tests (tower::oneshot) for
        POST /experiments, GET /experiments, GET /experiments/{id},
        PATCH /experiments/{id}, DELETE /experiments/{id},
        POST /experiments/{id}/transitions,
        GET /experiments/{id}/iterations
  - [x] Sub-task: Implement all handlers with mutation guards and JWT auth
  - [x] Sub-task: Wire OpenTelemetry spans on each handler
  - [x] Sub-task: Register routes in stitchd-server router
  - [x] Sub-task: Pass all integration tests

- [x] Task 3: OpenAPI spec verification [881f54a]
  - [x] Sub-task: Run xtask docs, confirm all experiment endpoints appear
        in generated OpenAPI spec

- [x] Task: Conductor - User Manual Verification 'Phase 3: REST API Layer' (Protocol in workflow.md) [018af3c]

## Phase 4: Coverage & Quality Gate [checkpoint: e993143]

- [x] Task 1: Full lifecycle integration tests [604f988]
  - [x] Sub-task: draft→running→paused→running→stopped full lifecycle
  - [x] Sub-task: stopped→running restart (iteration number increments)
  - [x] Sub-task: Mutation guard: PATCH while running → 409
  - [x] Sub-task: Flag rule frozen while running; PATCH flag rule → 409
  - [x] Sub-task: Two experiments on same rule, second start → 409
  - [x] Sub-task: Soft-delete while running → 409; while stopped → 204

- [x] Task 2: Coverage verification [f72d847]
  - [x] Sub-task: Run cargo-tarpaulin on stitchd-core, stitchd-db,
        stitchd-server (experiment paths)
  - [x] Sub-task: Achieve ≥ 90% coverage on new code; add missing tests
        until threshold met

- [x] Task: Conductor - User Manual Verification 'Phase 4: Coverage & Quality Gate' (Protocol in workflow.md) [f72d847]
