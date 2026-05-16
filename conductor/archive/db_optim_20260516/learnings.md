# Track Learnings: db_optim_20260516

Patterns, gotchas, and context discovered during implementation.

## Codebase Patterns (Inherited)

- `CREATE INDEX CONCURRENTLY` cannot run inside a transaction — sqlx migrations
  wrap each file in a transaction by default; use `-- migrate:noTransaction` or
  split into separate migration files.
- `sqlx::query!` macros require live DB or up-to-date `.sqlx` cache. Run
  `cargo sqlx prepare --workspace` after adding new queries.
- `#[sqlx::test(migrations = "./migrations")]` for isolated DB tests.
- ClickHouse crate v0.13 has no `derive` feature — use `uuid`, `time`, `lz4` features.
- `moka` cache with `time_to_live` is the project's chosen in-process cache primitive
  (added this track).
- `COUNT(*) OVER()` window function avoids a second COUNT query for pagination totals.
- ClickHouse `AggregatingMergeTree` + `AggregateFunction` state columns require
  `initializeAggregation` on insert and `finalizeAggregation` on read.
- `toMonday(date)` for weekly ClickHouse partitions (returns the Monday of that week).

---

<!-- Learnings from implementation will be appended below -->
