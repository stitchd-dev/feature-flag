# Implementation Plan: fix_errors_20260412

#### Phase 1: Protocol & Build Infrastructure
- [x] Task: Fix `stitchd-proto` compilation and code generation
  - [x] Sub-task: Resolve `prost`/`tonic` build script issues in `crates/stitchd-proto/build.rs`
  - [x] Sub-task: Fix protobuf definition errors in `proto/**/*.proto`
  - [x] Sub-task: Ensure all downstream crates can successfully import generated code
- [x] Task: Resolve Workspace-wide Dependency Conflicts
  - [x] Sub-task: Check `Cargo.lock` for duplicate or incompatible version requirements
  - [x] Sub-task: Sync dependency versions across all `Cargo.toml` files
- [x] Task: Conductor - User Manual Verification 'Protocol & Build Infrastructure' (Protocol in workflow.md)

#### Phase 2: Core & Persistence Layers
- [x] Task: Fix `stitchd-core` Compilation and Lints
  - [x] Sub-task: Resolve type errors and broken imports in `crates/stitchd-core/src/`
  - [x] Sub-task: Fix all Clippy warnings in `stitchd-core`
- [x] Task: Fix `stitchd-db` and SQLx Query Validation
  - [x] Sub-task: Update `.sqlx/` offline cache files if schema changed
  - [x] Sub-task: Fix broken SQLx macros and query types in `crates/stitchd-db/src/`
  - [x] Sub-task: Resolve migration script issues
- [x] Task: Conductor - User Manual Verification 'Core & Persistence Layers' (Protocol in workflow.md)

#### Phase 3: Consumer Layers (Server & SDK) <!-- execution: parallel -->
- [x] Task: Fix `stitchd-server` Alignment <!-- files: crates/stitchd-server/ -->
  - [x] Sub-task: Resolve API handler type mismatches with new proto definitions
  - [x] Sub-task: Fix startup and telemetry errors in `crates/stitchd-server/src/`
- [x] Task: Fix `stitchd-sdk` Alignment <!-- files: crates/stitchd-sdk/ -->
  - [x] Sub-task: Resolve client-side gRPC client errors
  - [x] Sub-task: Fix SDK logic regressions
- [x] Task: Conductor - User Manual Verification 'Consumer Layers' (Protocol in workflow.md) <!-- depends: Task: Fix stitchd-server Alignment, Task: Fix stitchd-sdk Alignment -->

#### Phase 4: Final Workspace Verification
- [x] Task: Global Formatting and Linting
  - [x] Sub-task: Run `cargo fmt` across all crates
  - [x] Sub-task: Run `cargo clippy --workspace --all-targets -- -D warnings` and fix remaining issues
- [x] Task: Full Test Suite Execution
  - [x] Sub-task: Run `cargo test --workspace` and resolve all remaining failures
  - [x] Sub-task: Verify coverage reaches ≥ 90% threshold
- [x] Task: Conductor - User Manual Verification 'Final Workspace Verification' (Protocol in workflow.md)
