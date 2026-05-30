# Domain-Boundary Refactor: Audit Findings
*Generated: 2026-05-30 · Track: domain_boundaries_20260530*
*Source audits: findings_1_gateway_leanness.md, findings_2_domain_boundary.md, findings_3_duplication.md, findings_4_consistency.md, findings_5_dead_code.md*

---

## Executive Summary

| Category | Count | Contract-impact |
|----------|-------|----------------|
| Gateway domain-logic leaks (→ move to service) | 14 | 5 require proto change |
| Cross-domain boundary violations | 5 | 2 require new RPCs |
| Duplicate code groups | 8 | 0 |
| Consistency issues | 16 | some require proto change |
| Dead code — DELETE-NOW | 6 | 0 |
| Dead code — PROPOSE | 4 | 1 requires migration |

**Items needing approval before execution** (contract-affecting or judgment-call):
- GL-04, GL-06, GL-07, GL-08 — gateway refactors requiring proto extensions
- BV-01, BV-02 — two new gRPC RPCs
- PROP-001 — `frozen` column removal requires migration
- PROP-002 — hash_input_spec_cutover.rs deletion (linked to DC-006)

---

## Part A: Gateway Leanness — Domain-Logic Leaks

**Classification summary:** 16 route files; 62 TRANSLATION, 4 ORCHESTRATION, 14 DOMAIN-LOGIC-LEAK instances.

### GL-01 · `validate_hash_inputs` — hash selector business rules in gateway
- **File**: `crates/stitchd-gateway/src/routes/flags.rs:157–197`
- **Owning service**: flag-service
- **Disposition**: MOVE — move to `flag-service` `MutateFlag`/`UpdateRules` gRPC handler; return `INVALID_ARGUMENT`; gateway calls gRPC → maps status to 400
- **Risk**: LOW
- **Contract-impact**: NO — validation already exists server-side (defense-in-depth per `flag_eval_unify_20260522`)

### GL-02 · `validate_variant_values` — variant value type enforcement + key uniqueness
- **File**: `crates/stitchd-gateway/src/routes/flags.rs:368–412`
- **Owning service**: flag-service
- **Disposition**: MOVE — move to flag-service `MutateFlag`; return `INVALID_ARGUMENT` on mismatch
- **Risk**: LOW
- **Contract-impact**: NO

### GL-03 · `update_variants` — boolean flag structural invariant (exactly 2 variants)
- **File**: `crates/stitchd-gateway/src/routes/flags.rs:861–879`
- **Owning service**: flag-service
- **Disposition**: MOVE — move to flag-service; return `INVALID_ARGUMENT`
- **Risk**: LOW
- **Contract-impact**: NO

### GL-04 · `update_flag` — read-modify-write to preserve `enabled` when omitted
- **File**: `crates/stitchd-gateway/src/routes/flags.rs:659–674`
- **Owning service**: flag-service
- **Disposition**: **PROPOSE** — add `partial_update: bool` or `FieldMask` to `MutateFlagRequest`; service preserves unset fields; eliminates pre-fetch round-trip
- **Risk**: MED
- **Contract-impact**: **YES** — proto change to `MutateFlagRequest`

### GL-05 · `update_variants` / `update_rules` — read-modify-write to carry flag metadata
- **File**: `crates/stitchd-gateway/src/routes/flags.rs:847–918, 1095–1145`
- **Owning service**: flag-service
- **Disposition**: **PROPOSE** — add `ReplaceVariants` and `ReplaceRules` mutation kinds to `MutateFlagRequest`; service handles metadata preservation server-side
- **Risk**: MED
- **Contract-impact**: **YES** — proto change to `MutateFlagRequest` / `MutationKind` enum

### GL-06 · `set_default_rule_distribution` — gRPC error message prefix parsing
- **File**: `crates/stitchd-gateway/src/routes/flags.rs:1326–1336`
- **Owning service**: flag-service
- **Disposition**: **PROPOSE** — flag-service should use proto `ErrorDetails` or distinct error code for distribution validation failures; gateway maps known codes → HTTP codes without string inspection
- **Risk**: LOW
- **Contract-impact**: **YES** — proto ErrorDetails extension or new status code convention

### GL-07 · `evaluate_preview` — context bundle reconstruction + opaque results_json re-parsing
- **File**: `crates/stitchd-gateway/src/routes/flags.rs:1418–1599`
- **Owning service**: flag-service
- **Disposition**: **PROPOSE** — (a) `EvaluatePreviewRequest` accepts flat UI-shape list directly; (b) `EvaluatePreviewResponse` carries structured `repeated ContextPreviewResult` proto rather than opaque JSON string
- **Risk**: HIGH (large refactor; opaque JSON is a significant surface)
- **Contract-impact**: **YES** — proto changes to both request and response of `EvaluatePreview`

### GL-08 · `validate_experiment_binding` — multi-service domain validation in gateway
- **File**: `crates/stitchd-gateway/src/routes/experiments.rs:373–458`
- **Owning service**: experimentation-service
- **Disposition**: **PROPOSE** — experimentation-service `CreateExperiment`/`UpdateExperiment` RPCs perform these validations internally via internal service-to-service calls; gateway becomes a pure translator
- **Risk**: MED
- **Contract-impact**: **YES** — experimentation-service must call flag-service and analytics-service internally

### GL-09 · `get_results` — back-compat shim synthesising context-type bundles from flat variant_results
- **File**: `crates/stitchd-gateway/src/routes/experiments.rs:908–924`
- **Owning service**: experimentation-service
- **Disposition**: MOVE — experimentation-service always populates `results_by_context_type`, defaulting context_type to `"user"` for legacy rows server-side; remove gateway shim
- **Risk**: LOW (new rows already have `results_by_context_type`; only legacy rows affected)
- **Contract-impact**: NO

### GL-10 · `forward_to_analytics` — `_test` property stamping
- **File**: `crates/stitchd-gateway/src/routes/events.rs:585–590`
- **Owning service**: analytics-service
- **Disposition**: MOVE — add `mark_test: bool` to `TrackEventsRequest` proto; analytics-service stamps the property; gateway sends `mark_test=true` for admin path
- **Risk**: LOW
- **Contract-impact**: YES (additive proto field — backward-compatible)

### GL-11 · `validate_segment_condition_expr` + `SEGMENT_FORBIDDEN_OPS` — operator allow-list
- **File**: `crates/stitchd-gateway/src/routes/segments.rs:623–672`
- **Owning service**: segmentation-service
- **Disposition**: MOVE — segmentation-service validates `condition_expr` bytes in `CreateAdminSegment`/`UpdateAdminSegment`; gateway forwards bytes without inspection
- **Risk**: LOW
- **Contract-impact**: NO

### GL-12 · `create_user` default `org_role = "org_member"` / `seed_user` default `"org_admin"`
- **File**: `crates/stitchd-gateway/src/routes/management.rs:224`, `admin.rs:147`
- **Owning service**: management-service
- **Disposition**: MOVE — management-service defaults `org_role` when absent in `CreateUserRequest`
- **Risk**: LOW
- **Contract-impact**: NO (proto field already optional; service just applies default)

### GL-13 · `create_event` — name defaulting to event_key
- **File**: `crates/stitchd-gateway/src/routes/event_admin.rs:219`
- **Owning service**: analytics-service
- **Disposition**: MOVE — analytics-service defaults name to event_key in `CreateEventDefinition` when name is absent
- **Risk**: LOW
- **Contract-impact**: NO

### GL-14 · `parse_aggregator` / `parse_goal_direction` — domain enum string validation in gateway
- **File**: `crates/stitchd-gateway/src/routes/metrics.rs:250–263`
- **Owning service**: analytics-service
- **Disposition**: MOVE (or ELIMINATE) — use proto enum fields for `aggregator` and `goal_direction`; or treat unknown strings as opaque pass-through; gateway should not enumerate valid aggregators
- **Risk**: MED
- **Contract-impact**: YES (changing string → enum in `MetricDefinition` proto is a breaking change if external clients use the REST API)

---

## Part B: Cross-Domain Boundary Violations

### BV-01 · analytics-service imports stats-service business logic directly (**HIGH**)
- **File**: `crates/stitchd-analytics-service/src/grpc/metric.rs:50-54`, `service.rs:24`
- **Violating crate**: `stitchd-analytics-service`
- **Crosses into**: `stitchd-stats-service` domain
- **Disposition**: **PROPOSE** — new `StatsService.PreviewMetric(PreviewMetricRequest) returns (PreviewMetricResponse)` RPC; analytics-service delegates CH dispatch to stats-service; `RecomputeTrigger` trait moves to `stitchd-core`
- **Risk**: MED (scoped service refactor)
- **Contract-impact**: **YES** — new gRPC RPC required

### BV-02 · stitchd-db exports stats-domain repository types; experimentation-service writes stats tables directly (**MED**)
- **Files**: `crates/stitchd-db/src/lib.rs:41-46`, `crates/stitchd-experimentation-service/src/main.rs:22`
- **Violating crates**: `stitchd-db` (re-exports), `stitchd-experimentation-service` (consumer)
- **Disposition**: **PROPOSE** — move `stats_jobs.rs` and `stats_schedule.rs` into `stitchd-stats-service/src/`; experimentation-service calls new `StatsService.InitSchedule` RPC
- **Risk**: MED
- **Contract-impact**: **YES** — new gRPC RPC required

### BV-03 · stitchd-db exports ClickHouse experiment-query functions (stats-service logic) (**MED**)
- **File**: `crates/stitchd-db/src/clickhouse/experiment_queries.rs`
- **Disposition**: MOVE — move `experiment_queries.rs` into `stitchd-stats-service/src/`; remove from `stitchd-db` exports
- **Risk**: LOW
- **Contract-impact**: NO

### BV-04 · stitchd-db dev-dependency on stitchd-stats-service (**MED**)
- **File**: `crates/stitchd-db/Cargo.toml:33`, `crates/stitchd-db/tests/event_metric_e2e.rs:66`
- **Disposition**: MOVE — `event_metric_e2e.rs` belongs in `stitchd-stats-service/tests/`
- **Risk**: LOW
- **Contract-impact**: NO

### BV-05 · stitchd-core carries auth-domain logic with I/O (violates "no I/O" contract) (**LOW**)
- **File**: `crates/stitchd-core/src/auth/` (crypto.rs, jwt.rs, oidc.rs, saml.rs, totp.rs, types.rs)
- **Disposition**: MOVE — move `stitchd-core/src/auth/` into `stitchd-auth-service` as internal module; remove seven heavy auth-protocol deps from `stitchd-core/Cargo.toml`; keep pure shared role/permission types in core
- **Risk**: LOW (used exclusively by auth-service)
- **Contract-impact**: NO

---

## Part C: Duplicate Code Groups

### DUP-001 · `hash_sdk_key` — SHA-256 hex encoder (3 identical copies)
- **Files**: `stitchd-auth-service/src/sdk_key.rs:51`, `stitchd-analytics-service/src/grpc/event_ingestion.rs:32`, `stitchd-flag-service/src/service.rs:336`
- **Canonical home**: `crates/stitchd-core/src/auth/crypto.rs` (or new `sdk_key.rs` submodule)
- **Migration effort**: LOW | **Contract-impact**: NO

### DUP-002 · `parse_uuid` — UUID string → `Status::invalid_argument` (~20 call-sites)
- **Files**: Defined in `stitchd-analytics-service/src/grpc/metric.rs:70`, `experiment_results.rs:71`; inline in 5+ modules across 5 crates
- **Canonical home**: `crates/stitchd-core/src/util/mod.rs` (gated on `"grpc"` feature)
- **Migration effort**: MED | **Contract-impact**: NO

### DUP-003 · `map_repo_err` / `repo_err_to_status` — RepositoryError → tonic::Status (7 instances)
- **Files**: auth-service (4 functions), analytics-service (2 functions), experimentation-service (1 function), plus structural-equivalent `impl From` in flag-service and segmentation-service
- **Canonical home**: `impl From<RepositoryError> for tonic::Status` in `crates/stitchd-db/src/error.rs` behind `feature = "tonic"`
- **Migration effort**: MED | **Contract-impact**: NO

### DUP-004 · Test `AssignmentRow` ClickHouse struct — experiment_assignments MV seeding (5–7 instances)
- **Files**: 4× in stats-service tests, 1× in stitchd-db tests, 1× in experimentation-service tests
- **Canonical home**: export from `crates/stitchd-db/tests/mv_experiment_assignments.rs` as `pub struct TestAssignmentRow`
- **Migration effort**: LOW | **Contract-impact**: NO

### DUP-005 · Test `EventRow` ClickHouse struct — events table with `ingested_at` (4 instances)
- **Files**: 4× in `stitchd-stats-service/tests/` (ratio_query, aggregation_query, preview_query, funnel_query)
- **Canonical home**: feature-gated `pub struct SeedEventRow` in `crates/stitchd-event-writer/src/`
- **Migration effort**: LOW | **Contract-impact**: NO

### DUP-006 · `InsertRow` analytics test helper (2 instances in same crate)
- **Files**: `stitchd-analytics-service/src/grpc/event_query.rs:467`, `metric.rs:1366`
- **Canonical home**: shared `#[cfg(test)]` module at `stitchd-analytics-service` crate root
- **Migration effort**: LOW | **Contract-impact**: NO

### DUP-007 · `validate_hash_inputs` / `validate_proto_hash_inputs` — same invariants, different input types
- **Files**: `stitchd-gateway/src/routes/flags.rs:157`, `stitchd-flag-service/src/mapping.rs:147`
- **Canonical home**: `stitchd-core::evaluation::types::HashInputSpec` or free function on common intermediate
- **Migration effort**: MED | **Contract-impact**: NO

### DUP-008 · `GoalDirection` string ↔ enum conversions split across two crates
- **Files**: `stitchd-analytics-service/src/grpc/metric.rs`, `stitchd-gateway/src/routes/metrics.rs`
- **Canonical home**: `impl fmt::Display + impl FromStr` for `GoalDirection` in `stitchd-core/src/metric/`
- **Migration effort**: MED | **Contract-impact**: NO

---

## Part D: Consistency Issues

### Canonical Patterns Agreed (for `patterns.md`)

| Concern | Canonical Pattern |
|---------|-------------------|
| Error: `InvalidState` | `Status::failed_precondition(reason)` |
| Error: `ForeignKeyViolation` | `Status::invalid_argument("referenced entity does not exist: {constraint}")` |
| Error: `UniqueViolation` | `Status::already_exists(msg)` (one approved exception: revoke-last-SDK-key → failed_precondition) |
| Error: version conflict message | `"version conflict: expected {e}, actual {a}"` |
| Error architecture | Typed `{ServiceName}Error` enum in `error.rs` per crate with `impl From<RepositoryError>` and `impl From<…> for Status` |
| Pagination: REST params | `page` + `per_page` (1-based) via shared `PaginationParams` |
| Pagination: REST envelope | `PaginatedResponse<T>` → `{items, total, page, per_page}` |
| Timestamp fields | `string created_at` RFC 3339 UTC (experiments/segments should migrate from `int64 *_ms`) |
| Version field type | `uint64 version` (analytics should migrate from `int64`) |
| REST env scope param name | `environment_id` (path or query; standardize away from `env_id`) |
| Delete RPC response | Empty response `{}` — gateway returns 204 anyway |
| Get RPC response | Return entity message directly (no wrapper) |
| Admin CRUD RPC naming | Separate `Create`/`Update`/`Delete`; `Mutate*` reserved for SDK sync path |
| Validation location | Structural (empty, UUID) at service layer; domain business-rule without DB at gateway |

### Issues Requiring Proto Changes (needing individual approval)

| ID | Description | Risk |
|----|-------------|------|
| INCON-N001 | Migrate `int64 *_ms` → `string created_at` RFC 3339 in experiments + segments protos | HIGH — breaking for clients that parse epoch |
| INCON-N003 | Migrate `int64 version` → `uint64 version` in analytics proto | MED |
| INCON-N004 | Extract `Mutate*` from segments admin proto → separate Create/Update/Delete | MED |
| INCON-R001 | Change `DeleteExperiment` to return empty response | MED |
| INCON-R002 | Unwrap `GetAuthProvider` + `GetOrg` response envelopes | LOW |

---

## Part E: Dead Code

### DELETE-NOW (safe to remove immediately)

| ID | File | Description | Risk |
|----|------|-------------|------|
| DC-001 | `crates/stitchd-flag-service/src/eval_log_writer.rs:33` | `spawn_eval_log_write` function — never called | NONE |
| DC-002 | `crates/stitchd-core/src/auth/jwt.rs:12,123` | `UserStatus` unused import + `_use_user_status` suppressor | NONE |
| DC-003 | `crates/stitchd-auth-service/src/auth_provider.rs:428–433` | `MockProviderRepo::with` — uncalled test helper | NONE |
| DC-004 | `crates/stitchd-gateway/src/tests/helpers.rs:60–62` + `flags.rs:1846–1848` | `make_stub_state_with_flag` + `_use_with_flag` suppressor | NONE |
| DC-005 | `crates/stitchd-core/src/hashing.rs:10–19` | `compute_hash_percentage` — retired `f64` hash function | NONE |
| DC-006 | `crates/xtask/src/verify_hash_cutover.rs` (370 lines) + `main.rs:18,25,35` | One-time migration verifier for completed `flag_eval_unify_20260522` | NONE |

### PROPOSE (judgment-call removals — need approval)

| ID | File | Description | Risk |
|----|------|-------------|------|
| PROP-001 | `crates/stitchd-db/src/repository/pg/experiment.rs:626–639` | `feature_flag_rules.frozen` write path + column (replaced by whole-flag lock; requires migration + 10+ test updates) | LOW |
| PROP-002 | `crates/stitchd-db/tests/hash_input_spec_cutover.rs` (677 lines) | Migration test corpus for completed `flag_eval_unify_20260522` — recommend deleting alongside DC-006 | LOW |
| PROP-003 | `crates/stitchd-flag-service/src/eval_log_writer.rs:25` | `EvalContextRow` type alias — dead since DC-001 (must be removed together) | NONE |
| PROP-004 | `crates/stitchd-db/src/auth/users.rs:480` | Misleading `#[allow(dead_code)]` on live `seed_org` function — annotation removal only | NONE |

---

## Items Requiring Explicit Approval Before Execution

The following items affect the REST or gRPC contract surface and require explicit approval:

### Contract-Affecting Gateway Refactors

| ID | Description | Proto change needed |
|----|-------------|---------------------|
| GL-04 | `update_flag` partial-update support | Add `partial_update`/`FieldMask` to `MutateFlagRequest` |
| GL-05 | `ReplaceVariants` / `ReplaceRules` mutation kinds | Add to `MutateFlagRequest` / `MutationKind` enum |
| GL-06 | Distribution validation error signaling | Use proto `ErrorDetails` or new status code |
| GL-07 | `EvaluatePreview` flat input + structured response | Proto changes to `EvaluatePreviewRequest` + `EvaluatePreviewResponse` |
| GL-08 | Experiment binding validation → experimentation-service | Requires internal service-to-service calls |
| GL-10 | `mark_test` flag on `TrackEventsRequest` | Add `mark_test: bool` field (backward-compatible) |
| GL-14 | `aggregator`/`goal_direction` as proto enum | Breaking for REST clients if `MetricDefinition` strings change |

### New gRPC RPCs

| ID | RPC | Reason |
|----|-----|--------|
| BV-01 | `StatsService.PreviewMetric(PreviewMetricRequest) returns (PreviewMetricResponse)` | analytics-service must not import stats-service crate |
| BV-02 | `StatsService.InitSchedule(InitScheduleRequest) returns (InitScheduleResponse)` | experimentation-service must not write stats tables directly |

### Schema/Migration Changes

| ID | Description |
|----|-------------|
| PROP-001 | Drop `feature_flag_rules.frozen` column + remove write path (requires migration) |

### Consistency Proto Migrations (breaking if adopted)

| ID | Description |
|----|-------------|
| INCON-N001 | `int64 *_ms` → `string` RFC 3339 in experiments + segments protos |
| INCON-N003 | `int64 version` → `uint64 version` in analytics proto |

---

## Conventions Seed for `patterns.md`

```markdown
## Domain-Boundary Refactor Conventions (from domain_boundaries_20260530, 2026-05-30)

### Gateway Responsibility
Gateway routes contain ONLY: REST↔proto translation, cross-cutting concerns (auth/rate-limit/quota), and multi-service orchestration. Zero domain logic — no validation beyond shape, no business rules, no inline evaluation/hashing/query-building.

### Cross-Domain Access
All cross-domain access MUST go through gRPC. A service crate MUST NOT import another service's crate at runtime (only dev-deps for shared test utilities, explicitly documented).

### stitchd-core Contract
stitchd-core is shared primitives only — pure domain types, rule-engine logic. NO I/O, NO database, NO network. Auth-protocol crypto/I/O does NOT belong in core.

### Error Mapping
Each service has a typed `{ServiceName}Error` enum in `error.rs` with `impl From<RepositoryError>` and `impl From<{ServiceName}Error> for tonic::Status`. Map: NotFound→not_found, VersionConflict→aborted, UniqueViolation→already_exists, ForeignKeyViolation→invalid_argument, InvalidState→failed_precondition, Database/Unexpected→internal.

### Pagination
REST layer: shared `PaginationParams` (page + per_page, 1-based) → `PaginatedResponse<T>` ({items, total, page, per_page}). Internal CH-backed protos: offset+limit is acceptable; gateway translates.

### Timestamp Fields
Use `string created_at` RFC 3339 UTC in all new/migrated protos. Do NOT use `int64 *_ms` epoch.

### Version Fields
Use `uint64 version` for all optimistic-lock version fields. Never `int64`.

### CRUD RPC Naming
Admin CRUD: separate `Create`/`Update`/`Delete` RPCs. `Mutate*` pattern reserved ONLY for SDK synchronization path (flags, segments SDK RPC) where a single RPC must support multiple mutation kinds in one roundtrip.
```
