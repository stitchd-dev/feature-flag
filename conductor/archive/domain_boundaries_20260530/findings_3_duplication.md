# Findings 1.3: Duplication Audit
*Date: 2026-05-30*

## Summary
- Total duplicate groups found: 8
- ClickHouse row structs: 3 duplicated groups (test seeding + inline test write helpers)
- Proto mappers: 1 duplicated (Metric domain↔proto split with shared GoalDirection parsing)
- Error mapping patterns: 1 duplicated group (RepositoryError → tonic::Status, 7 instances)
- Validation helpers: 1 duplicated (hash_inputs validation logic)
- ID/UUID conversions: 1 duplicated (parse_uuid → Status::invalid_argument)
- Hash/query helpers: 1 duplicated (hash_sdk_key SHA-256 helper)

---

## Duplicate Group Details

### DUP-001: `hash_sdk_key` — SHA-256 hex encoder for SDK keys
- **Category**: Hash helper
- **Instances**:
  - `crates/stitchd-auth-service/src/sdk_key.rs:51` — `pub fn hash_sdk_key(raw: &str) -> String`
  - `crates/stitchd-analytics-service/src/grpc/event_ingestion.rs:32` — `pub fn hash_sdk_key(raw: &str) -> String`
  - `crates/stitchd-flag-service/src/service.rs:336` — `pub(crate) fn hash_sdk_key(raw: &str) -> String`
- **Body**: All three are functionally identical — SHA-256 of raw bytes, then `hex::encode`. The auth-service variant uses the one-shot `Sha256::digest(...)` form; the other two use `new + update + finalize`. Output is identical; all three have determinism tests with the same assertions.
- **Canonical home proposal**: `crates/stitchd-core/src/auth/crypto.rs` — already owns password hashing with the same SHA-256/hex pattern; or a new `crates/stitchd-core/src/sdk_key.rs`. All three downstream crates already depend on `stitchd-core`.
- **Migration effort**: LOW
- **Contract-impact**: NO

---

### DUP-002: `parse_uuid` — UUID string → `Status::invalid_argument` helper
- **Category**: ID/UUID conversion
- **Instances**:
  - `crates/stitchd-analytics-service/src/grpc/metric.rs:70` — `fn parse_uuid(s: &str, field: &str) -> Result<Uuid, Status>`
  - `crates/stitchd-analytics-service/src/grpc/experiment_results.rs:71` — `fn parse_uuid(s: &str, field: &str) -> Result<uuid::Uuid, Status>`
  - Inline (~15 call-sites): `crates/stitchd-analytics-service/src/grpc/context_intel.rs:24`, `context_registry.rs:114`, `event_definition.rs:46`, `eval_stats.rs:68`, `event_query.rs:189`
  - Inline: `crates/stitchd-experimentation-service/src/service.rs` (~12 call-sites)
  - Inline: `crates/stitchd-stats-service/src/grpc/service.rs:54,82,109,113`
  - Inline: `crates/stitchd-flag-service/src/service.rs:319,330`
  - Inline: `crates/stitchd-segmentation-service/src/grpc/sdk_backend.rs:45`
- **Body**: Every instance reduces to `uuid::Uuid::parse_str(s).map_err(|_| Status::invalid_argument(format!("invalid {field}: {s}")))`.
- **Canonical home proposal**: `crates/stitchd-core/src/util/mod.rs` — add `pub fn parse_uuid(s: &str, field: &str) -> Result<Uuid, tonic::Status>`. Requires gating on a `"grpc"` feature or a new micro-crate.
- **Migration effort**: MED — ~20 call-sites across 7 crates, each is a one-liner swap.
- **Contract-impact**: NO

---

### DUP-003: `map_repo_err` / `repo_err_to_status` — RepositoryError → tonic::Status
- **Category**: Error mapping
- **Instances**:
  - `crates/stitchd-auth-service/src/auth_provider.rs:75` — `fn map_repo_err` (4-arm)
  - `crates/stitchd-auth-service/src/oidc_login.rs:129` — `fn map_repo_err` (2-arm)
  - `crates/stitchd-auth-service/src/saml_login.rs:143` — `fn map_repo_err` (2-arm)
  - `crates/stitchd-auth-service/src/management.rs:112` — `fn map_repo_err` (4-arm)
  - `crates/stitchd-analytics-service/src/grpc/event_definition.rs:135` — `fn map_repo_err` (4-arm with VersionConflict)
  - `crates/stitchd-analytics-service/src/grpc/metric.rs:278` — `fn repo_err_to_status(err, ctx: &str)` (full 7-arm with context-string prefix)
  - `crates/stitchd-experimentation-service/src/service.rs:961` — `fn repo_err_to_status` (full 7-arm)
  - Plus structural-equivalent `impl From<RepositoryError>` in `stitchd-flag-service/src/error.rs:62` and `stitchd-segmentation-service/src/error.rs:38`
- **Body**: All implement the same `match RepositoryError { NotFound → not_found, VersionConflict → aborted, UniqueViolation → already_exists, ForeignKeyViolation → invalid_argument/failed_precondition, InvalidState → failed_precondition, Database → internal, Unexpected → internal }` mapping.
- **Canonical home proposal**: Add `impl From<RepositoryError> for tonic::Status` in `crates/stitchd-db/src/error.rs` behind a `tonic` optional feature.
- **Migration effort**: MED — 7 function definitions, ~40 call-sites.
- **Contract-impact**: NO

---

### DUP-004: Test `AssignmentRow` ClickHouse struct (experiment_assignments MV seeding)
- **Category**: ClickHouse row struct (test-seeding)
- **Instances**:
  - `crates/stitchd-stats-service/tests/ratio_query.rs:28`
  - `crates/stitchd-stats-service/tests/aggregation_query.rs:33`
  - `crates/stitchd-stats-service/tests/preview_query.rs:24`
  - `crates/stitchd-stats-service/tests/funnel_query.rs:20`
  - `crates/stitchd-db/tests/event_metric_e2e.rs:429`
  - `crates/stitchd-experimentation-service/tests/experiment_lifecycle_e2e.rs:92` (8-field partial)
  - `crates/stitchd-db/tests/mv_experiment_assignments.rs:56` — `AssignmentRowRead` (5-field read subset)
- **Body**: Five full definitions are identical 11-field structs with clickhouse serde annotations.
- **Canonical home proposal**: Export from `crates/stitchd-db/tests/mv_experiment_assignments.rs` as `pub struct TestAssignmentRow` in a shared module under `#[cfg(any(test, feature = "test-fixtures"))]`.
- **Migration effort**: LOW — test-only, no production impact.
- **Contract-impact**: NO

---

### DUP-005: Test `EventRow` ClickHouse struct (events table with explicit `ingested_at`)
- **Category**: ClickHouse row struct (test-seeding)
- **Instances**:
  - `crates/stitchd-stats-service/tests/ratio_query.rs:49`
  - `crates/stitchd-stats-service/tests/aggregation_query.rs:54`
  - `crates/stitchd-stats-service/tests/preview_query.rs:45`
  - `crates/stitchd-stats-service/tests/funnel_query.rs:41`
- **Body**: All four are byte-for-byte identical 10-field structs. Differs from production `EventRow` in `stitchd-event-writer` only in the presence of `ingested_at`.
- **Canonical home proposal**: Add feature-gated `pub struct SeedEventRow` to `crates/stitchd-event-writer/src/writer.rs` or a `test_support` submodule.
- **Migration effort**: LOW — 4 identical definitions, test-only.
- **Contract-impact**: NO

---

### DUP-006: `InsertRow` inline local struct for `events` table (analytics-service test helpers)
- **Category**: ClickHouse row struct (test inline)
- **Instances**:
  - `crates/stitchd-analytics-service/src/grpc/event_query.rs:467` — `struct InsertRow<'a>` in `#[cfg(test)]`
  - `crates/stitchd-analytics-service/src/grpc/metric.rs:1366` — `struct InsertRow<'a>` in `#[cfg(test)]`
- **Body**: Identical 9-field lifetime-parameterized structs writing to the `events` table.
- **Canonical home proposal**: Consolidate into a test-support struct in `stitchd-event-writer`.
- **Migration effort**: LOW — 2 call-sites, both in same crate.
- **Contract-impact**: NO

---

### DUP-007: `validate_hash_inputs` / `validate_proto_hash_inputs` — hash selector dedup + empty-string guard
- **Category**: Validation helper
- **Instances**:
  - `crates/stitchd-gateway/src/routes/flags.rs:157` — `pub fn validate_hash_inputs(selectors: &[HashSelectorJson]) -> Result<(), String>`
  - `crates/stitchd-flag-service/src/mapping.rs:147` — `pub fn validate_proto_hash_inputs(selectors: &[ProtoHashSelector]) -> Result<(), String>`
- **Body**: Both enforce the same three invariants with identical error message strings. Different input types (JSON vs proto) but identical logic and error text.
- **Canonical home proposal**: Extract invariant logic into `stitchd-core::evaluation::types::HashInputSpec` or a free function accepting `&[(context_type: &str, field: &str)]`.
- **Migration effort**: MED — requires shared intermediate representation.
- **Contract-impact**: NO

---

### DUP-008: Metric domain↔proto mapper and `GoalDirection` string helpers split across two crates
- **Category**: Proto mapper
- **Instances**:
  - `crates/stitchd-analytics-service/src/grpc/metric.rs:256` — `fn domain_to_proto(m: &DomainMetric) -> ProtoMetric`
  - `crates/stitchd-gateway/src/routes/metrics.rs:329` — `fn proto_to_domain(p: ProtoMetric) -> Result<MetricDefinition, GatewayError>`
  - `crates/stitchd-analytics-service/src/grpc/metric.rs` — `fn goal_direction_to_str` (3-variant match)
  - `crates/stitchd-gateway/src/routes/metrics.rs:286` — `fn parse_goal_direction` (inverse 3-variant match)
- **Body**: The analytics-service serializes `GoalDirection` as `"increase"/"decrease"/"neutral"` strings and the gateway parses those same strings back — both sides maintain their own 3-arm match functions.
- **Canonical home proposal**: Move `GoalDirection` string ↔ enum conversions to `crates/stitchd-core/src/metric/` (`impl fmt::Display` + `impl FromStr`).
- **Migration effort**: MED — two crates, involves both analytics and gateway round-trip logic.
- **Contract-impact**: NO

---

## Quick-Win Consolidations (lowest risk, highest duplication)

1. **DUP-001 `hash_sdk_key`** (3 instances, LOW effort) — Move to `stitchd-core::auth::crypto`. Zero logic risk: identical algorithm confirmed by independent test assertions in all three crates. Every downstream crate already depends on `stitchd-core`.

2. **DUP-004 Test `AssignmentRow`** (5–7 instances, LOW effort) — Export from `stitchd-db/tests/mv_experiment_assignments.rs` as `pub struct TestAssignmentRow`.

3. **DUP-005 Test `EventRow`** (4 instances, LOW effort) — Add feature-gated `SeedEventRow` to `stitchd-event-writer`.

4. **DUP-006 Analytics `InsertRow`** (2 instances, LOW effort) — Hoist to a single shared `#[cfg(test)]` module at the crate root.

5. **DUP-003 `repo_err_to_status`** (7 instances, MED effort, highest blast radius) — Add `impl From<RepositoryError> for tonic::Status` in `stitchd-db/src/error.rs` behind `feature = "tonic"`.
