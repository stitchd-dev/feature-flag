# Spec: Experimentation Module — Experiment CRUD

## Overview

Implement the Experiment CRUD layer. Covers experiment definitions bound
to a specific flag rule, an iteration-based lifecycle (each contiguous run
period is a distinct iteration), flag-rule freezing while running, and
mutation guards that prevent changes during active runs.

## Functional Requirements

### Experiment Entity
- Scoped to an environment; soft-delete; optimistic locking (`version`);
  audit log on all mutations
- Metadata: `name`, `description`, `hypothesis`
- `flag_rule_id` — bound to a specific rule; one active (running/paused)
  experiment per rule at a time
- `metric_keys[]` — 1+ pre-registered event definition keys (required)
- `traffic_allocation` — % of rule-matched contexts enrolled
  (0.1% granularity, default 100%)
- `min_sample_size` — optional informational guardrail
- `scheduled_start_at` / `scheduled_end_at` — optional duration window
  (persisted; enforcement out of scope)
- `status`: `draft | running | paused | stopped`

### Lifecycle & Transitions
Valid transitions:
- `draft → running`
- `running → paused`
- `paused → running`
- `running → stopped`
- `paused → stopped`
- `stopped → running` (restart)

Managed via `POST .../transitions` with a `{ "to": "<status>" }` body.

### Iteration Model
A new iteration is created on each transition **into** `running`:
- `draft → running` → Iteration 1
- `paused → running` → next iteration
- `stopped → running` (restart) → next iteration

Each iteration captures:
- `iteration_number`, `started_at`, `ended_at` (null while active)
- Snapshot of `metric_keys`, `traffic_allocation`, `min_sample_size`
  at the moment the iteration starts
- Immutable once `ended_at` is set

### Mutation Guards
Fields `name`, `description`, `hypothesis`, `flag_rule_id`, `metric_keys`,
`traffic_allocation`, `min_sample_size`, `scheduled_start_at`,
`scheduled_end_at` are mutable **only** in `draft`, `paused`, or `stopped`.
Mutation while `running` → 409 Conflict.

### Flag Rule Freezing
- On transition to `running`: target flag rule marked `frozen = true`
- On transition to `paused` or `stopped`: `frozen` cleared
- Any create/update/delete on a frozen rule → 409 Conflict

### Uniqueness
At most one experiment in `running` or `paused` state per `flag_rule_id`.
`draft` and `stopped` are unrestricted.

### API Endpoints
- `POST   /v1/environments/{env_id}/experiments`
- `GET    /v1/environments/{env_id}/experiments` (filter: status, flag_id)
- `GET    /v1/environments/{env_id}/experiments/{id}`
- `PATCH  /v1/environments/{env_id}/experiments/{id}`
- `DELETE /v1/environments/{env_id}/experiments/{id}` (draft/stopped only)
- `POST   /v1/environments/{env_id}/experiments/{id}/transitions`
- `GET    /v1/environments/{env_id}/experiments/{id}/iterations`

Auth: JWT (human) for all endpoints.

## Non-Functional Requirements
- Optimistic locking on experiment entity
- Audit log on all mutations and transitions
- OpenTelemetry spans on ingestion path
- utoipa annotations for OpenAPI generation
- Coverage ≥ 90% on new code

## Acceptance Criteria
- [ ] Experiment CRUD with correct mutation guards (running → 409)
- [ ] Transitions into `running` create new iterations with a snapshot
- [ ] Flag rule frozen on `running`; unfrozen on `paused`/`stopped`
- [ ] At most one running/paused experiment per flag rule (409 on conflict)
- [ ] Soft-delete only allowed in `draft` or `stopped` state
- [ ] All endpoints have utoipa annotations in OpenAPI spec
- [ ] Integration tests: full lifecycle, mutation guard, flag freeze,
      duplicate active experiment rejected, restart iteration numbering
- [ ] Coverage ≥ 90% on new code

## Out of Scope
- Statistical analysis (Frequentist/Bayesian/CUPED) — next track
- Metric result queries and aggregations
- Scheduled auto-start/stop enforcement (persisted, not acted on)
- Warehouse-backed event ingestion
- Admin UI
- SDK direct event submission
