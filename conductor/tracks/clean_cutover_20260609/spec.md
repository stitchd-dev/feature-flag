# Spec: Clean Cutover — Final-State Consolidation (clean_cutover_20260609)

## Overview

The Stitchd Feature Flag platform is **not yet live**. Since the last hard cutover
(`schema_cutover_20260525`), nine PostgreSQL and two ClickHouse incremental migrations
have accumulated, alongside a layer of "additive, backward-compatible" proto/API shims
and retired-but-still-present legacy artifacts (most notably the dual `events` /
`events_v2` ClickHouse tables).

Because there is no production data and no live client to preserve compatibility for,
this track performs a **clean cutover to final state**: collapse every store's schema
into a single fresh dated V1 baseline, strip backward-compatibility from the proto/API
surface (compacting tags freely), and delete dead legacy tables/code paths. There is
**no migration path and no backward-compatibility support** — the schema, contracts,
and code move directly to their final shape.

## Functional Requirements

### FR1 — Single fresh DB baselines (new dated files)
- **PostgreSQL:** Produce one new baseline `crates/stitchd-db/migrations/20260609000001_v1_baseline.sql`
  that represents the exact final schema (the 2026-05-25 baseline folded together with all
  nine post-baseline incrementals). **Delete** all prior PG migration files (old V1 baseline +
  the nine incrementals).
- **ClickHouse:** Produce one new baseline `crates/stitchd-event-writer/migrations/20260609000001_v1_baseline.sql`
  folding the old CH baseline + `experiment_interactions` + `eval_log_evaluation_id`. **Delete**
  the superseded CH migration files. Update the `event_writer::migrations` MIGRATIONS array to
  reference only the new baseline.
- **ScyllaDB:** Already a single baseline (`0001_v1_baseline.cql`) with no incrementals — rename/redate
  only if needed for consistency; otherwise leave as the single source of truth.
- Migrating a **fresh** database from each new baseline must reproduce the current applied schema
  to functional equivalence (same tables, columns, indexes, constraints, MVs, dictionaries, TTLs).

### FR2 — Collapse ClickHouse to a single `events` table
- The canonical table is **`events`** (per the user decision); `events_v2` is dropped entirely.
- The single `events` table carries the final multi-context schema
  (`contexts Array(Tuple(String, String))`, `metric_key LowCardinality(String)`, the three
  nullable typed value columns, `properties Map(String, String)`, the `DateTime64(3,'UTC')`
  timestamps). Determine the genuinely-active schema authoritatively from what the query layer
  actually reads (`event_query.rs`, `aggregation.rs`, `clickhouse_query.rs`, `ingestion.rs`, the
  `events_experiment_daily_mv`) and reconcile against the docs (which currently label `events_v2`
  canonical — a contradiction to resolve in this track).
- Drop the duplicate/legacy table and any MV/dictionary targeting it. Update all
  readers/writers/tests to the single `events` name; no `events_v2` reference may remain.

### FR3 — Strip proto/API backward-compatibility (compact tags)
- Remove "additive, backward-compatible" shims that exist solely to preserve an older contract:
  retired/duplicated fields, compat-only optionals, dead enum values, and any dual-write/dual-read
  paths. **Compact proto tag numbers** to their natural final contiguous order where a retired
  field left a gap (allowed because nothing is live — no `reserved` placeholders).
- Regenerate the proto stubs and update every gateway/service/SDK consumer to the final contract.

### FR4 — Remove dead legacy code paths
- Delete the Rust code, comments, and tests that exist only to support a migration path or an
  old contract (e.g. "legacy callers that don't supply context_type", dual-table query branches,
  retired-field mapping).

### FR5 — Dev database reset from scratch
- Reset the local Docker Postgres/ClickHouse/ScyllaDB from the new baselines via
  `scripts/reset_dev_db.sh --all`. Update that script (and `scripts/reset_dev_db.sh`) if it
  references deleted migration files.

### FR6 — Regenerate derived artifacts
- Regenerate the sqlx offline cache (`.sqlx/`) against the new baseline so CI's
  `cargo sqlx prepare --workspace --check` passes.
- Regenerate doc artifacts (`cargo xtask docs`) and confirm zero drift; update `product.md` /
  `tech-stack.md` schema/proto/migration sections to describe the new single-baseline final state.
- Keep CI's "Live-ClickHouse integration tests" explicit `--test` list in sync if any
  `stitchd-stats-service` test file is renamed/removed.

## Non-Functional Requirements

- **No migration/back-compat:** No upgrade path, no compatibility shim, no dual-write window is
  retained anywhere. Final state only.
- **Behaviour-preserving:** The *runtime behaviour* of the system (evaluation results, stats output,
  API responses for the final contract) is unchanged — only schema history, contract surface, and
  dead code change.
- **Quality gates:** `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`,
  `cargo test --workspace` all green; ≥90% coverage maintained per crate; `cargo xtask docs`
  idempotent; admin UI type-check + lint + vitest green if any contract it consumes changes.

## Acceptance Criteria

1. Each store has exactly **one** baseline migration file dated `20260609…` (Scylla: its single
   baseline), and all superseded migration files are deleted.
2. A from-scratch `scripts/reset_dev_db.sh --all` applies cleanly and yields a schema functionally
   identical to today's applied schema.
3. Exactly one canonical `events` table exists; `events_v2` and its dead MV are gone; all code
   references `events`.
4. Proto contracts contain no compat-only fields; tags are compacted to final contiguous order;
   all consumers compile and pass against the final contract.
5. `cargo fmt --check`, `cargo clippy -D warnings`, and `cargo test --workspace` pass; `.sqlx/` cache
   is regenerated and `cargo sqlx prepare --check` passes.
6. `cargo xtask docs && git diff --exit-code` is clean; `product.md`/`tech-stack.md` describe the new
   single-baseline final state with a dated cutover note.

## Out of Scope

- Any new feature or behavioural change to flag evaluation, experimentation, stats, or scheduling.
- Performance/optimization work beyond what the consolidation naturally produces.
- Admin UI feature changes (only mechanical updates if a consumed contract field is renumbered/removed).
- Retaining ANY backward-compatibility or migration path (explicitly excluded by the directive).
