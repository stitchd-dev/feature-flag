# Plan: Increase Test Coverage to >90% Across All Crates

## Phase 1: stitchd-core Coverage Gaps
<!-- execution: sequential -->
<!-- depends: -->

- [x] Task 1: Audit current coverage in stitchd-core
  <!-- files: crates/stitchd-core/src/ -->
  - Run `cargo tarpaulin -p stitchd-core` and identify uncovered lines
  - Focus on: `condition.rs` (0 tests), `flag.rs`, `evaluation/engine.rs`

- [x] Task 2: Write tests for condition.rs
  <!-- files: crates/stitchd-core/src/rule_engine/condition.rs -->
  - Add `#[cfg(test)]` module covering all condition matching paths

- [x] Task 3: Fill coverage gaps in flag.rs and evaluation/engine.rs
  <!-- files: crates/stitchd-core/src/flag.rs, crates/stitchd-core/src/evaluation/engine.rs -->
  - Add tests for any uncovered branches in flag lifecycle and engine evaluation

- [x] Task 4: Verify stitchd-core reaches ≥90%
  - Run `cargo tarpaulin -p stitchd-core --fail-under 90`
  - Result: 97.42% (454/466 lines), commit f1bc615

- [ ] Task: Conductor - User Manual Verification 'stitchd-core Coverage' (Protocol in workflow.md)

## Phase 2: stitchd-events and stitchd-sdk Coverage Gaps
<!-- execution: sequential -->
<!-- depends: -->

- [ ] Task 1: Audit coverage in stitchd-events and stitchd-sdk
  <!-- files: crates/stitchd-events/src/, crates/stitchd-sdk/src/ -->
  - Run tarpaulin per crate, identify gaps
  - stitchd-sdk gaps: `error.rs`, `config.rs`, `http_client.rs`, `grpc_client.rs`

- [ ] Task 2: Write tests for stitchd-events
  <!-- files: crates/stitchd-events/src/lib.rs -->
  - Verify existing 2 test markers cover all paths; add any missing

- [ ] Task 3: Write tests for stitchd-sdk error.rs and config.rs
  <!-- files: crates/stitchd-sdk/src/error.rs, crates/stitchd-sdk/src/config.rs -->
  - Unit tests for error variants and config validation

- [ ] Task 4: Write tests for stitchd-sdk http_client.rs
  <!-- files: crates/stitchd-sdk/src/http_client.rs -->
  - Mock HTTP responses using `wiremock` or `httpmock`

- [ ] Task 5: Verify stitchd-events and stitchd-sdk each reach ≥90%
  - Run `cargo tarpaulin -p stitchd-events -p stitchd-sdk --fail-under 90`

- [ ] Task: Conductor - User Manual Verification 'stitchd-events and stitchd-sdk Coverage' (Protocol in workflow.md)

## Phase 3: stitchd-server Coverage Gaps
<!-- execution: sequential -->
<!-- depends: -->

- [ ] Task 1: Audit coverage in stitchd-server
  <!-- files: crates/stitchd-server/src/ -->
  - Run tarpaulin, identify gaps
  - Key gaps: `api/flags/handlers.rs` (349 lines), `api/segments/handlers.rs` (290 lines), `api/segments/types.rs`, `api/router.rs`

- [ ] Task 2: Write handler tests for api/flags/handlers.rs
  <!-- files: crates/stitchd-server/src/api/flags/handlers.rs -->
  - Use `tower::ServiceExt::oneshot` pattern (from patterns.md)
  - Cover CRUD paths, error cases, auth failures

- [ ] Task 3: Write handler tests for api/segments/handlers.rs and types.rs
  <!-- files: crates/stitchd-server/src/api/segments/handlers.rs, crates/stitchd-server/src/api/segments/types.rs -->
  - Same Axum oneshot pattern, cover request/response types

- [ ] Task 4: Write tests for api/router.rs and sdk_auth.rs gaps
  <!-- files: crates/stitchd-server/src/api/router.rs, crates/stitchd-server/src/api/sdk_auth.rs -->
  - Router wiring tests, SDK auth edge cases

- [ ] Task 5: Verify stitchd-server reaches ≥90%
  - Run `cargo tarpaulin -p stitchd-server --fail-under 90`

- [ ] Task: Conductor - User Manual Verification 'stitchd-server Coverage' (Protocol in workflow.md)

## Phase 4: stitchd-db Coverage (Unit + Integration)
<!-- execution: sequential -->
<!-- depends: -->

- [ ] Task 1: Audit and plan stitchd-db coverage strategy
  <!-- files: crates/stitchd-db/src/ -->
  - Identify pure-logic paths in each repository file (validation, error mapping, query building)
  - Plan which functions need unit tests vs integration tests

- [ ] Task 2: Add unit tests for stitchd-db error.rs and pure logic
  <!-- files: crates/stitchd-db/src/error.rs, crates/stitchd-db/src/repository/mod.rs -->
  - Mock-free tests for error conversion, validation helpers

- [ ] Task 3: Add sqlx::test integration tests for repository/pg/flag.rs and segment.rs
  <!-- files: crates/stitchd-db/src/repository/pg/flag.rs, crates/stitchd-db/src/repository/pg/segment.rs -->
  - Use `#[sqlx::test]` with transaction rollback
  - Cover create, read, update, delete paths

- [ ] Task 4: Add sqlx::test integration tests for remaining repositories
  <!-- files: crates/stitchd-db/src/repository/pg/role.rs, crates/stitchd-db/src/repository/pg/organisation.rs, crates/stitchd-db/src/repository/pg/project.rs, crates/stitchd-db/src/repository/pg/user.rs, crates/stitchd-db/src/repository/pg/sdk_key.rs, crates/stitchd-db/src/repository/pg/environment.rs, crates/stitchd-db/src/repository/pg/audit.rs -->
  - role.rs, organisation.rs, project.rs, user.rs, sdk_key.rs, environment.rs, audit.rs

- [ ] Task 5: Verify stitchd-db reaches ≥90%
  - Run `cargo tarpaulin -p stitchd-db --fail-under 90`

- [ ] Task: Conductor - User Manual Verification 'stitchd-db Coverage' (Protocol in workflow.md)

## Phase 5: Workspace Validation
<!-- execution: sequential -->
<!-- depends: phase1, phase2, phase3, phase4 -->

- [ ] Task 1: Run full workspace coverage check
  - `cargo tarpaulin --workspace --exclude-files "crates/stitchd-proto/*" --fail-under 90`
  - Identify any remaining gaps across crates

- [ ] Task 2: Fix any remaining gaps to hit overall 90%
  - Targeted additions where workspace total falls short

- [ ] Task 3: Commit final coverage baseline
  - Commit all test files with message: `test(coverage): achieve >90% coverage across all crates`

- [ ] Task: Conductor - User Manual Verification 'Workspace Coverage Validation' (Protocol in workflow.md)
