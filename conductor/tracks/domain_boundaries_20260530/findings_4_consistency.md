# Findings 1.4: Consistency Audit
*Date: 2026-05-30*

## 1. Error Handling Inconsistencies

### INCON-E001: `RepositoryError::InvalidState` maps to different Status codes across services
- **Dimension**: Error handling
- **Divergent instances**:
  - `crates/stitchd-flag-service/src/error.rs`: `InvalidState` falls through to `Self::Internal` (catch-all)
  - `crates/stitchd-segmentation-service/src/error.rs`: same — falls through to `Self::Internal`
  - `crates/stitchd-experimentation-service/src/service.rs:975`: `InvalidState { reason } => Status::failed_precondition(reason)`
  - `crates/stitchd-analytics-service/src/grpc/metric.rs:293`: `InvalidState { reason } => Status::failed_precondition(…)`
  - `crates/stitchd-auth-service/src/management.rs:118`: `InvalidState { reason } => Status::permission_denied(reason)` (unique — uses PERMISSION_DENIED)
  - `crates/stitchd-auth-service/src/auth_provider.rs`: `InvalidState { reason } => Status::permission_denied(reason)`
- **Proposed canonical pattern**: `Status::failed_precondition(reason)` — `InvalidState` models a business pre-condition violation. Auth service's `permission_denied` mapping is the outlier.

### INCON-E002: `RepositoryError::ForeignKeyViolation` maps to different Status codes
- **Dimension**: Error handling
- **Divergent instances**:
  - `crates/stitchd-flag-service/src/error.rs:77–79`: `ForeignKeyViolation → Status::invalid_argument("referenced entity does not exist: {constraint}")`
  - `crates/stitchd-experimentation-service/src/service.rs:972–973`: `ForeignKeyViolation → Status::invalid_argument(…)` (consistent with flag service)
  - `crates/stitchd-analytics-service/src/grpc/metric.rs:289–291`: `ForeignKeyViolation → Status::failed_precondition("{ctx}: foreign key violation on `{constraint}`")` (different code!)
- **Proposed canonical pattern**: `Status::invalid_argument(…)` — FK violation means the caller referenced an entity that doesn't exist, which is an input problem.

### INCON-E003: `RepositoryError::UniqueViolation` mapping has one intentional exception
- **Dimension**: Error handling
- **Divergent instances**:
  - `crates/stitchd-flag-service/src/error.rs:92`: `UniqueViolation → Status::already_exists(…)` ✓
  - `crates/stitchd-segmentation-service/src/error.rs:58–59`: `UniqueViolation → Status::already_exists(…)` ✓
  - `crates/stitchd-experimentation-service/src/service.rs:969–970`: `UniqueViolation → Status::already_exists(…)` ✓
  - `crates/stitchd-analytics-service/src/grpc/metric.rs:286–288`: `UniqueViolation → Status::already_exists(…)` ✓
  - `crates/stitchd-auth-service/src/management.rs:~544` (revoke SDK key path): `UniqueViolation → Status::failed_precondition("cannot revoke the last active SDK key")` (intentional domain exception)
- **Notes**: The revoke-SDK-key special case is intentional and should be documented as the only approved exception.

### INCON-E004: Experimentation, analytics, and auth services lack typed service-level error enums
- **Dimension**: Error handling / architecture
- **Divergent instances**:
  - `crates/stitchd-flag-service/src/error.rs`: `FlagServiceError` enum with `impl From<RepositoryError>` and `impl From<FlagServiceError> for Status` (well-structured)
  - `crates/stitchd-segmentation-service/src/error.rs`: `SegmentationServiceError` enum, same pattern
  - `crates/stitchd-experimentation-service/src/service.rs:961`: No typed enum — inline `repo_err_to_status()` helper
  - `crates/stitchd-analytics-service/src/grpc/`: No typed enum — each sub-module has its own local `map_repo_err()` helper
  - `crates/stitchd-auth-service/src/management.rs`: `map_repo_err()` local helper, no typed enum
- **Proposed canonical pattern**: Typed `{ServiceName}Error` enum in `error.rs` per crate.

### INCON-E005: Version conflict error message format is inconsistent
- **Dimension**: Error handling
- **Divergent instances**:
  - `crates/stitchd-flag-service/src/error.rs:90`: `"version conflict: expected {expected}, actual {actual}"` ✓
  - `crates/stitchd-segmentation-service/src/error.rs:55–56`: matches ✓
  - `crates/stitchd-experimentation-service/src/service.rs:966–967`: matches ✓
  - `crates/stitchd-analytics-service/src/grpc/event_definition.rs:141–143`: `"event_definition: version conflict (expected={expected}, actual={actual})"` (prefixed, parentheses)
  - `crates/stitchd-analytics-service/src/grpc/metric.rs:283–285`: `"{ctx}: version conflict — expected={expected}, actual={actual}"` (dash separator, context prefix)
- **Proposed canonical pattern**: `"version conflict: expected {expected}, actual {actual}"`.

---

## 2. Pagination Inconsistencies

### INCON-P001: Two pagination styles coexist — `page`+`per_page` vs `offset`+`limit`
- **Dimension**: Pagination
- **`page`+`per_page` (majority)**:
  - `proto/flags/v1/flag_service.proto`: `ListFlagsRequest` — `uint32 page` + `uint32 per_page`
  - `proto/experiments/v1/experimentation_service.proto`: `ListExperimentsRequest` — same
  - `proto/segments/v1/segmentation_service.proto`: `ListAdminSegmentsRequest` — same
  - `proto/management/v1/management_service.proto`: `ListSdkKeysRequest`, `ListOrgUsersRequest` — same
- **`offset`+`limit` (minority)**:
  - `proto/experiments/v1/experimentation_service.proto`: `ListIterationsRequest`, `ListExposuresRequest`
  - `proto/analytics/v1/analytics.proto`: `ListEventDefinitionsRequest`, `ListMetricsRequest`
  - Gateway `metrics.rs`: exposes `?offset=&limit=` query params directly
  - Gateway `event_admin.rs`: translates `page`/`per_page` query params to `offset`/`limit` proto fields internally
- **Proposed canonical pattern**: `page` + `per_page` (1-based) at the REST layer via shared `PaginationParams`. `offset`+`limit` is acceptable for internal ClickHouse-backed list RPCs; the gateway translates.

### INCON-P002: REST response envelope shape differs between list endpoints
- **Dimension**: Pagination
- **Divergent instances**:
  - `flags.rs`, `experiments.rs`, `segments.rs`, `management.rs`: shared `PaginatedResponse<T>` → `{items, total, page, per_page}`
  - `event_admin.rs:103–108`: custom `ListEventsJson` with `{items, total, page, per_page}` — same shape but not using shared type
  - `metrics.rs:187–192`: custom `ListMetricsResponseJson` with `{items, total, offset, limit}` — **different fields**
- **Proposed canonical pattern**: `PaginatedResponse<T>` everywhere.

---

## 3. Validation Inconsistencies

### INCON-V001: `hash_inputs` validation runs in both gateway AND service layers (intentional but undocumented)
- **Locations**:
  - `crates/stitchd-gateway/src/routes/flags.rs:157–197` (`validate_hash_inputs`) — validates before gRPC call
  - `crates/stitchd-flag-service/src/service.rs:822–833` — re-validates via `mapping::validate_proto_hash_inputs` with "server-side defense-in-depth" comment
- **Notes**: Intentional per `flag_eval_unify_20260522` Phase 4. No explicit policy exists for other fields.
- **Proposed canonical pattern**: Document: gateway validates for fast-fail UX; service validates as defense-in-depth for security-sensitive fields only.

### INCON-V002: Input validation is performed in inconsistent locations across services
- **Gateway-layer (domain business-rule validation)**:
  - `experiments.rs:361–430` (`validate_experiment_binding`) — complex binding invariant before gRPC call
  - `flags.rs:368–415` (`validate_variant_values`) — variant value type checking in gateway
  - `segments.rs:627–668` (`validate_segment_condition_expr`) — condition expression validation in gateway
- **Service-layer (structural validation)**:
  - `analytics-service/src/grpc/event_definition.rs:155–159` — empty key/name checks in service handler
  - `analytics-service/src/grpc/metric.rs:369,601` — empty field checks in service handler
  - `experimentation-service/src/service.rs:248` — `"experiment field is required"` check in service
- **Proposed canonical pattern**: Lightweight structural validation (empty fields, UUID parse) at the service layer. Domain/business-rule validation requiring no DB round-trip is appropriate in the gateway. Document this split explicitly.

---

## 4. Naming Convention Inconsistencies

### INCON-N001: Timestamp field types differ — `int64 *_ms` vs `string` RFC 3339
- **`int64 millisecond epoch`**:
  - `proto/experiments/v1/experimentation_service.proto`: `int64 created_at_ms`, `int64 updated_at_ms`, `int64 started_at_ms`, `int64 ended_at_ms`, `int64 computed_at_ms`
  - `proto/segments/v1/segmentation_service.proto:79–80` on `AdminSegment`: `int64 created_at_ms`, `int64 updated_at_ms`
- **`string RFC 3339`**:
  - `proto/management/v1/management_service.proto`: `string created_at` on various responses
  - `proto/analytics/v1/analytics.proto`: `string created_at`, `string updated_at`, etc.
  - `proto/auth/v1/management.proto`: `string created_at`, `string updated_at`
  - Mixed within same file: `proto/experiments/v1/experimentation_service.proto:244` — `string assigned_at` (RFC 3339, in `ExposureRow`)
- **Proposed canonical pattern**: `string created_at` RFC 3339 UTC — self-documenting, human-readable.

### INCON-N002: Environment scope expressed as path param (`environment_id`) vs query param (`env_id`)
- **Path param (long name)**:
  - `GET /v1/environments/{environment_id}/experiments`
  - `GET /v1/environments/{environment_id}/segments` (admin list)
- **Query param (short name `env_id`)**:
  - `GET /v1/metrics?env_id=<uuid>` — `metrics.rs:60`
  - `GET /v1/events?env_id=<uuid>` — `event_admin.rs:34`
  - `GET /v1/eval-stats?env_id=<uuid>` — `eval_stats.rs`
- **Proposed canonical pattern**: Path param `environment_id` for resource-scoped endpoints. For query-param analytics endpoints, standardize to `environment_id` instead of `env_id`.

### INCON-N003: Optimistic-lock version field type — `uint64` vs `int64`
- **`uint64`**: `proto/flags/v1/flag_service.proto`, `proto/segments/v1/segmentation_service.proto`, `proto/experiments/v1/experimentation_service.proto`
- **`int64`**: `proto/analytics/v1/analytics.proto` on `EventDefinitionMsg` and `MetricDefinition`; `int64 expected_version` on update requests
- **Proposed canonical pattern**: `uint64 version` — versions are never negative.

### INCON-N004: CRUD RPC naming — `Mutate*` (flags, segments SDK) vs separate verbs (all admin paths)
- **`Mutate*`**: `proto/flags/v1/flag_service.proto` — `MutateFlag(MutateFlagRequest)` with `MutationKind` enum; `proto/segments/v1/segmentation_service.proto` — `MutateSegment` for SDK path AND separate `CreateAdminSegment`/`UpdateAdminSegment`/`DeleteAdminSegment` for admin path (mixed in same service)
- **Separate verbs**: `proto/experiments/v1/experimentation_service.proto`, `proto/analytics/v1/analytics.proto`, `proto/management/v1/management_service.proto`
- **Proposed canonical pattern**: `Mutate*` reserved for SDK sync path only. Admin CRUD should always use separate `Create`/`Update`/`Delete` RPCs.

---

## 5. RPC Shape Inconsistencies

### INCON-R001: `Delete` RPCs return inconsistently — entity vs empty response
- **Returns entity**: `ExperimentationService.DeleteExperiment` returns `Experiment`; `FlagService.MutateFlag` with `DELETE` kind returns `MutateFlagResponse { flag, version }`
- **Returns empty**: `SegmentationService.DeleteAdminSegment`, `AnalyticsService.DeleteMetric`, `AnalyticsService.DeleteEventDefinition`, `ManagementService.DeleteProject`, `ManagementService.DeleteEnvironment`, `AuthProviderService.DeleteAuthProvider` — all return empty responses
- **Notes**: Gateway discards the entity payload for all deletes and returns `204 No Content` uniformly.
- **Proposed canonical pattern**: Return empty responses from `Delete` RPCs.

### INCON-R002: `Get` RPC response wrapping is inconsistent
- **Entity returned directly (majority)**: `FlagService.GetFlag → FeatureFlag`, `ExperimentationService.GetExperiment → Experiment`, `SegmentationService.GetAdminSegment → AdminSegment`, `AnalyticsService.GetMetric → MetricDefinition`
- **Wrapped in response message**: `AuthProviderService.GetAuthProvider → GetAuthProviderResponse { provider: AuthProviderResponse }` (double-wrapped)
- **Flat fields, no domain entity**: `ManagementService.GetOrg → GetOrgResponse { org_id, org_name, created_at }` (inline fields)
- **Proposed canonical pattern**: Return the entity message directly.

### INCON-R003: `ListSegments` (SDK) returns a heterogeneous bundle; `ListAdminSegments` returns a typed list
- **Locations**:
  - `proto/segments/v1/segmentation_service.proto:18–22`: `ListSegmentsResponse { repeated RuleSegment rule_segments; repeated ListSegmentMeta list_segments }` — two separate repeated fields
  - `proto/segments/v1/segmentation_service.proto:161–164`: `ListAdminSegmentsResponse { repeated AdminSegment segments; uint64 total }` — single typed list

---

## Recommended Canonical Patterns

| Concern | Proposed Standard | Rationale |
|---------|-------------------|-----------|
| Error: `InvalidState` | `Status::failed_precondition(reason)` | Business pre-condition, not a permission issue |
| Error: `ForeignKeyViolation` | `Status::invalid_argument("referenced entity does not exist: {constraint}")` | Caller provided invalid input |
| Error: `UniqueViolation` | `Status::already_exists(msg)` | Standard gRPC semantics |
| Error: version conflict message | `"version conflict: expected {e}, actual {a}"` | Majority convention |
| Error architecture | Typed `{ServiceName}Error` enum in `error.rs` per crate | Unit-testable mappings |
| Pagination: REST params | `page` + `per_page` (1-based) via shared `PaginationParams` | Majority convention |
| Pagination: REST envelope | `PaginatedResponse<T>` → `{items, total, page, per_page}` | Single shared type |
| Pagination: internal CH-backed protos | `offset` + `limit` acceptable internally; gateway translates | ClickHouse queries are naturally offset-based |
| Validation location | Structural (empty, UUID) at service; domain business-rule at gateway when no DB round-trip needed | Thin gateway |
| Timestamp fields | `string created_at` RFC 3339 UTC | Self-documenting |
| Version field type | `uint64 version` | Non-negative by domain |
| REST env scope param name | `environment_id` (path or query) | Consistent with proto field names |
| Delete RPC response | Empty response `{}` | Gateway returns 204 anyway |
| Get RPC response | Return entity message directly | Consistent with flags, experiments, segments, analytics |
| Admin CRUD RPC naming | Separate `Create`/`Update`/`Delete` | `Mutate` reserved for SDK sync path only |

---

## Summary

- **Error handling**: 5 inconsistencies
- **Pagination**: 2 inconsistencies
- **Validation**: 2 inconsistencies
- **Naming**: 4 inconsistencies
- **RPC shapes**: 3 inconsistencies
