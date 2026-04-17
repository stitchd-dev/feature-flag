# Spec: Increase Test Coverage to >90% Across All Crates

## Overview

Current CI coverage job fails under the `--fail-under 90` threshold. This track
brings every crate (except `stitchd-proto`, which is generated code) to >90%
coverage individually, and the workspace overall to >90%.

## Functional Requirements

- Achieve >90% line coverage for each of these crates:
  - `stitchd-core`
  - `stitchd-db` (unit tests for pure logic + integration tests via `sqlx::test`)
  - `stitchd-events`
  - `stitchd-sdk`
  - `stitchd-server`
- Achieve >90% overall workspace coverage (measured by `cargo tarpaulin --workspace`)
- `stitchd-proto` is excluded (generated protobuf code)

## Non-Functional Requirements

- Integration tests for `stitchd-db` must use `sqlx::test` with transaction rollback — no persistent test state
- Tests must pass in CI (Postgres service available via GitHub Actions)
- No test should require mocking what can be tested directly
- `stitchd-db` unit tests mock the DB layer where testing pure logic (validation, mapping, error handling)

## Acceptance Criteria

- [ ] `cargo tarpaulin --workspace --exclude-files "crates/stitchd-proto/*" --fail-under 90` passes
- [ ] Each crate individually reports ≥90% coverage
- [ ] All tests pass in CI with Postgres service
- [ ] No flaky tests introduced

## Out of Scope

- `stitchd-proto` (generated code — excluded from tarpaulin)
- Achieving 100% coverage — 90% is the target
- Refactoring production code solely to improve testability
