# Plan: Clean Cutover — Final-State Consolidation (clean_cutover_20260609)

Execution: **sequential** (all phases). The proto/dead-code phases share consumer files,
and the DB phases both end in a `reset_dev_db.sh` + test run that would race on the same
dev containers; parallelism savings are small and conflict risk is real.

## Phase 1: PostgreSQL Final-State Baseline

- [x] Task 1.1: [91949fe] Capture the fully-migrated PG schema (pg_dump --schema-only of a dev DB
      migrated through all 10 files) as the consolidation target, and author
      `crates/stitchd-db/migrations/20260609000001_v1_baseline.sql` reproducing the exact
      final schema (all tables/columns/constraints; the 6 partial soft-delete indexes;
      sdk-key + covering + context-registry indexes; exclusion_groups + unit_context_type;
      lifecycle_automation; experiment_start_prerequisites; bandit_foundation + lifecycle;
      idempotency_keys; flag_key partial-unique fix; frozen column already absent).
- [x] Task 1.2: [91949fe] Delete the old PG baseline + the nine post-baseline incremental migration files.
- [x] Task 1.3: [91949fe] Reset dev Postgres from scratch; verify the single baseline applies cleanly and
      its schema is functionally identical to the pre-cutover applied schema (diff two pg_dump
      --schema-only outputs → zero meaningful diff).
- [x] Task 1.4: [91949fe] Regenerate `.sqlx/` offline cache; confirm
      `cargo sqlx prepare --workspace --check -- --all-targets --features stitchd-sdk-rust/test-util` passes.
- [x] Task: Conductor - User Manual Verification 'PostgreSQL Final-State Baseline' [autonomous] (Protocol in workflow.md)

## Phase 2: ClickHouse Final-State Baseline + Single `events` Table

- [x] Task 2.1: [ef8c97a] Determine the canonical `events` schema authoritatively (the query layer reads
      `FROM events`; docs label `events_v2` canonical) and document the collapse: a single
      `events` table carrying the multi-context `Array(Tuple(String,String))` schema; the
      duplicate/legacy table dropped.
- [x] Task 2.2: [ef8c97a] Author `crates/stitchd-event-writer/migrations/20260609000001_v1_baseline.sql`
      folding old CH baseline + `experiment_interactions` (N-way schema) + `eval_log_evaluation_id`
      into one file with: single `events` table, `flag_evaluation_log` (+ evaluation_id),
      `experiment_results` (+ sequential_result), `experiment_assignments` (+ _mv),
      `events_experiment_daily` (+ _mv reading `FROM events`), `experiment_interactions`,
      and the `experiment_iterations_active` dictionary. No `events_v2` anywhere.
- [x] Task 2.3: [ef8c97a] Delete superseded CH migration files; update the `event_writer::migrations`
      MIGRATIONS array to reference only the new baseline.
- [x] Task 2.4: [ef8c97a] Update every CH reader/writer/test to the single `events` name (event_query.rs,
      ingestion.rs, aggregation.rs, clickhouse_query.rs, dispatch.rs, stats queries,
      clickhouse_views.rs) — assert no `events_v2` references remain.
- [x] Task 2.5: [ef8c97a] Reset dev ClickHouse from scratch; run the live-CH integration + view tests
      (`--ignored` self-seeding set) to verify the baseline + single-table collapse.
- [x] Task: Conductor - User Manual Verification 'ClickHouse Final-State Baseline + Single events Table' [autonomous] (Protocol in workflow.md)

## Phase 3: Proto/API Backward-Compat Removal + Tag Compaction

- [x] Task 3.1: [f95b809] Inventory compat-only / retired / duplicated proto fields, dead enum values, and
      any `*_v2` message names across all `.proto` files (enabled_override optional, additive
      sequential/bandit/pre_period_days fields, MutationKind values, etc.).
- [x] Task 3.2: [f95b809] Remove compat-only fields and **compact tag numbers** to final contiguous order
      (no `reserved` gaps); regenerate tonic/prost stubs.
- [x] Task 3.3: [f95b809] Update every consumer to the final contract (gateway routes, each service's
      mapping.rs/service.rs, the Rust SDK); restore compile + behaviour parity.
- [x] Task 3.4: [f95b809] Update proto contract tests (`stitchd-proto/src/tests.rs`) and the gateway
      OpenAPI contract check to the final surface.
- [x] Task: Conductor - User Manual Verification 'Proto/API Backward-Compat Removal + Tag Compaction' [autonomous] (Protocol in workflow.md)

## Phase 4: Dead Legacy Code-Path Removal

- [x] Task 4.1: [9c60185] Remove migration-path / old-contract Rust branches and comments (e.g. the
      "legacy callers that don't supply context_type" branch in experiment_results.rs, dual-table
      query branches, retired-field mappings).
- [x] Task 4.2: [9c60185] Grep-sweep residual `legacy|backward|compat|_v2|deprecated` references; remove or
      rewrite each; confirm no dead compat code remains.
- [x] Task: Conductor - User Manual Verification 'Dead Legacy Code-Path Removal' [autonomous] (Protocol in workflow.md)

## Phase 5: Derived Artifacts, Docs, and Full-Stack Verification

- [ ] Task 5.1: Update `scripts/reset_dev_db.sh` (+ `--all`) for the deleted migration files; run a
      full `scripts/reset_dev_db.sh --all` from scratch across PG/CH/ScyllaDB.
- [ ] Task 5.2: Regenerate docs (`cargo xtask docs`); update `product.md` / `tech-stack.md`
      schema/proto/migration sections to the new single-baseline final state with a dated cutover
      note; confirm `git diff --exit-code` is clean.
- [ ] Task 5.3: Full gate — `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`,
      `cargo test --workspace`, `cargo sqlx prepare --check`; sync CI's live-CH `--test` list if any
      stats test file changed; run admin UI type-check/lint/vitest if a consumed contract changed.
- [ ] Task: Conductor - User Manual Verification 'Derived Artifacts, Docs, and Full-Stack Verification' (Protocol in workflow.md)
