# Plan: Experimentation Module — Experiment CRUD
Track: experimentation_20260419

## Phase 1: Database Migrations & Domain Types
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

- [ ] Task: Conductor - User Manual Verification 'Phase 1: Database Migrations & Domain Types' (Protocol in workflow.md)

## Phase 2: Repository Layer

- [ ] Task 1: ExperimentRepository trait + sqlx implementation
  - [ ] Sub-task: Write failing tests (sqlx::test) for create, get, list,
        soft-delete
  - [ ] Sub-task: Implement create / get / list / update / soft-delete
  - [ ] Sub-task: Pass tests

- [ ] Task 2: Transition repository operations
  - [ ] Sub-task: Write failing tests for transition + iteration creation
        (paused→running creates new iteration; stopped→running creates next)
  - [ ] Sub-task: Implement apply_transition: update status, freeze/unfreeze
        flag rule, insert iteration row if transitioning into running
  - [ ] Sub-task: Write failing tests for uniqueness guard (one
        running/paused per rule → Err)
  - [ ] Sub-task: Implement uniqueness check in apply_transition
  - [ ] Sub-task: Pass all tests

- [ ] Task: Conductor - User Manual Verification 'Phase 2: Repository Layer' (Protocol in workflow.md)

## Phase 3: REST API Layer

- [ ] Task 1: Request/Response types & utoipa schemas
  - [ ] Sub-task: Write failing tests for request deserialization and
        validation (metric_keys empty → error)
  - [ ] Sub-task: CreateExperimentRequest, UpdateExperimentRequest,
        TransitionRequest, response types with utoipa annotations
  - [ ] Sub-task: Pass tests

- [ ] Task 2: Route handlers
  - [ ] Sub-task: Write failing integration tests (tower::oneshot) for
        POST /experiments, GET /experiments, GET /experiments/{id},
        PATCH /experiments/{id}, DELETE /experiments/{id},
        POST /experiments/{id}/transitions,
        GET /experiments/{id}/iterations
  - [ ] Sub-task: Implement all handlers with mutation guards and JWT auth
  - [ ] Sub-task: Wire OpenTelemetry spans on each handler
  - [ ] Sub-task: Register routes in stitchd-server router
  - [ ] Sub-task: Pass all integration tests

- [ ] Task 3: OpenAPI spec verification
  - [ ] Sub-task: Run xtask docs, confirm all experiment endpoints appear
        in generated OpenAPI spec

- [ ] Task: Conductor - User Manual Verification 'Phase 3: REST API Layer' (Protocol in workflow.md)

## Phase 4: Coverage & Quality Gate

- [ ] Task 1: Full lifecycle integration tests
  - [ ] Sub-task: draft→running→paused→running→stopped full lifecycle
  - [ ] Sub-task: stopped→running restart (iteration number increments)
  - [ ] Sub-task: Mutation guard: PATCH while running → 409
  - [ ] Sub-task: Flag rule frozen while running; PATCH flag rule → 409
  - [ ] Sub-task: Two experiments on same rule, second start → 409
  - [ ] Sub-task: Soft-delete while running → 409; while stopped → 204

- [ ] Task 2: Coverage verification
  - [ ] Sub-task: Run cargo-tarpaulin on stitchd-core, stitchd-db,
        stitchd-server (experiment paths)
  - [ ] Sub-task: Achieve ≥ 90% coverage on new code; add missing tests
        until threshold met

- [ ] Task: Conductor - User Manual Verification 'Phase 4: Coverage & Quality Gate' (Protocol in workflow.md)
