# Specification: Fix Lint Errors

## Overview
This track focuses on resolving all existing lint and formatting errors across the workspace to ensure code quality and maintainability.

## Functional Requirements
- Resolve all Clippy warnings and errors across all crates (`stitchd-core`, `stitchd-db`, etc.).
- Ensure all files conform to the project's `rustfmt.toml` configuration.
- Identify and remove unused imports, variables, and dead code.
- Address complex or overly large functions identified by lints.

## Non-Functional Requirements
- **Code Quality:** Prioritize fixing underlying issues over suppression.
- **Maintainability:** Standardize code patterns across the codebase.

## Acceptance Criteria
- `cargo clippy` runs with zero warnings or errors across the entire workspace.
- `cargo fmt --check` passes for all files.
- All instances of `#[allow(...)]` that were masking fixable issues have been removed.
- All existing tests pass after lint fixes are applied.

## Out of Scope
- Updating `clippy.toml` or adding new strictness levels (e.g., `pedantic`, `nursery`).
- Refactoring logic that is not directly related to a lint violation.