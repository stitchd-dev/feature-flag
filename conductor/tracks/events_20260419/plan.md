# Implementation Plan: Events Layer

## Phase 1: ClickHouse Schema & Migrations
<!-- execution: parallel -->
<!-- depends: -->

- [x] Task: Create ClickHouse migration for `events` table (env_id, contexts Array(Tuple(String,String)), metric_key, value_bool, value_int, value_double, timestamp, ingested_at) using ReplicatedMergeTree, partitioned by toYYYYMM(timestamp)
  <!-- files: crates/stitchd-events/migrations/ -->
- [x] Task: Create ClickHouse migration for `metric_definitions` table (env_id, key, value_type) to mirror PostgreSQL registrations for query-time joins
  <!-- files: crates/stitchd-events/migrations/ -->
- [x] Task: Create materialized view `events_count_mv` — count per (env_id, metric_key, day)
  <!-- files: crates/stitchd-events/migrations/ -->
- [x] Task: Create materialized view `events_numeric_mv` — sum/avg/p50/p95/p99 per (env_id, metric_key, day)
  <!-- files: crates/stitchd-events/migrations/ -->
- [x] Task: Conductor - User Manual Verification 'Phase 1: ClickHouse Schema & Migrations' (Protocol in workflow.md)

## Phase 2: Event Registration (PostgreSQL)
<!-- execution: sequential -->
<!-- depends: -->

- [x] Task: Write failing tests for EventDefinition repository (create, list, soft-delete, optimistic locking)
  <!-- files: crates/stitchd-db/src/repositories/event_definition.rs -->
- [x] Task: Create PostgreSQL migration for `event_definitions` table (env_id, key, value_type enum, version, deleted_at, audit fields)
  <!-- files: crates/stitchd-db/migrations/ -->
- [x] Task: Implement `EventDefinition` domain model and `EventDefinitionRepository` in `stitchd-db`
  <!-- files: crates/stitchd-db/src/repositories/event_definition.rs, crates/stitchd-core/src/models/event.rs -->
- [x] Task: Implement `EventDefinitionService` in `stitchd-server` with optimistic locking and audit log writes
  <!-- files: crates/stitchd-server/src/services/event_definition.rs -->
- [x] Task: Implement REST handlers with utoipa annotations: `POST /v1/environments/{env_id}/event-definitions`, `GET /v1/environments/{env_id}/event-definitions`, `DELETE /v1/environments/{env_id}/event-definitions/{key}`
  <!-- files: crates/stitchd-server/src/handlers/event_definition.rs -->
- [x] Task: Conductor - User Manual Verification 'Phase 2: Event Registration (PostgreSQL)' (Protocol in workflow.md)

## Phase 3: Event Ingestion API
<!-- execution: sequential -->
<!-- depends: phase1, phase2 -->

- [ ] Task: Write failing tests for single and batch event ingestion (valid, unknown key, type mismatch, batch > 500)
  <!-- files: crates/stitchd-server/tests/event_ingestion.rs -->
- [ ] Task: Define `EventPayload` and `BatchEventPayload` types in `stitchd-core` (`contexts: Vec<EventContext>`, metric_key, typed value union, timestamp)
  <!-- files: crates/stitchd-core/src/models/event.rs -->
- [ ] Task: Implement ClickHouse writer in `stitchd-events` crate — async fire-and-forget with OTel spans
  <!-- files: crates/stitchd-events/src/writer.rs -->
- [ ] Task: Implement ingestion validation: resolve event definition from PostgreSQL, check value type match, reject unknown keys with 422
  <!-- files: crates/stitchd-server/src/services/event_ingestion.rs -->
- [ ] Task: Implement `POST /v1/environments/{env_id}/events` (single event) with SDK key auth, 202 on success, utoipa annotations
  <!-- files: crates/stitchd-server/src/handlers/events.rs -->
- [ ] Task: Implement `POST /v1/environments/{env_id}/events/batch` (max 500 events), 202 on success, utoipa annotations
  <!-- files: crates/stitchd-server/src/handlers/events.rs -->
- [ ] Task: Conductor - User Manual Verification 'Phase 3: Event Ingestion API' (Protocol in workflow.md)

## Phase 4: Integration Tests & Coverage
<!-- execution: sequential -->
<!-- depends: phase3 -->

- [ ] Task: Write integration tests: valid single event accepted (202), valid batch accepted (202), unknown metric_key → 422, type mismatch → 422, batch > 500 → 422
  <!-- files: crates/stitchd-server/tests/event_ingestion.rs -->
- [ ] Task: Verify ClickHouse materialized views populate correctly from test events
  <!-- files: crates/stitchd-events/tests/clickhouse_views.rs -->
- [ ] Task: Run coverage — enforce ≥90% on new code across `stitchd-events` and new handlers
- [ ] Task: Conductor - User Manual Verification 'Phase 4: Integration Tests & Coverage' (Protocol in workflow.md)
