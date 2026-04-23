# Spec: Scheduled Stats Processing Microservice

## Overview

Introduce a new `stats` microservice responsible for computing experiment
results on a 1-hour periodic schedule. The existing on-demand recompute
path in the `experimentation` service is replaced by this scheduler-driven
approach. The Results API never triggers a live ClickHouse query at request
time — it reads exclusively from pre-computed results in PostgreSQL.

The `stats` service is deployed as a separate binary and Docker Compose
service to allow independent scaling and compute allocation.

## Functional Requirements

### Scheduler
- Runs every 60 minutes; processes all experiments with status `running`
- For each experiment, queries ClickHouse events bounded to the iteration
  window: `[iteration.started_at, iteration.ended_at]` — uses `NOW()` as
  the upper bound if the iteration is still active
- Writes computed results to the existing `experiment_results` table
  (one row per experiment iteration, upserted on each run)

### Job Tracking (new PostgreSQL tables)
- `stats_jobs` — one row per computation job:
  `(id, experiment_id, status: pending|running|completed|failed,
   created_at, started_at, completed_at, error)`
- `stats_schedule` — one row per experiment, tracks schedule state:
  `(experiment_id, last_computed_at, next_run_at,
   computation_status: ready|computing|never_computed)`

### Manual Recompute Endpoint
- `POST /experiments/{id}/recompute` — triggers an async background job,
  returns `{job_id, status: "pending", created_at}` (202 Accepted)
- `GET /jobs/{job_id}` — returns current job status and timestamps

### Results API (experimentation service)
- `GET /experiments/{id}/results` reads exclusively from
  `experiment_results` — no inline ClickHouse queries
- Response includes staleness metadata:
  - `computed_at` — ISO timestamp of last successful computation
  - `is_stale: bool` — true if `computed_at` is older than 60 minutes
  - `next_run_at` — estimated next scheduler run for this experiment
  - `computation_status` — `ready | computing | never_computed`

## Non-Functional Requirements

- `stats` microservice is a separate Rust binary with its own `Cargo.toml`
  crate in the workspace
- `experimentation` Results API must have zero live ClickHouse calls;
  enforce this at the repository/query layer
- Job and schedule state persists in PostgreSQL (no in-memory-only state)

## Acceptance Criteria

- [ ] `stats` service starts, connects to PostgreSQL and ClickHouse
- [ ] Scheduler fires every 60 min and processes all `running` experiments
- [ ] Event queries are bounded to `[iteration.started_at, ended_at/now]`
- [ ] `experiment_results` is upserted on each successful run
- [ ] `POST /experiments/{id}/recompute` returns a job_id (202)
- [ ] `GET /jobs/{job_id}` reflects live job status
- [ ] Results API response includes all four staleness fields
- [ ] Results API makes no direct ClickHouse calls

## Out of Scope

- ClickHouse materialized view optimizations (separate deferred track)
- Admin UI for stats visualization
- Multi-instance distributed scheduling (single scheduler instance assumed)
- Backfilling results for experiments that concluded before this service
  was deployed
