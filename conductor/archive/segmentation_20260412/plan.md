# Plan: Segmentation

## Phase 1: Segment Evaluation Logic (`stitchd-core`)

- [x] Task 1: Define segment domain types
  - [x] `SegmentDefinition` enum (`RuleBased` | `ListBased`)
  - [x] `RuleBasedSegment { id: SegmentId, rules: Vec<Rule> }`
  - [x] `ListBasedSegment { id: SegmentId, lists: HashMap<String, ContextList> }`
  - [x] `ContextList { include: HashSet<String>, exclude: HashSet<String> }`
  - [x] `MatchResult { matched: bool, trace: MatchTrace }`
  - [x] `MatchTrace` enum (`RuleBased` | `ListBased`)
  - [x] `ListMatchReason` enum (`Included` | `Excluded` | `NoMatch` | `NoContext`)
  - [x] `SegmentEvaluatorError` enum (`RuleEngine(RuleEngineError)` | `InvalidSegmentRule`)
  - [x] `///` doc comments on all public types

- [x] Task 2: Implement list-based segment evaluation
  - [x] `evaluate_one` for `ListBasedSegment`
  - [x] Exclude wins over include logic
  - [x] `NoContext` when no matching context_type provided
  - [x] `MatchTrace::ListBased` populated with context_type + reason

- [x] Task 3: Implement rule-based segment evaluation
  - [x] Validate no `InSegment`/`NotInSegment` leaf in rules → `InvalidSegmentRule`
  - [x] `evaluate_one` for `RuleBasedSegment` — delegates to Rule Engine with
        empty `resolved_segments` and empty `evaluated_flags`
  - [x] `MatchTrace::RuleBased` populated with `matched_rule_index`

- [x] Task 4: Implement `evaluate_all`
  - [x] Iterate `&[SegmentDefinition]` independently
  - [x] Return `HashMap<SegmentId, MatchResult>` — entry for every segment

- [x] Task 5: Unit tests (≥90% coverage on segmentation module)
  - [x] List-based: exclude wins over include
  - [x] List-based: key in neither list → `NoMatch`
  - [x] List-based: no matching context → `NoContext`
  - [x] Rule-based: first matching rule wins
  - [x] Rule-based: no match → `matched: false`, `matched_rule_index: None`
  - [x] Rule-based: `InSegment` in rule → `InvalidSegmentRule`
  - [x] `evaluate_all`: all segments evaluated independently

- [x] Task: Conductor - User Manual Verification 'Segment Evaluation Logic' (Protocol in workflow.md)

---

## Phase 2: Database Schema & Repository (`stitchd-db`)
<!-- execution: parallel -->

- [x] Task 1: Write `segment_rules` migration
  <!-- files: crates/stitchd-db/migrations/20260412000001_segment_rules.sql -->
  - [x] `segment_rules(id, segment_id FK, rule_index, rule_def JSONB)`
  - [x] `UNIQUE (segment_id, rule_index)` + index `(segment_id, rule_index ASC)`
  - [x] Runs cleanly against fresh PostgreSQL

- [x] Task 2: Write `segment_list_entries` migration
  <!-- files: crates/stitchd-db/migrations/20260412000002_segment_list_entries.sql -->
  - [x] `segment_list_entries` range-partitioned on `created_at`
  - [x] `pg_partman` `create_parent` (monthly, premake=3)
  - [x] `part_config`: `retention = NULL`, `infinite_time_partitions = true`
  - [x] Index on `(segment_id, context_type, list_type)`
  - [x] Runs cleanly against fresh PostgreSQL with pg_partman installed

- [x] Task 3: Extend `SegmentRepository` trait + implement `PgSegmentRepository`
  <!-- files: crates/stitchd-db/src/repositories/segment.rs -->
  <!-- depends: task1, task2 -->
  - [x] Trait: `find_with_rules`, `find_with_list`, `upsert_rules`, `set_list_entries`
  - [x] `find_with_rules` — query ordered by `rule_index`, deserialize JSONB → `Rule`
  - [x] `find_with_list` — query grouped by `context_type`/`list_type`
  - [x] `upsert_rules` — delete existing then insert (replace semantics)
  - [x] `set_list_entries` — delete for `(segment_id, context_type)` then insert
  - [x] All queries use `sqlx::query!` macros
  - [x] `///` doc comments on all new trait methods

- [x] Task 4: Integration tests
  <!-- files: crates/stitchd-db/tests/segment_repository.rs -->
  <!-- depends: task3 -->
  - [x] Rule-based: upsert → find → round-trips correctly
  - [x] Rule-based: second upsert replaces previous rules
  - [x] List-based: set → find → round-trips correctly
  - [x] List-based: set replaces previous entries for that context_type
  - [x] Soft-delete → absent from `list_by_environment`
  - [x] `find_with_rules` on list segment → `NotFound`

- [x] Task: Conductor - User Manual Verification 'Database Schema & Repository' (Protocol in workflow.md)

---

## Phase 3: REST Admin API & Maintenance (`stitchd-server`)
<!-- execution: parallel -->

- [x] Task 1: Define request/response Serde types + validation
  <!-- files: crates/stitchd-server/src/api/segments/types.rs -->
  - [x] `CreateSegmentRequest`, `UpdateSegmentRequest` (with `version: i64`)
  - [x] `SegmentResponse` (full definition + `version`)
  - [x] `RuleBody` / `ListBody` JSON shapes
  - [x] Validation fn: reject `InSegment`/`NotInSegment` in rule body → `422`

- [x] Task 2: Implement all 5 segment handler functions
  <!-- files: crates/stitchd-server/src/api/segments/handlers.rs -->
  <!-- depends: task1 -->
  - [x] `list_segments`, `create_segment`, `get_segment`, `update_segment`, `delete_segment`
  - [x] `VersionConflict` → `409`, `NotFound` → `404`, `UniqueViolation` → `409`, validation → `422`

- [x] Task 3: Wire segment routes into Axum router
  <!-- files: crates/stitchd-server/src/api/router.rs -->
  <!-- depends: task2 -->
  - [x] Register all 5 routes under `/v1/environments/:env_id/segments`
  - [x] Pass `SegmentRepository` via `AppState`

- [x] Task 4: Wire pg_partman maintenance background task
  <!-- files: crates/stitchd-server/src/startup.rs -->
  - [x] On startup: `SELECT partman.run_maintenance(false)`
  - [x] Tokio background task: repeat every 1 hour

- [x] Task 5: Endpoint integration tests
  <!-- files: crates/stitchd-server/tests/segments.rs -->
  <!-- depends: task2, task3, task4 -->
  - [x] `POST` → `GET` round-trip for both segment types
  - [x] `PUT` correct version → `200`; stale version → `409`
  - [x] `DELETE` → absent from list
  - [x] `POST` with `InSegment` in rule → `422`
  - [x] `GET` unknown segment → `404`

- [x] Task: Conductor - User Manual Verification 'REST Admin API & Maintenance' (Protocol in workflow.md)
