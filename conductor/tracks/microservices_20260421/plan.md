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

- [x] Task 1: Scaffold `crates/stitchd-auth-service` — `Cargo.toml`, `main.rs`, `lib.rs`, module structure [6852431]
- [x] Task 2: Write failing unit tests for JWT credential validation gRPC handler [6852431]
- [x] Task 3: Implement JWT validation — migrate from `stitchd-server/src/auth/`; wire to `AuthService` gRPC trait [6852431]
- [x] Task 4: Write failing unit tests for SDK key credential validation gRPC handler [6852431]
- [x] Task 5: Implement SDK key validation — query `auth` schema via sqlx, enforce active-key constraint [6852431]
- [x] Task 6: Build RBAC context assembly — map tenant/env/roles to `RbacContext` proto message [6852431]
- [x] Task 7: Wire tonic gRPC server in `main.rs` with graceful shutdown; add Prometheus metrics endpoint [6852431]
- [x] Task 8: Verify >95% unit test coverage for crate [6852431]
- [ ] Task: Conductor - User Manual Verification 'Auth Service' (Protocol in workflow.md)

## Phase 3: Flag Service (`stitchd-flag-service`)
<!-- depends: phase1 -->

- [x] Task 1: Scaffold `crates/stitchd-flag-service` — `Cargo.toml`, `main.rs`, `lib.rs`, module structure [c581ec2]
- [x] Task 2: Write failing tests for `GetFlagDefinitions` streaming handler (definition sync contract) [c581ec2]
- [x] Task 3: Implement flag CRUD handlers — migrate from `stitchd-server/src/api/flags/`; owns `flags` schema [c581ec2]
- [x] Task 4: Write failing tests for flag mutation handlers (create, update, delete, archive) [c581ec2]
- [x] Task 5: Implement flag mutation handlers with optimistic locking (version field) [c581ec2]
- [x] Task 6: Implement `GetFlagDefinitions` server-streaming gRPC handler for SDK sync [c581ec2]
- [x] Task 7: Wire tonic gRPC server in `main.rs` with graceful shutdown; metrics [c581ec2]
- [x] Task 8: Verify >95% unit test coverage for crate (40 tests, all passing) [c581ec2]
- [ ] Task: Conductor - User Manual Verification 'Flag Service' (Protocol in workflow.md)

## Phase 4: Segmentation Service (`stitchd-segmentation-service`)
<!-- depends: phase1 -->

- [x] Task 1: Scaffold `crates/stitchd-segmentation-service` — `Cargo.toml`, `main.rs`, `lib.rs`, module structure [aff5be5]
- [x] Task 2: Write failing tests for rule-based segment evaluation handler [c859f22]
- [x] Task 3: Implement segment CRUD — migrate from `stitchd-server/src/api/segments/`; owns `segments` schema [c6402b7]
- [x] Task 4: Write failing tests for list-based segment membership evaluation [90b8499]
- [x] Task 5: Implement `EvaluateMembership` gRPC handler (rule-based + list-based paths) [90b8499]
- [x] Task 6: Wire tonic gRPC server in `main.rs` with graceful shutdown; metrics [782d418]
- [x] Task 7: Verify >95% unit test coverage for crate [a60a6ea]
- [ ] Task: Conductor - User Manual Verification 'Segmentation Service' (Protocol in workflow.md)

## Phase 5: Experimentation Event Service (`stitchd-event-service`)
<!-- depends: phase1 -->

- [x] Task 1: Scaffold `crates/stitchd-event-service` — `Cargo.toml`, `main.rs`, `lib.rs`, module structure [aa931e9]
- [x] Task 2: Write failing tests for `IngestEvent` handler — unknown key rejection, type validation [f72dcb5]
- [x] Task 3: Implement event definition registry — migrate from `stitchd-server/src/event_definitions/`; owns `events` schema (PostgreSQL) [ac7f282]
- [x] Task 4: Implement `IngestEvent` gRPC handler — validate against registry, write to ClickHouse [ac7f282]
- [x] Task 5: Wire tonic gRPC server in `main.rs` with graceful shutdown; metrics [aa931e9]
- [x] Task 6: Verify >95% unit test coverage for crate [1634020]
- [ ] Task: Conductor - User Manual Verification 'Experimentation Event Service' (Protocol in workflow.md)

## Phase 6: Experimentation Service (`stitchd-experimentation-service`)
<!-- depends: phase3 -->

- [x] Task 1: Scaffold `crates/stitchd-experimentation-service` — `Cargo.toml`, `main.rs`, `lib.rs`, module structure [53d5adb]
  <!-- files: crates/stitchd-experimentation-service/Cargo.toml, src/lib.rs, src/main.rs, src/service.rs, src/flag_client.rs -->
- [x] Task 2: Write failing tests for experiment CRUD gRPC handlers [53d5adb]
  <!-- files: crates/stitchd-experimentation-service/src/service.rs -->
- [x] Task 3: Implement experiment CRUD — migrate from `stitchd-server/src/api/experiments/`; owns `experiments` schema [53d5adb]
  <!-- files: crates/stitchd-experimentation-service/src/service.rs -->
- [x] Task 4: Write failing tests for flag-lock integration (experiment activate → Flag Service gRPC call) [53d5adb]
  <!-- files: crates/stitchd-experimentation-service/src/service.rs -->
- [x] Task 5: Implement Flag Service gRPC client — call `GetFlag` to verify and lock flag state during experiment lifecycle [53d5adb]
  <!-- files: crates/stitchd-experimentation-service/src/flag_client.rs -->
- [x] Task 6: Implement `GetResults` handler — reads from pre-computed `experiment_results` table (PostgreSQL) [53d5adb]
  <!-- files: crates/stitchd-experimentation-service/src/service.rs -->
- [x] Task 7: Wire tonic gRPC server in `main.rs` with graceful shutdown; metrics [53d5adb]
  <!-- files: crates/stitchd-experimentation-service/src/main.rs -->
- [x] Task 8: Verify >95% unit test coverage for crate [53d5adb]
  <!-- 33 unit tests, all pass, clippy -D warnings clean -->
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
