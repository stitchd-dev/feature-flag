# Spec: Fix CI Clippy Failure — telemetry.rs map_unwrap_or

## Overview

CI is failing on every push due to a Clippy lint error in `stitchd-server`.
The `#[deny(warnings)]` attribute causes Clippy's `map_unwrap_or` lint to fail
compilation, blocking all merges.

## Functional Requirements

- Fix the Clippy `map_unwrap_or` error in `crates/stitchd-server/src/telemetry.rs:54-56`
- Replace `.map(|v| v.eq_ignore_ascii_case("production")).unwrap_or(false)`
  with `.is_ok_and(|v| v.eq_ignore_ascii_case("production"))`
- CI Clippy job must pass green after the fix

## Non-Functional Requirements

- No behavior change — semantics of `is_ok_and` match `map(...).unwrap_or(false)` exactly
- No other files should need changes for this specific fix

## Acceptance Criteria

- [ ] `cargo clippy --workspace` passes locally with no errors
- [ ] CI Clippy job passes on push
- [ ] No logic regression in `is_production` detection

## Out of Scope

- Fixing coverage threshold failures (separate issue)
- Any other Clippy warnings not currently failing CI
