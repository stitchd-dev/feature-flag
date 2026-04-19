# Spec: Events Layer

## Overview

Implement the events ingestion foundation for the Experimentation module. This covers:
- Pre-registered event definitions (PostgreSQL)
- REST ingestion API (`POST /events`, `POST /events/batch`)
- ClickHouse schema: raw events table + metric_definitions table + materialized views for aggregations
- Unknown events rejected at ingestion boundary

## Functional Requirements

### Event Registration (PostgreSQL)
- Events are pre-registered per environment with a `key` (unique within environment) and a typed metric value (`bool | int | double`)
- Registration enforced at ingestion: unregistered event keys → 422 rejection
- Soft-delete support; version-based optimistic locking on event definitions
- Audit log entry on create/update/delete

### Ingestion API (REST)
- `POST /v1/environments/{env_id}/events` — single event
- `POST /v1/environments/{env_id}/events/batch` — bulk ingestion (array payload)
- Payload: `{ contexts: [{_type, key}], metric_key, value, timestamp }`
- Auth: SDK key (`x-sdk-key` header), scoped to project + environment
- Unknown metric_key → 422; type mismatch → 422; valid → 202 Accepted (async write)
- Batch endpoint: max 500 events per request

### ClickHouse Schema
- `events` table: raw ingested events (env_id, contexts as Array(Tuple(String, String)), metric_key, value_bool, value_int, value_double, timestamp, ingested_at) — partitioned by toYYYYMM(timestamp)
- `metric_definitions` table: mirrors PostgreSQL event registration for query-time joins
- Materialized views:
  - `events_count_mv` — count per (env_id, metric_key, day)
  - `events_numeric_mv` — sum/avg/p50/p95/p99 per (env_id, metric_key, day)

## Non-Functional Requirements
- Async ClickHouse write (fire-and-forget after validation); REST returns 202
- OpenTelemetry spans on ingestion path
- utoipa annotations for OpenAPI generation
- Coverage ≥ 90% on new code

## Acceptance Criteria
- [ ] Event definitions CRUD API (create, list, soft-delete) with optimistic locking
- [ ] `POST /events` and `POST /events/batch` reject unknown keys and type mismatches
- [ ] Batch endpoint rejects payloads > 500 events
- [ ] ClickHouse migrations create events + metric_definitions tables and both materialized views
- [ ] Integration tests: valid event accepted, unknown key rejected, type mismatch rejected, batch works, batch > 500 rejected
- [ ] Coverage ≥ 90% on new code

## Out of Scope
- Experiment CRUD, statistical analysis, Bayesian/Frequentist models
- SDK direct event submission (future)
- Warehouse-backed ingestion
- Admin UI
- context `parameters` on event payload (evaluation-time concern only)
