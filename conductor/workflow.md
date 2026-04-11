# Conductor Workflow

## Test Coverage
- Minimum coverage: **90%** (enforced in CI via `cargo-tarpaulin`)
- Domain logic (rule engine, segment evaluation, percentage allocation) targets near-100%
- Coverage checked before marking any task complete

## Commit Strategy
- Commit after **each task** completes
- Commit message format: `<type>(<scope>): <short description>`
  - Types: `feat`, `fix`, `refactor`, `test`, `docs`, `chore`
  - Example: `feat(flags): add percentage allocation rule evaluation`
- All commits must pass `cargo fmt --check` and `cargo clippy -- -D warnings`

## Task Summaries
- Use **Git Notes** to record task summaries (not commit messages)
- Git note format:
  ```
  Conductor Task: <task title>
  Track: <track_id>
  Summary: <what was done and why>
  Decisions: <any non-obvious choices made>
  ```

## Definition of Done (per task)
- [ ] Code compiles with no warnings
- [ ] `cargo fmt` applied
- [ ] `cargo clippy -- -D warnings` passes
- [ ] Unit tests written and passing
- [ ] Integration tests written where applicable
- [ ] Coverage ≥ 90% on changed code
- [ ] Public APIs documented with `///` doc comments
- [ ] No `privateParameters` fields logged anywhere in changed code
- [ ] Committed with descriptive message + Git Note

## Definition of Done (per phase)
- [ ] All tasks in phase complete
- [ ] Full test suite passes
- [ ] Manual verification step completed (Conductor User Verification)
- [ ] No regressions in existing tests

## Branch Strategy
- Feature work on `feature/<track_id>` branches
- Merge to `main` via PR after phase completion
- No direct commits to `main`
