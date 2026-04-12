# Plan: Segmentation

## Phase 1: Segment Evaluation Logic (`stitchd-core`)

- [x] Task 1: Define segment domain types
  - [ ] `SegmentDefinition` enum (`RuleBased` | `ListBased`)
  - [ ] `RuleBasedSegment { id: SegmentId, rules: Vec<Rule> }`
  - [ ] `ListBasedSegment { id: SegmentId, lists: HashMap<String, ContextList> }`
  - [ ] `ContextList { include: HashSet<String>, exclude: HashSet<String> }`
  - [ ] `MatchResult { matched: bool, trace: MatchTrace }`
  - [ ] `MatchTrace` enum (`RuleBased` | `ListBased`)
  - [ ] `ListMatchReason` enum (`Included` | `Excluded` | `NoMatch` | `NoContext`)
  - [ ] `SegmentEvaluatorError` enum (`RuleEngine(RuleEngineError)` | `InvalidSegmentRule`)
  - [ ] `///` doc comments on all public types

- [x] Task 2: Implement list-based segment evaluation
  - [ ] `evaluate_one` for `ListBasedSegment`
  - [ ] Exclude wins over include logic
  - [ ] `NoContext` when no matching context_type provided
  - [ ] `MatchTrace::ListBased` populated with context_type + reason

- [x] Task 3: Implement rule-based segment evaluation
  - [ ] Validate no `InSegment`/`NotInSegment` leaf in rules → `InvalidSegmentRule`
  - [ ] `evaluate_one` for `RuleBasedSegment` — delegates to Rule Engine with
        empty `resolved_segments` and empty `evaluated_flags`
  - [ ] `MatchTrace::RuleBased` populated with `matched_rule_index`

- [x] Task 4: Implement `evaluate_all`
  - [ ] Iterate `&[SegmentDefinition]` independently
  - [ ] Return `HashMap<SegmentId, MatchResult>` — entry for every segment

- [x] Task 5: Unit tests (≥90% coverage on segmentation module)
  - [ ] List-based: exclude wins over include
  - [ ] List-based: key in neither list → `NoMatch`
  - [ ] List-based: no matching context → `NoContext`
  - [ ] Rule-based: first matching rule wins
  - [ ] Rule-based: no match → `matched: false`, `matched_rule_index: None`
  - [ ] Rule-based: `InSegment` in rule → `InvalidSegmentRule`
  - [ ] `evaluate_all`: all segments evaluated independently

- [x] Task: Conductor - User Manual Verification 'Segment Evaluation Logic' (Protocol in workflow.md)

---

## Phase 2: Database Schema & Repository (`stitchd-db`)
<!-- execution: parallel -->

- [x] Task 1: Write `segment_rules` migration
  <!-- files: crates/stitchd-db/migrations/20260412000001_segment_rules.sql -->
  - [ ] `segment_rules(id, segment_id FK, rule_index, rule_def JSONB)`
  - [ ] `UNIQUE (segment_id, rule_index)` + index `(segment_id, rule_index ASC)`
  - [ ] Runs cleanly against fresh PostgreSQL

- [x] Task 2: Write `segment_list_entries` migration
  <!-- files: crates/stitchd-db/migrations/20260412000002_segment_list_entries.sql -->
  - [ ] `segment_list_entries` range-partitioned on `created_at`
  - [ ] `pg_partman` `create_parent` (monthly, premake=3)
  - [ ] `part_config`: `retention = NULL`, `infinite_time_partitions = true`
  - [ ] Index on `(segment_id, context_type, list_type)`
  - [ ] Runs cleanly against fresh PostgreSQL with pg_partman installed

- [x] Task 3: Extend `SegmentRepository` trait + implement `PgSegmentRepository`
  <!-- files: crates/stitchd-db/src/repositories/segment.rs -->
  <!-- depends: task1, task2 -->
  - [ ] Trait: `find_with_rules`, `find_with_list`, `upsert_rules`, `set_list_entries`
  - [ ] `find_with_rules` — query ordered by `rule_index`, deserialize JSONB → `Rule`
  - [ ] `find_with_list` — query grouped by `context_type`/`list_type`
  - [ ] `upsert_rules` — delete existing then insert (replace semantics)
  - [ ] `set_list_entries` — delete for `(segment_id, context_type)` then insert
  - [ ] All queries use `sqlx::query!` macros
  - [ ] `///` doc comments on all new trait methods

- [x] Task 4: Integration tests
  <!-- files: crates/stitchd-db/tests/segment_repository.rs -->
  <!-- depends: task3 -->
  - [ ] Rule-based: upsert → find → round-trips correctly
  - [ ] Rule-based: second upsert replaces previous rules
  - [ ] List-based: set → find → round-trips correctly
  - [ ] List-based: set replaces previous entries for that context_type
  - [ ] Soft-delete → absent from `list_by_environment`
  - [ ] `find_with_rules` on list segment → `NotFound`

- [x] Task: Conductor - User Manual Verification 'Database Schema & Repository' (Protocol in workflow.md)

---

## Phase 3: REST Admin API & Maintenance (`stitchd-server`)
<!-- execution: parallel -->

- [x] Task 1: Define request/response Serde types + validation
  <!-- files: crates/stitchd-server/src/api/segments/types.rs -->
  - [ ] `CreateSegmentRequest`, `UpdateSegmentRequest` (with `version: i64`)
  - [ ] `SegmentResponse` (full definition + `version`)
  - [ ] `RuleBody` / `ListBody` JSON shapes
  - [ ] Validation fn: reject `InSegment`/`NotInSegment` in rule body → `422`

- [x] Task 2: Implement all 5 segment handler functions
  <!-- files: crates/stitchd-server/src/api/segments/handlers.rs -->
  <!-- depends: task1 -->
  - [ ] `list_segments`, `create_segment`, `get_segment`, `update_segment`, `delete_segment`
  - [ ] `VersionConflict` → `409`, `NotFound` → `404`, `UniqueViolation` → `409`, validation → `422`

- [x] Task 3: Wire segment routes into Axum router
  <!-- files: crates/stitchd-server/src/api/router.rs -->
  <!-- depends: task2 -->
  - [ ] Register all 5 routes under `/v1/environments/:env_id/segments`
  - [ ] Pass `SegmentRepository` via `AppState`

- [x] Task 4: Wire pg_partman maintenance background task
  <!-- files: crates/stitchd-server/src/startup.rs -->
  - [ ] On startup: `SELECT partman.run_maintenance(false)`
  - [ ] Tokio background task: repeat every 1 hour

- [x] Task 5: Endpoint integration tests
  <!-- files: crates/stitchd-server/tests/segments.rs -->
  <!-- depends: task2, task3, task4 -->
  - [ ] `POST` → `GET` round-trip for both segment types
  - [ ] `PUT` correct version → `200`; stale version → `409`
  - [ ] `DELETE` → absent from list
  - [ ] `POST` with `InSegment` in rule → `422`
  - [ ] `GET` unknown segment → `404`

- [x] Task: Conductor - User Manual Verification 'REST Admin API & Maintenance' (Protocol in workflow.md)
