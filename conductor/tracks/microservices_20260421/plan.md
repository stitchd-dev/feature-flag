# Plan: Microservice Architecture Decomposition

## Phase 1: Proto Contracts & Shared gRPC Definitions [checkpoint: 78ff491]

- [x] Task 1: Write proto compilation tests — confirm existing `stitchd-proto` builds cleanly (baseline) [0ed8e8c]
- [x] Task 2: Define `auth_service.proto` — `ValidateCredential(CredentialRequest) → RbacContext` [80b0867]
  <!-- files: proto/auth_service.proto -->
- [x] Task 3: Define `flag_service.proto` — `GetFlagDefinitions(stream)`, `GetFlag`, `ListFlags`, `MutateFlag` [80b0867]
  <!-- files: proto/flag_service.proto -->
- [x] Task 4: Define `segmentation_service.proto` — `GetSegment`, `ListSegments`, `EvaluateMembership`, `MutateSegment` [80b0867]
  <!-- files: proto/segmentation_service.proto -->
- [x] Task 5: Define `event_service.proto` — `IngestEvent(EventRequest) → IngestResponse` [80b0867]
  <!-- files: proto/event_service.proto -->
- [x] Task 6: Define `experimentation_service.proto` — `CreateExperiment`, `GetExperiment`, `ListExperiments`, `GetResults` [80b0867]
  <!-- files: proto/experimentation_service.proto -->
- [x] Task 7: Regenerate Rust bindings, verify workspace compiles with all new proto contracts [80b0867]
  <!-- depends: task2, task3, task4, task5, task6 -->
- [x] Task: Conductor - User Manual Verification 'Proto Contracts & Shared gRPC Definitions' (Protocol in workflow.md) [ca08993]

## Phase 2: Auth Service (`stitchd-auth-service`)
<!-- depends: phase1 -->

- [~] Task 1: Scaffold `crates/stitchd-auth-service` — `Cargo.toml`, `main.rs`, `lib.rs`, module structure
- [ ] Task 2: Write failing unit tests for JWT credential validation gRPC handler
- [ ] Task 3: Implement JWT validation — migrate from `stitchd-server/src/auth/`; wire to `AuthService` gRPC trait
- [ ] Task 4: Write failing unit tests for SDK key credential validation gRPC handler
- [ ] Task 5: Implement SDK key validation — query `auth` schema via sqlx, enforce active-key constraint
- [ ] Task 6: Build RBAC context assembly — map tenant/env/roles to `RbacContext` proto message
- [ ] Task 7: Wire tonic gRPC server in `main.rs` with graceful shutdown; add Prometheus metrics endpoint
- [ ] Task 8: Verify >95% unit test coverage for crate
- [ ] Task: Conductor - User Manual Verification 'Auth Service' (Protocol in workflow.md)

## Phase 3: Flag Service (`stitchd-flag-service`)
<!-- depends: phase1 -->

- [ ] Task 1: Scaffold `crates/stitchd-flag-service` — `Cargo.toml`, `main.rs`, `lib.rs`, module structure
- [ ] Task 2: Write failing tests for `GetFlagDefinitions` streaming handler (definition sync contract)
- [ ] Task 3: Implement flag CRUD handlers — migrate from `stitchd-server/src/api/flags/`; owns `flags` schema
- [ ] Task 4: Write failing tests for flag mutation handlers (create, update, delete, archive)
- [ ] Task 5: Implement flag mutation handlers with optimistic locking (version field)
- [ ] Task 6: Implement `GetFlagDefinitions` server-streaming gRPC handler for SDK sync
- [ ] Task 7: Wire tonic gRPC server in `main.rs` with graceful shutdown; metrics
- [ ] Task 8: Verify >95% unit test coverage for crate
- [ ] Task: Conductor - User Manual Verification 'Flag Service' (Protocol in workflow.md)

## Phase 4: Segmentation Service (`stitchd-segmentation-service`)
<!-- depends: phase1 -->

- [ ] Task 1: Scaffold `crates/stitchd-segmentation-service` — `Cargo.toml`, `main.rs`, `lib.rs`, module structure
- [ ] Task 2: Write failing tests for rule-based segment evaluation handler
- [ ] Task 3: Implement segment CRUD — migrate from `stitchd-server/src/api/segments/`; owns `segments` schema
- [ ] Task 4: Write failing tests for list-based segment membership evaluation
- [ ] Task 5: Implement `EvaluateMembership` gRPC handler (rule-based + list-based paths)
- [ ] Task 6: Wire tonic gRPC server in `main.rs` with graceful shutdown; metrics
- [ ] Task 7: Verify >95% unit test coverage for crate
- [ ] Task: Conductor - User Manual Verification 'Segmentation Service' (Protocol in workflow.md)

## Phase 5: Experimentation Event Service (`stitchd-event-service`)
<!-- depends: phase1 -->

- [ ] Task 1: Scaffold `crates/stitchd-event-service` — `Cargo.toml`, `main.rs`, `lib.rs`, module structure
- [ ] Task 2: Write failing tests for `IngestEvent` handler — unknown key rejection, type validation
- [ ] Task 3: Implement event definition registry — migrate from `stitchd-server/src/event_definitions/`; owns `events` schema (PostgreSQL)
- [ ] Task 4: Implement `IngestEvent` gRPC handler — validate against registry, write to ClickHouse
- [ ] Task 5: Wire tonic gRPC server in `main.rs` with graceful shutdown; metrics
- [ ] Task 6: Verify >95% unit test coverage for crate
- [ ] Task: Conductor - User Manual Verification 'Experimentation Event Service' (Protocol in workflow.md)

## Phase 6: Experimentation Service (`stitchd-experimentation-service`)
<!-- depends: phase3 -->

- [ ] Task 1: Scaffold `crates/stitchd-experimentation-service` — `Cargo.toml`, `main.rs`, `lib.rs`, module structure
- [ ] Task 2: Write failing tests for experiment CRUD gRPC handlers
- [ ] Task 3: Implement experiment CRUD — migrate from `stitchd-server/src/api/experiments/`; owns `experiments` schema
- [ ] Task 4: Write failing tests for flag-lock integration (experiment activate → Flag Service gRPC call)
- [ ] Task 5: Implement Flag Service gRPC client — call `GetFlag` to verify and lock flag state during experiment lifecycle
- [ ] Task 6: Implement `GetResults` handler — reads from pre-computed `experiment_results` table (PostgreSQL)
- [ ] Task 7: Wire tonic gRPC server in `main.rs` with graceful shutdown; metrics
- [ ] Task 8: Verify >95% unit test coverage for crate
- [ ] Task: Conductor - User Manual Verification 'Experimentation Service' (Protocol in workflow.md)

## Phase 7: Orchestration / Gateway Service (`stitchd-gateway`)
<!-- depends: phase2, phase3, phase4, phase5, phase6 -->

- [ ] Task 1: Scaffold `crates/stitchd-gateway` — Axum REST server + tonic gRPC clients for all domain services
- [ ] Task 2: Write failing tests for auth middleware — every request must call Auth Service gRPC before forwarding
- [ ] Task 3: Implement auth middleware — call `AuthService::ValidateCredential`, inject `RbacContext` into request extensions
- [ ] Task 4: Write failing tests for flag route proxying (REST → gRPC Flag Service)
- [ ] Task 5: Implement flag route handlers — translate REST JSON ↔ gRPC proto, forward to Flag Service
- [ ] Task 6: Write failing tests for segment route proxying
- [ ] Task 7: Implement segment route handlers
- [ ] Task 8: Write failing tests for event route proxying
- [ ] Task 9: Implement event route handlers
- [ ] Task 10: Write failing tests for experiment route proxying
- [ ] Task 11: Implement experiment route handlers
- [ ] Task 12: Implement SDK gRPC passthrough — gateway accepts SDK `GetFlagDefinitions` stream, proxies to Flag Service
- [ ] Task 13: Migrate OpenAPI spec annotations (`utoipa`) from `stitchd-server` to gateway handlers
- [ ] Task 14: Verify >95% unit test coverage for crate
- [ ] Task: Conductor - User Manual Verification 'Orchestration / Gateway Service' (Protocol in workflow.md)

## Phase 8: Docker Compose Wiring & End-to-End Integration
<!-- depends: phase7 -->

- [ ] Task 1: Update `docker-compose.yml` — add six service containers with port assignments and service discovery env vars
- [ ] Task 2: Write end-to-end integration test — SDK client connects to gateway, syncs flags, evaluates a flag
- [ ] Task 3: Write end-to-end integration test — REST client authenticates via gateway, creates flag, creates experiment
- [ ] Task 4: Verify no existing REST API contract is broken (diff OpenAPI spec against pre-decomposition snapshot)
- [ ] Task 5: Remove or deprecate `stitchd-server` crate from workspace
- [ ] Task 6: Update CI workflow — build and test each service crate independently
- [ ] Task: Conductor - User Manual Verification 'Docker Compose Wiring & End-to-End Integration' (Protocol in workflow.md)
