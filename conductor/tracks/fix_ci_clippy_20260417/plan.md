# Plan: Fix CI Clippy Failure — telemetry.rs map_unwrap_or

## Phase 1: Fix Clippy Lint Error

- [x] Task 1: Confirm CI failure reproduces locally
  - Run `cargo clippy --workspace` and confirm the `map_unwrap_or` error at `crates/stitchd-server/src/telemetry.rs:54`
  - This is the "Red" phase — verify the error exists before fixing

- [x] Task 2: Apply fix to telemetry.rs — e57eecf
  - Replace `.map(|v| v.eq_ignore_ascii_case("production")).unwrap_or(false)` with `.is_ok_and(|v| v.eq_ignore_ascii_case("production"))` on lines 54-56
  - Run `cargo clippy --workspace` — must pass with no errors

- [x] Task 3: Commit fix — e57eecf
  - Commit with message: `fix(telemetry): use is_ok_and instead of map().unwrap_or(false)`

- [ ] Task: Conductor - User Manual Verification 'Fix Clippy Lint Error' (Protocol in workflow.md)
