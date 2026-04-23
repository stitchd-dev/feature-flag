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

## Phase 2: Stats Service Scaffold [checkpoint: a0c5e45]

- [x] Task 1: Write failing tests for `StatsConfig` env-var parsing a0c5e45
  - [x] Test postgres DSN, ClickHouse URL, scheduler interval fields load correctly
- [x] Task 2: Create `stitchd-stats-service` crate in workspace a0c5e45
  - [x] `Cargo.toml` with sqlx, clickhouse, tokio, axum, metrics dependencies
  - [x] Add crate to workspace `Cargo.toml`
  - [x] `src/main.rs` with graceful shutdown (`tokio::select!` over SIGTERM + ctrl_c)
  - [x] `src/config.rs` env-based config struct
- [x] Task 3: Implement health + metrics endpoints (`/health`, `/metrics`) a0c5e45
  - [x] Axum router with `PrometheusHandle` state (matching existing service pattern)
- [x] Task 4: Implement `StatsConfig` parsing to pass tests a0c5e45
- [x] Task: Conductor - User Manual Verification 'Stats Service Scaffold' (Protocol in workflow.md)

## Phase 3: Core Scheduler & ClickHouse Query [checkpoint: aba205d]

- [x] Task 1: Write failing tests for `fetch_running_experiments`
  <!-- files: crates/stitchd-stats-service/src/scheduler.rs -->
  - [x] Assert only experiments with `status = running` are returned
- [x] Task 2: Write failing tests for time-bounded ClickHouse event query
  <!-- files: crates/stitchd-stats-service/src/clickhouse_query.rs -->
  - [x] Assert lower bound = `iteration.started_at`
  - [x] Assert upper bound = `iteration.ended_at` when present, else `NOW()`
- [x] Task 3: Write failing tests for results writer
  <!-- files: crates/stitchd-stats-service/src/results_writer.rs -->
  - [x] Assert upsert to `experiment_results` with correct `experiment_id` + `iteration_id`
- [x] Task 4: Write failing tests for post-run `stats_schedule` update
  <!-- files: crates/stitchd-stats-service/src/schedule_updater.rs -->
  - [x] Assert `last_computed_at`, `next_run_at`, `computation_status` updated on success
- [x] Task 5: Implement 60-minute scheduler loop (`tokio::time::interval`)
  - [x] Iterate all running experiments; spawn task per experiment
- [x] Task 6: Implement `fetch_running_experiments` (PostgreSQL query)
- [x] Task 7: Implement time-bounded ClickHouse event query
- [x] Task 8: Implement results writer (upsert to `experiment_results`)
- [x] Task 9: Implement `stats_schedule` post-run updater
- [x] Task: Conductor - User Manual Verification 'Core Scheduler & ClickHouse Query' (Protocol in workflow.md)

## Phase 4: Recompute Job API [checkpoint: ed6cc2a]

- [x] Task 1: Write failing tests for job service ed6cc2a
  <!-- files: crates/stitchd-stats-service/src/job_service.rs -->
  - [x] `create_recompute_job` returns job with status `pending` and a job_id
  - [x] `get_job_status` returns current status from `stats_jobs`
- [x] Task 2: Define proto for `StatsService` gRPC ed6cc2a
  <!-- files: proto/stats/v1/stats_service.proto -->
  - [x] `TriggerRecompute(TriggerRecomputeRequest) → TriggerRecomputeResponse {job_id, status, created_at}`
  - [x] `GetJobStatus(GetJobStatusRequest) → GetJobStatusResponse {job_id, status, started_at, completed_at, error}`
- [x] Task 3: Implement gRPC service handler in `stitchd-stats-service` ed6cc2a
  <!-- files: crates/stitchd-stats-service/src/grpc/service.rs -->
  - [x] `TriggerRecompute` — inserts job row (`pending`), spawns background task, returns job_id
  - [x] `GetJobStatus` — reads from `stats_jobs`, returns current status
- [x] Task 4: Add gateway routes (with OpenAPI annotations) ed6cc2a
  <!-- files: crates/stitchd-gateway/src/routes/stats.rs -->
  - [x] `POST /experiments/{id}/recompute` → stats gRPC → 202 with `{job_id, status, created_at}`
  - [x] `GET /jobs/{job_id}` → stats gRPC → job status response
- [x] Task 5: Register stats gRPC client in gateway startup + config ed6cc2a
  <!-- files: crates/stitchd-gateway/src/main.rs -->
- [x] Task: Conductor - User Manual Verification 'Recompute Job API' (Protocol in workflow.md)

## Phase 5: Results API Staleness [checkpoint: 6b907ef]

- [x] Task 1: Write failing test asserting no ClickHouse calls from Results handler 6b907ef
  - [x] Integration test with mock results repo; assert results come from PostgreSQL, not ClickHouse
- [x] Task 2: Write failing tests for staleness computation 6b907ef
  - [x] `is_stale = true` when `last_computed_at` is >60 min ago
  - [x] `computation_status` maps correctly from `stats_schedule` enum
- [x] Task 3: ExperimentationServiceImpl reads stats_schedule for staleness 6b907ef
  - [x] Inject StatsScheduleRepository; get_results fetches schedule row
  - [x] Populate is_stale, next_run_at_ms, computation_status
- [x] Task 4: Update Results API response type 6b907ef
  - [x] Add `is_stale`, `next_run_at_ms`, `computation_status` to ExperimentResults proto + gateway JSON
- [x] Task: Conductor - User Manual Verification 'Results API Staleness' (Protocol in workflow.md)

## Phase 6: Infrastructure & CI [checkpoint: eef1738]

- [x] Task 1: Add `stitchd-stats-service` to `docker-compose.yml` eef1738
  - [x] `depends_on: [postgres, clickhouse]` with health checks
  - [x] Environment variable block for postgres DSN, ClickHouse URL, interval
- [x] Task 2: Add stats crate to CI coverage config eef1738
  - [x] Add `stitchd-stats-service` to cargo-tarpaulin flags in coverage-full.yml + codecov.yml
- [x] Task 3: Update `tech-stack.md` with `tokio::time::interval` scheduler pattern eef1738
- [x] Task 4: Update mdBook docs (architecture page) eef1738
  - [x] Add stats service to service table in architecture/README.md
- [x] Task: Conductor - User Manual Verification 'Infrastructure & CI' (Protocol in workflow.md)
