# Plan: Fix Lint Errors

This plan outlines the steps to resolve all linting and formatting issues across the `stitchd` workspace.

## Phase 1: Research and Reproduction [checkpoint: 2f4f815]
- [x] Task: Run full workspace linting and formatting checks to identify all issues. (2f4f815)
  - Sub-task: Run `cargo clippy --workspace --all-targets --all-features` and save output.
  - Sub-task: Run `cargo fmt --all -- --check` and save output.
- [x] Task: Conductor - User Manual Verification 'Research and Reproduction' (Protocol in workflow.md) (2f4f815)

## Phase 2: Fix `stitchd-core` Lints
<!-- execution: parallel -->
- [x] Task: Fix Clippy and formatting issues in `crates/stitchd-core/src/flag.rs`. <!-- files: crates/stitchd-core/src/flag.rs -->
- [x] Task: Fix Clippy and formatting issues in `crates/stitchd-core/src/segment.rs`. <!-- files: crates/stitchd-core/src/segment.rs -->
- [x] Task: Fix Clippy and formatting issues in `crates/stitchd-core/src/rule_engine/`. <!-- files: crates/stitchd-core/src/rule_engine/ -->
- [x] Task: Fix Clippy and formatting issues in other `stitchd-core` modules. <!-- files: crates/stitchd-core/src/lib.rs, crates/stitchd-core/src/context.rs, crates/stitchd-core/src/hashing.rs, crates/stitchd-core/src/id.rs, crates/stitchd-core/src/tenant.rs, crates/stitchd-core/src/user.rs, crates/stitchd-core/src/variants.rs -->
- [x] Task: Conductor - User Manual Verification 'Fix stitchd-core Lints' (Protocol in workflow.md)

## Phase 3: Fix `stitchd-db` Lints
<!-- execution: parallel -->
- [x] Task: Fix Clippy and formatting issues in `crates/stitchd-db/src/repository/`. <!-- files: crates/stitchd-db/src/repository/ -->
- [x] Task: Fix Clippy and formatting issues in `crates/stitchd-db/src/lib.rs` and `error.rs`. <!-- files: crates/stitchd-db/src/lib.rs, crates/stitchd-db/src/error.rs -->
- [x] Task: Fix Clippy and formatting issues in `crates/stitchd-db/tests/`. <!-- files: crates/stitchd-db/tests/ -->
- [x] Task: Conductor - User Manual Verification 'Fix stitchd-db Lints' (Protocol in workflow.md)

## Phase 4: Fix `stitchd-server` and Remaining Crates
<!-- execution: parallel -->
- [x] Task: Fix Clippy and formatting issues in `crates/stitchd-server/src/`. <!-- files: crates/stitchd-server/src/ -->
- [x] Task: Fix Clippy and formatting issues in `crates/stitchd-proto/`, `crates/stitchd-events/`, and `crates/stitchd-sdk/`. <!-- files: crates/stitchd-proto/src/, crates/stitchd-events/src/, crates/stitchd-sdk/src/ -->
- [x] Task: Conductor - User Manual Verification 'Fix stitchd-server and Remaining Crates' (Protocol in workflow.md)

## Phase 5: Final Workspace Verification
- [x] Task: Verify zero warnings/errors across the entire workspace. (ed61fad)
  - Sub-task: Run `cargo clippy --workspace --all-targets --all-features`. ✓ 0 errors
  - Sub-task: Run `cargo fmt --all -- --check`. ✓ 0 diffs
  - Sub-task: Run all tests with `cargo test --workspace` to ensure no regressions.
- [ ] Task: Conductor - User Manual Verification 'Final Workspace Verification' (Protocol in workflow.md)
