# Specification: fix_errors_20260412

## Overview
This track addresses widespread errors across the entire `stitchd` workspace, including compilation failures, linting violations, formatting issues, test regressions, gRPC/Protobuf mismatches, SQLx query validation errors, and dependency conflicts. The goal is to restore the workspace to a fully healthy, CI-passing state.

## Functional Requirements
- **Protocol Integrity:** Resolve all `stitchd-proto` issues and ensure gRPC code generation is correct and consistent across the workspace.
- **Compilation:** All crates must compile without errors using `cargo check --workspace --all-targets`.
- **Linting & Formatting:** All code must adhere to `rustfmt` and pass `cargo clippy --workspace --all-targets -- -D warnings`.
- **Database Safety:** Resolve all SQLx compile-time query validation errors in `stitchd-db`.
- **Logic Correctness:** All unit and integration tests across the workspace must pass.

## Non-Functional Requirements
- **Protocol-First Approach:** Repairs will start with `stitchd-proto` to resolve downstream type and generation errors first.
- **CI Parity:** The final state must pass a full suite of checks (fmt, clippy, test, sqlx) as defined in `conductor/workflow.md`.

## Acceptance Criteria
- [ ] `cargo fmt --check` passes for the entire workspace.
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` passes with zero warnings/errors.
- [ ] `cargo test --workspace` passes all tests.
- [ ] `sqlx prepare --check` (or equivalent) passes for `stitchd-db`.
- [ ] `stitchd-proto` generates code without errors and downstream crates consume it successfully.

## Out of Scope
- Adding new features or changing existing functionality beyond what is required to fix errors.
- Major architectural refactorings, unless necessary to resolve a fundamental type or dependency conflict.
