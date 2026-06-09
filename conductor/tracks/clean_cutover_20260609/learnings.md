# Track Learnings: clean_cutover_20260609

Patterns, gotchas, and context discovered during implementation.

## Codebase Patterns (Inherited)

Inherited from `conductor/patterns.md` (the full pattern set from prior tracks — keyset
pagination, worker-wave parallelism, stats-math de-duplication, live-CH integration-test
discipline, etc.). Especially relevant to this clean-cutover track:

- **Schema hard-cutover precedent (`schema_cutover_20260525`):** prior collapse of 44 PG /
  14 CH / 5 Scylla migrations into single V1 baselines. `flag_evaluation_log_v2` was renamed
  back to `flag_evaluation_log` during that cutover — the same "drop the `_v2` suffix, keep one
  canonical name" move this track applies to `events_v2` → `events`.
- **In-place migration rewrite precedent (`nway_interaction_20260603`):** the CH
  `experiment_interactions` migration was rewritten in place (clean cutover, system not live) —
  no backfill assumed, fresh DB only.
- **Live-ClickHouse CI step gotcha:** the Coverage job has a SEPARATE
  "Live-ClickHouse integration tests (stats-service)" step listing each `--test` target by
  filename; `cargo llvm-cov` does NOT run `#[ignore]`d tests. Renaming/removing a self-seeding
  `stitchd-stats-service` `tests/*.rs` file silently reddens CI on the next push. Current set:
  aggregation_query, ratio_query, funnel_query, preview_query, interaction_compute, compute_pass,
  cuped_compute, percentile_significance.
- **sqlx offline cache:** after adding/removing `sqlx::query!` macros, regenerate with the SAME
  flags CI verifies (`--all-targets --features stitchd-sdk-rust/test-util`); dropping a query
  prunes its `.sqlx/` entry — commit the deletion.
- **ClickHouse eval-log table** uses `targeting_on Bool` (NOT `is_disabled`); a pre-existing
  evaluation_id schema-drift follow-up (`feature-flag` platform_hardening) is relevant to the
  CH baseline fold.

---

<!-- Learnings from implementation will be appended below -->

## [2026-06-09] - Phase 1: PostgreSQL Final-State Baseline (Tasks 1.1-1.4)
- **Implemented:** Collapsed 10 PG migrations into one 20260609000001_v1_baseline.sql.
- **Files changed:** crates/stitchd-db/migrations/ (10 deleted, 1 added); commit 91949fe
- **Learnings:**
  - Pattern: Build a consolidated baseline by pg_dump --schema-only of a fully-migrated
    scratch DB, then VERIFY via round-trip (apply baseline to fresh DB, dump, diff vs
    ground truth = zero diff). Robust + verifiable vs hand-folding.
  - Gotcha: pg_dump emits psql \restrict/\unrestrict meta-commands AND a
    'SELECT set_config(search_path,'',false)' that PERSISTS an empty search_path —
    this breaks sqlx's _sqlx_migrations bookkeeping ("relation _sqlx_migrations does
    not exist"). Strip the entire SET preamble + backslash meta-commands; all DDL is
    public.-qualified so none is needed.
  - Gotcha: dev 'stitchd' DB can hold idle connections that block 'sqlx database drop'
    ("being accessed by other users") — pg_terminate_backend first.
  - Context: schema is functionally identical, so the existing .sqlx cache validates
    unchanged (cargo sqlx prepare --check passed, no regen needed).
  - Context: discovered events_v2 + flag_evaluation_log_v2 already consolidated to
    events/flag_evaluation_log in the DB; crates/stitchd-db/clickhouse-migrations/ is
    DEAD (only README references it) -> Phase 4 removal. tech-stack.md wrongly lists
    events_v2 + pg_partman -> Phase 5 docs fix.
---
