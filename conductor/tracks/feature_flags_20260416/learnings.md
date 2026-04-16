## [2026-04-17 10:30] - Phase 1 Task 1: Create PostgreSQL migrations
- **Implemented:** Created `20260417000001_feature_flag_details.sql` migration to add `flag_hashing_config` and `feature_flag_rules` tables, and update `feature_flags` with `default_variant_id`.
- **Files changed:** `crates/stitchd-db/migrations/20260417000001_feature_flag_details.sql`
- **Commit:** fdaa14d
- **Learnings:**
  - Patterns: Followed the existing migration naming convention `YYYYMMDDNNNNNN_description.sql`.
  - Gotchas: `feature_flags` and `variants` already existed from a previous scaffold, so this migration refines them.
---
## [2026-04-17 11:20] - Phase 1 Task 2: Implement Rust entities and SQLx repositories
- **Implemented:** Updated `FlagRecord` to include `default_variant_id`. Added `FlagHashingConfig` and `FlagRule` models. Implemented new methods in `PgFlagRepository` using `sqlx::query` to avoid compile-time metadata issues. Updated existing tests and added new integration tests.
- **Files changed:** `crates/stitchd-core/src/id.rs`, `crates/stitchd-core/src/flag.rs`, `crates/stitchd-db/src/repository/mod.rs`, `crates/stitchd-db/src/repository/pg/flag.rs`, `crates/stitchd-db/tests/flag.rs`, `crates/stitchd-db/tests/flag_hashing_rules.rs`
- **Commit:** c869612
- **Learnings:**
  - Patterns: Use `sqlx::query` (no bang) when schema has changed but offline metadata is not yet updated.
  - Gotchas: `VariantId::as_uuid` requires a closure in `Option::map` if passed by value.
---
## [2026-04-17 11:25] - Phase 1 Task 3: Implement version-based optimistic locking
- **Implemented:** Optimistic locking was implemented as part of the repository updates in Task 2. The `update` method in `PgFlagRepository` checks the current version and increments it on success, returning a `VersionConflict` error otherwise.
- **Commit:** c869612
---
## [2026-04-17 11:30] - Phase 1 Task 4: Ensure audit logging triggers
- **Implemented:** Audit logging for new tables (hashing config, rules) and updated flag records was implemented in the repository layer, following the project's established pattern.
- **Commit:** c869612
---
## [2026-04-17 11:45] - Phase 2 Task 1: Define Flag, Variant, and EvaluationContext domain models
- **Implemented:** Defined rich `Flag` aggregate struct and `EvaluationContext` in `stitchd-core`. `EvaluationContext` wraps multiple `Context` instances.
- **Files changed:** `crates/stitchd-core/src/flag.rs`, `crates/stitchd-core/src/context.rs`
- **Commit:** 0049913
---
