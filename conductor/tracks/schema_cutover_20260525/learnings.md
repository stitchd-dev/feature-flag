# Track Learnings: schema_cutover_20260525

Patterns, gotchas, and context discovered during implementation.

## Codebase Patterns (Inherited)

- SQLx offline cache (`cargo sqlx prepare --workspace -- --tests`) must include
  `-- --tests` to capture test-only queries; omitting it silently skips them
  and CI fails with `no cached data for this query`.
- `cargo clippy --workspace --all-targets -- -D warnings` treats all warnings
  as errors; run before committing.
- Composite type sqlx support requires implementing `PgHasArrayType` in addition
  to `Type + Encode + Decode` when the type is used as an array column.

## Key Constraints

- `hash_inputs` is already populated for all rows (backfilled by migration
  `20260522000001_hash_input_spec_cutover.sql`). No data migration needed.
- `segment_rules` has no new writers; segmentation-service only writes
  `condition_expr` for new segments. Removal is safe for fresh deployments.
- `calculate_allocation` bucket count drops from 100,000 to 10,000 — this is
  intentional and gives 0.01% precision matching storage. Hash bucket assignments
  for existing rules will change; this is acceptable for a fresh-deploy cutover.

---

<!-- Learnings from implementation will be appended below -->
