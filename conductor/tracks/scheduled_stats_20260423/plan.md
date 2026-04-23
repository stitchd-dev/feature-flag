# Implementation Plan: scheduled_stats_20260423

## Phase 1: Database Schema & Repository Layer [checkpoint: 81b8cc9]
<!-- execution: parallel -->

- [x] Task 1: Write failing tests for `stats_jobs` repository 14264e9
  <!-- files: crates/stitchd-db/src/stats_jobs.rs -->
  - [x] Define `StatsJob` domain type and `StatsJobStatus` enum (`pending|running|completed|failed`)
  - [x] Test `create_job`, `get_job`, `update_job_status` repository methods
- [x] Task 2: Write failing tests for `stats_schedule` repository d88e2c6
  <!-- files: crates/stitchd-db/src/stats_schedule.rs -->
  - [x] Define `StatsSchedule` domain type and `ComputationStatus` enum (`ready|computing|never_computed`)
  - [x] Test `upsert_schedule`, `get_schedule_for_experiment` repository methods
- [x] Task 3: Create PostgreSQL migrations 8df78c5
  <!-- files: crates/stitchd-db/migrations/ -->
  <!-- depends: task1, task2 -->
  - [x] `stats_jobs(id, experiment_id, status, created_at, started_at, completed_at, error)`
  - [x] `stats_schedule(experiment_id PK, last_computed_at, next_run_at, computation_status)`
- [x] Task 4: Implement repository methods to pass tests ebca93e
  <!-- files: crates/stitchd-db/src/stats_jobs.rs, crates/stitchd-db/src/stats_schedule.rs -->
  <!-- depends: task3 -->
- [x] Task 5: Run `SQLX_OFFLINE=false cargo sqlx prepare --workspace` to update offline cache 67089a4
  <!-- depends: task4 -->
- [x] Task: Conductor - User Manual Verification 'Database Schema & Repository Layer' (Protocol in workflow.md)

## Phase 2: Stats Service Scaffold

- [ ] Task 1: Write failing tests for `StatsConfig` env-var parsing
  - [ ] Test postgres DSN, ClickHouse URL, scheduler interval fields load correctly
- [ ] Task 2: Create `stitchd-stats-service` crate in workspace
  - [ ] `Cargo.toml` with sqlx, clickhouse, tokio, axum, tokio-cron-scheduler dependencies
  - [ ] Add crate to workspace `Cargo.toml`
  - [ ] `src/main.rs` with graceful shutdown (`tokio::select!` over SIGTERM + ctrl_c)
  - [ ] `src/config.rs` env-based config struct
- [ ] Task 3: Implement health + metrics endpoints (`/health`, `/metrics`)
  - [ ] Axum router with `PrometheusHandle` state (matching existing service pattern)
- [ ] Task 4: Implement `StatsConfig` parsing to pass tests
- [ ] Task: Conductor - User Manual Verification 'Stats Service Scaffold' (Protocol in workflow.md)

## Phase 3: Core Scheduler & ClickHouse Query

- [ ] Task 1: Write failing tests for `fetch_running_experiments`
  <!-- files: crates/stitchd-stats-service/src/scheduler.rs -->
  - [ ] Assert only experiments with `status = running` are returned
- [ ] Task 2: Write failing tests for time-bounded ClickHouse event query
  <!-- files: crates/stitchd-stats-service/src/clickhouse_query.rs -->
  - [ ] Assert lower bound = `iteration.started_at`
  - [ ] Assert upper bound = `iteration.ended_at` when present, else `NOW()`
- [ ] Task 3: Write failing tests for results writer
  <!-- files: crates/stitchd-stats-service/src/results_writer.rs -->
  - [ ] Assert upsert to `experiment_results` with correct `experiment_id` + `iteration_id`
- [ ] Task 4: Write failing tests for post-run `stats_schedule` update
  <!-- files: crates/stitchd-stats-service/src/schedule_updater.rs -->
  - [ ] Assert `last_computed_at`, `next_run_at`, `computation_status` updated on success
- [ ] Task 5: Implement 60-minute scheduler loop (`tokio::time::interval`)
  - [ ] Iterate all running experiments; spawn task per experiment
- [ ] Task 6: Implement `fetch_running_experiments` (PostgreSQL query)
- [ ] Task 7: Implement time-bounded ClickHouse event query
- [ ] Task 8: Implement results writer (upsert to `experiment_results`)
- [ ] Task 9: Implement `stats_schedule` post-run updater
- [ ] Task: Conductor - User Manual Verification 'Core Scheduler & ClickHouse Query' (Protocol in workflow.md)

## Phase 4: Recompute Job API
<!-- depends: phase2 -->

- [ ] Task 1: Write failing tests for job service
  <!-- files: crates/stitchd-stats-service/src/job_service.rs -->
  - [ ] `create_recompute_job` returns job with status `pending` and a job_id
  - [ ] `get_job_status` returns current status from `stats_jobs`
- [ ] Task 2: Define proto for `StatsService` gRPC
  <!-- files: crates/stitchd-proto/proto/stats.proto -->
  - [ ] `TriggerRecompute(TriggerRecomputeRequest) → TriggerRecomputeResponse {job_id, status, created_at}`
  - [ ] `GetJobStatus(GetJobStatusRequest) → GetJobStatusResponse {job_id, status, started_at, completed_at, error}`
- [ ] Task 3: Implement gRPC service handler in `stitchd-stats-service`
  <!-- files: crates/stitchd-stats-service/src/grpc/service.rs -->
  - [ ] `TriggerRecompute` — inserts job row (`pending`), spawns background task, returns job_id
  - [ ] `GetJobStatus` — reads from `stats_jobs`, returns current status
- [ ] Task 4: Add gateway routes (with OpenAPI annotations)
  <!-- files: crates/stitchd-gateway/src/routes/stats.rs -->
  - [ ] `POST /experiments/{id}/recompute` → stats gRPC → 202 with `{job_id, status, created_at}`
  - [ ] `GET /jobs/{job_id}` → stats gRPC → job status response
- [ ] Task 5: Register stats gRPC client in gateway startup + config
  <!-- files: crates/stitchd-gateway/src/main.rs, crates/stitchd-gateway/src/config.rs -->
- [ ] Task: Conductor - User Manual Verification 'Recompute Job API' (Protocol in workflow.md)

## Phase 5: Results API Staleness
<!-- depends: phase3, phase4 -->

- [ ] Task 1: Write failing test asserting no ClickHouse calls from Results handler
  - [ ] Integration test with mock ClickHouse client; assert it is never invoked
- [ ] Task 2: Write failing tests for staleness computation
  - [ ] `is_stale = true` when `last_computed_at` is >60 min ago
  - [ ] `computation_status` maps correctly from `stats_schedule` enum
- [ ] Task 3: Remove inline ClickHouse query from `experimentation` Results handler
  - [ ] Results handler reads exclusively from `experiment_results` (PostgreSQL)
  - [ ] Join with `stats_schedule` to populate staleness fields
- [ ] Task 4: Update Results API response type
  - [ ] Add `computed_at`, `is_stale`, `next_run_at`, `computation_status` to response struct
  - [ ] Update OpenAPI spec annotations (`#[utoipa::path]`)
- [ ] Task: Conductor - User Manual Verification 'Results API Staleness' (Protocol in workflow.md)

## Phase 6: Infrastructure & CI
<!-- depends: phase5 -->

- [ ] Task 1: Add `stitchd-stats-service` to `docker-compose.yml`
  - [ ] `depends_on: [postgres, clickhouse]` with health checks
  - [ ] Environment variable block for postgres DSN, ClickHouse URL, interval
- [ ] Task 2: Add stats crate to CI coverage config
  - [ ] Add `stitchd-stats-service` to cargo-tarpaulin flags in CI workflow
- [ ] Task 3: Update `tech-stack.md` with `tokio::time::interval` scheduler pattern
  - [ ] Add entry to Key Dependencies / Architecture notes
- [ ] Task 4: Update mdBook docs (architecture page, API reference)
  - [ ] Add stats service to service table
  - [ ] Document `/recompute` and `/jobs/{id}` endpoints
- [ ] Task: Conductor - User Manual Verification 'Infrastructure & CI' (Protocol in workflow.md)
