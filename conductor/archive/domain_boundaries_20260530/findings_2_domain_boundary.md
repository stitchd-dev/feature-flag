# Findings 1.2: Domain-Boundary Audit
*Date: 2026-05-30*

## Crate Dependency Graph

| Crate | Dependencies (non-std, non-workspace-util) |
|-------|---------------------------------------------|
| stitchd-gateway | stitchd-proto, stitchd-core |
| stitchd-flag-service | stitchd-core, stitchd-db, stitchd-event-writer, stitchd-proto |
| stitchd-experimentation-service | stitchd-core, stitchd-db, stitchd-proto *(+ stitchd-event-writer dev-only)* |
| stitchd-analytics-service | stitchd-core, stitchd-db, stitchd-event-writer, stitchd-proto, **stitchd-stats-service** |
| stitchd-segmentation-service | stitchd-core, stitchd-db, stitchd-proto |
| stitchd-auth-service | stitchd-core, stitchd-db, stitchd-proto |
| stitchd-stats-service | stitchd-core, stitchd-db, stitchd-proto |
| stitchd-db | stitchd-core *(+ stitchd-stats-service dev-only, stitchd-event-writer dev-only)* |
| stitchd-core | (no internal workspace deps) |

---

## Boundary Violations

### [SEVERITY: HIGH] analytics-service imports stats-service business logic directly

- **File**: `crates/stitchd-analytics-service/src/grpc/metric.rs:50-54`, `crates/stitchd-analytics-service/src/grpc/service.rs:24`
- **Violating crate**: `stitchd-analytics-service`
- **Crosses into**: `stitchd-stats-service`'s domain
- **Evidence**:
  ```rust
  // metric.rs:50-54
  use stitchd_stats_service::{
      dispatch::{DispatchError, dispatch_preview_query},
      queries::{QueryBind, QueryBuildError},
      recompute_trigger::{RecomputeTrigger, trigger_recompute_for_metric},
  };
  // metric.rs:631
  let sql = stitchd_stats_service::dispatch::rewrite_placeholders_to_clickhouse(built.sql);
  // service.rs:24
  use stitchd_stats_service::recompute_trigger::RecomputeTrigger;
  ```
- **What it does**: analytics-service calls `dispatch_preview_query`, `rewrite_placeholders_to_clickhouse`, uses `RecomputeTrigger` trait and `trigger_recompute_for_metric` to fire recompute RPCs.
- **Correct pattern**: `PreviewMetric` should delegate ClickHouse dispatch to stats-service via a new `StatsService.PreviewMetric` gRPC RPC.
- **Contract-impact**: YES — requires new `StatsService.PreviewMetric(PreviewMetricRequest) returns (PreviewMetricResponse)` proto RPC.

---

### [SEVERITY: MED] stitchd-db exports stats-domain repository types (`stats_jobs`, `stats_schedule`)

- **File**: `crates/stitchd-db/src/lib.rs:41-46`, `crates/stitchd-db/src/stats_jobs.rs`, `crates/stitchd-db/src/stats_schedule.rs`
- **Violating crate**: `stitchd-db` (re-exports); `stitchd-experimentation-service` (consumer)
- **Crosses into**: `stitchd-stats-service` domain
- **Evidence**:
  ```rust
  // stitchd-db/src/lib.rs
  pub use stats_jobs::{
      CreateStatsJob, PgStatsJobRepository, StatsJobRepository, StatsJobRow, StatsJobStatus,
  };
  pub use stats_schedule::{
      ComputationStatus, PgStatsScheduleRepository, StatsScheduleRepository, StatsScheduleRow,
      UpsertStatsSchedule,
  };
  ```
  `stitchd-experimentation-service/src/main.rs:22` + `service.rs:14` import `PgStatsScheduleRepository`/`StatsScheduleRepository` to write `stats_schedule` rows directly.
- **Correct pattern**: Move `stats_jobs.rs` and `stats_schedule.rs` into `stitchd-stats-service/src/`. Experimentation-service calls a new `StatsService.InitSchedule` RPC instead.
- **Contract-impact**: YES — requires new `StatsService.InitSchedule` (or `NotifyExperimentStarted`) RPC.

---

### [SEVERITY: MED] stitchd-db exports ClickHouse experiment-query functions that are stats-service logic

- **File**: `crates/stitchd-db/src/clickhouse/experiment_queries.rs`, re-exported in `crates/stitchd-db/src/clickhouse/mod.rs`
- **Violating crate**: `stitchd-db`
- **Evidence**:
  ```rust
  pub use experiment_queries::{
      CountMetricRow, FunnelStepRow, NumericMetricRow, QueryError,
      query_count_metric, query_funnel, query_numeric_metric,
  };
  ```
  These are ClickHouse aggregate-metric computations — pure stats-service logic in the shared DB layer.
- **Correct pattern**: Move `experiment_queries.rs` into `stitchd-stats-service/src/` (or make `pub(crate)` inside `stitchd-db` until relocated).
- **Contract-impact**: NO — crate-internal reorganization only.

---

### [SEVERITY: MED] stitchd-db dev-dependency on stitchd-stats-service couples DB tests to stats domain

- **File**: `crates/stitchd-db/Cargo.toml:33`, `crates/stitchd-db/tests/event_metric_e2e.rs:66`
- **Violating crate**: `stitchd-db` (test code)
- **Evidence**:
  ```toml
  [dev-dependencies]
  stitchd-stats-service = { path = "../stitchd-stats-service" }
  ```
  ```rust
  use stitchd_stats_service::{
      dispatch::{dispatch_metric_query, rewrite_placeholders_to_clickhouse},
      queries::QueryBind,
  };
  ```
- **Correct pattern**: `event_metric_e2e.rs` belongs in `stitchd-stats-service/tests/` where the dep is legal.
- **Contract-impact**: NO — test relocation only.

---

### [SEVERITY: LOW] stitchd-core carries auth-domain logic with I/O (violates its own "no I/O" contract)

- **File**: `crates/stitchd-core/src/auth/` (`crypto.rs`, `jwt.rs`, `oidc.rs`, `saml.rs`, `totp.rs`, `types.rs`)
- **Violating crate**: `stitchd-core`
- **Evidence**: `stitchd-core` pulls in `argon2`, `aes-gcm`, `jsonwebtoken`, `totp-rs`, `openidconnect`, `reqwest`, `quick-xml`, `flate2` — I/O-capable, auth-protocol-specific libraries. `lib.rs` doc comment explicitly states *"No I/O, no database, no network."* `stitchd-core::auth` is consumed **exclusively** by `stitchd-auth-service` (confirmed by grep — no other service imports it).
- **Correct pattern**: Move `stitchd-core/src/auth/` into `stitchd-auth-service` as an internal module. Remove the seven auth-protocol deps from `stitchd-core/Cargo.toml`.
- **Contract-impact**: NO — internal reorganization.

---

## stitchd-core Ownership Assessment

`lib.rs` states: *"No I/O, no database, no network."*

**Appropriate as shared primitives:** `context`, `evaluation`, `event`, `experimentation`, `flag`, `hashing`, `id`, `metric`, `rollout`, `rule_engine`, `segment`, `tenant`, `user`, `util`, `variants` — pure domain types and rule-engine logic, no I/O.

**Misplaced:** `auth` — violates the "no I/O" contract. Contains HTTP-calling OIDC fetcher (`oidc.rs`), SAML XML parsing, argon2 password hashing, AES-GCM encryption, and TOTP generation. Used exclusively by `stitchd-auth-service`. Every workspace crate transitively builds seven heavy auth-protocol dependencies because of this.

---

## stitchd-db Ownership Assessment

**Appropriate:** `repository/` (flag, segment, experiment, metric, event-definition, context-registry, environment, project, organisation, sdk-key, user, variant, audit-logger, role), `auth/` (auth-service repos), `scylla/`, `clickhouse/eval_log.rs`, `error.rs`.

**Misplaced (stats-domain objects that should move to stitchd-stats-service):**
- `stats_jobs.rs` — `StatsJobRepository`, `PgStatsJobRepository`, `StatsJobRow`, `CreateStatsJob`, `StatsJobStatus`
- `stats_schedule.rs` — `StatsScheduleRepository`, `PgStatsScheduleRepository`, `StatsScheduleRow`, `UpsertStatsSchedule`, `ComputationStatus`
- `clickhouse/experiment_queries.rs` — `query_count_metric`, `query_funnel`, `query_numeric_metric`, result row types

---

## Summary

- **Boundary violations found: 5**
  - HIGH: 1
  - MED: 3
  - LOW: 1

- **Proposed gRPC extensions required:**
  1. `StatsService.PreviewMetric(PreviewMetricRequest) returns (PreviewMetricResponse)` — analytics-service delegates metric-preview CH dispatch to stats-service.
  2. `StatsService.InitSchedule(InitScheduleRequest) returns (InitScheduleResponse)` — experimentation-service triggers schedule initialization via RPC.

- **No proto change required for:** moving `stats_jobs`/`stats_schedule` repos and `experiment_queries` out of `stitchd-db`, relocating `event_metric_e2e` test to stats-service, extracting `stitchd-core::auth` into auth-service.
