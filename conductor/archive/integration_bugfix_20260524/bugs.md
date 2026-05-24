# Bugs: integration_bugfix_20260524

Discovered during Phase 1 (Stack Bringup) and subsequent discovery phases.

---

## Critical

### BUG-010: RBAC not enforced — any org user (including viewer) can perform write operations
- **Phase discovered:** Phase 2 — Auth + Org Discovery
- **Component:** `crates/stitchd-gateway/src/routes/management.rs`, `crates/stitchd-gateway/src/middleware/auth.rs`
- **Reproduction:** Create a user with `org_role: viewer`; login; call `POST /v1/management/projects/{id}/environments` or `DELETE /v1/management/environments/{id}/sdk-keys/{key_id}` — both succeed with 201/204
- **Expected:** viewer gets 403 Forbidden on write operations; only `org_admin` or `org_member` (with write perms) can create/delete resources
- **Actual:** All management routes accept ANY valid JWT regardless of `org_role`. The `RbacContext.permissions` field is populated by the auth service but never checked by any route handler. Bob (viewer role) successfully created an environment and revoked an SDK key.
- **Root cause:** The `auth_middleware` only validates the JWT and injects `RbacContext` — it does NOT enforce permissions. No management route handler calls `rbac_context()` to check `org_role` or `permissions`. There is no `require_org_admin` or `require_write_permission` middleware applied to write routes.
- **Fix:** Add a `require_org_admin` (or permission-check) Axum layer to write routes in management, or add per-handler RBAC checks using the injected `RbacContext`.

---

## High

### BUG-001: `STITCHD_AUTH_ENCRYPTION_KEY` missing from docker-compose.yml
- **Phase discovered:** Phase 1 — stack bringup
- **Component:** `docker-compose.yml`, `stitchd-auth-service`
- **Reproduction:** `docker compose up` (or start auth-service without the env var set)
- **Expected:** auth-service starts and binds gRPC port 50051
- **Actual:** auth-service panics on startup: `STITCHD_AUTH_ENCRYPTION_KEY must be set: MissingEnvVar`
- **Root cause:** `docker-compose.yml` does not define `STITCHD_AUTH_ENCRYPTION_KEY` for the auth-service container, so the env var is absent when running via `docker compose`. Also missing from `.env.example`.
- **Fix:** Add `STITCHD_AUTH_ENCRYPTION_KEY` with a dev default (base64-encoded 32-byte key) to `docker-compose.yml` auth-service environment block; add to `.env.example` with a note that production deployments must generate their own key.
- **Status:** ✅ FIXED (b644d80)

### BUG-002: stats-service default gRPC ports for experimentation/analytics are swapped
- **Phase discovered:** Phase 1 — stack bringup
- **Component:** `crates/stitchd-stats-service/src/config.rs`
- **Reproduction:** Start `stitchd-stats-service` without setting `STITCHD_EXPERIMENTATION_SERVICE_GRPC_URL` or `STITCHD_ANALYTICS_SERVICE_GRPC_URL`
- **Expected:** experimentation defaults to `http://localhost:50055`, analytics defaults to `http://localhost:50054`
- **Actual:** experimentation defaults to `http://localhost:50054` (analytics port!), analytics defaults to `http://localhost:50055` (experimentation port!) → both gRPC connections land on the wrong service → runtime errors on every stats computation
- **Root cause:** The `unwrap_or_else` default strings in `StatsConfig::from_env()` have the port numbers transposed.
- **Fix:** Swap the defaults in `config.rs`.
- **Status:** ✅ FIXED (b644d80)

### BUG-009: `rename_environment` and `rename_project` always return 502 "version conflict"
- **Phase discovered:** Phase 2 — Auth + Org Discovery
- **Component:** `crates/stitchd-auth-service/src/management.rs:397,460`
- **Reproduction:** `PATCH /v1/management/environments/{env_id}` or `PATCH /v1/management/projects/{project_id}` on any freshly-created entity
- **Expected:** 204 No Content — entity renamed successfully
- **Actual:** 502 `{"error":"version conflict: expected 2, actual 1"}` — rename always fails on the first attempt
- **Root cause:** Double-increment bug. `management.rs` does `entity.version += 1` before calling `entity_repo.update()`, but the pg repo's `update()` also does `new_version = entity.version + 1` — so the WHERE clause checks `version = entity.version` (now 2) when the DB is actually at version 1. Both `rename_project` (line 397) and `rename_environment` (line 460) are affected.
- **Fix:** Remove `project.version += 1` and `env.version += 1` from management.rs — let the repo handle version bumping.

---

## Medium

### BUG-003: stats-service context_refresher queries stale table `flag_evaluation_log` instead of `flag_evaluation_log_v2`
- **Phase discovered:** Phase 1 — stack bringup
- **Component:** `crates/stitchd-stats-service/src/context_refresher.rs`
- **Reproduction:** Start stats-service; observe ERROR log: `Unknown table expression identifier 'flag_evaluation_log'`
- **Expected:** Context registry refreshes successfully from ClickHouse eval log
- **Actual:** ClickHouse returns error `Code: 60 ... Unknown table expression identifier 'flag_evaluation_log'` — the table was renamed to `flag_evaluation_log_v2` in migration `0004_flag_evaluation_log_v2.sql`, but `context_refresher.rs` still queries the old name
- **Root cause:** Stale table reference. The `ClickHouseEvalLogSource` SQL uses `FROM flag_evaluation_log` instead of `FROM flag_evaluation_log_v2`.
- **Fix:** Updated the SQL in `context_refresher.rs` to reference `flag_evaluation_log_v2`. Commit: `a7bc8f5`
- **Status:** ✅ FIXED

### BUG-004: `POST /v1/superadmin/orgs` response has `created_at: null`
- **Phase discovered:** Phase 2 — Auth + Org Discovery
- **Component:** `crates/stitchd-gateway/src/routes/admin.rs:117`, `proto/management/v1/management_service.proto`
- **Reproduction:** `POST /v1/superadmin/orgs` with a name
- **Expected:** Response contains `created_at` with the org's creation timestamp
- **Actual:** Response has `"created_at": null` — `CreateOrgResponse` proto has no `created_at` field, so gateway hardcodes `None`
- **Root cause:** `CreateOrgResponse` proto (management_service.proto) is missing `created_at`; gateway code at line 117 hardcodes `created_at: None`
- **Fix:** Add `string created_at = 5` to `CreateOrgResponse` in management_service.proto; populate it in the management service handler; update gateway to use it.

### BUG-005: `POST /v1/auth/password-reset/*` and user-invite accept endpoint are missing entirely
- **Phase discovered:** Phase 2 — Auth + Org Discovery
- **Component:** `proto/auth/v1/auth_service.proto`, `crates/stitchd-gateway/src/router.rs`
- **Reproduction:** `POST /v1/auth/password-reset/request` → 404; `POST /v1/auth/invite/accept` → 404
- **Expected:** Org users can request a password-reset email and use a reset link; invited users can accept invitations
- **Actual:** These flows are completely unimplemented — absent from auth_service.proto RPCs, not wired in gateway router
- **Root cause:** Feature not implemented. The gateway's `auth_routes` block has login, refresh, switch-org, OIDC/SAML flows, but no password reset or invite flows.
- **Fix:** Add RPCs to auth_service.proto; implement in auth-service; wire gateway routes.

### BUG-006: `GET/DELETE /v1/management/orgs/{org_id}/users` routes are missing in gateway
- **Phase discovered:** Phase 2 — Auth + Org Discovery
- **Component:** `crates/stitchd-gateway/src/routes/management.rs:469`, `router.rs:127`
- **Reproduction:** `GET /v1/management/orgs/{org_id}/users` → 405; `DELETE /v1/management/orgs/{org_id}/users/{user_id}` → 404
- **Expected:** Org admins can list members and remove members via the management API
- **Actual:** Only `POST` is wired on `/v1/management/orgs/{org_id}/users` (create_user); `ListOrgUsers` and `RemoveOrgUser` RPCs exist in management_service.proto but are not exposed by the gateway
- **Fix:** Add `get(list_org_users)` handler and `delete(remove_org_user)` handler in gateway management routes.

### BUG-007: `CreateOrgResponse` proto missing `created_at` — TopbarNav hardcodes user avatar and env badge
- **Phase discovered:** Phase 2 — Auth + Org Discovery
- **Component:** `admin/src/shell/Sidebar.tsx:178`
- **Reproduction:** Load any page in the admin UI that uses `TopbarNav` (responsive/mobile view)
- **Expected:** User avatar shows the user's initials; environment badge shows the current environment
- **Actual:** User avatar hardcoded as "PR"; environment badge hardcoded as "production" with no dynamic data
- **Fix:** Wire `auth.getSession()` for user initials; use the active environment from context/store for the badge (same as the Sidebar footer already does correctly).

### BUG-008: SDK keys cannot be named — `name` field silently dropped in create request
- **Phase discovered:** Phase 2 — Auth + Org Discovery
- **Component:** `proto/management/v1/management_service.proto` (`CreateSdkKeyRequest`), `crates/stitchd-gateway/src/routes/management.rs:162`
- **Reproduction:** `POST /v1/management/environments/{env_id}/sdk-keys` with body `{"name":"prod-key-1"}` — list view shows key without any name
- **Expected:** SDK keys can be given human-readable names for identification
- **Actual:** `CreateSdkKeyRequest` proto has no `name` field; gateway handler takes no body at all; passed `name` is silently ignored; list view shows only `sdk_key_id`, `is_active`, `created_at`, `revoked_at`
- **Fix:** Add `string name = 2` to `CreateSdkKeyRequest` and `string name = 1` to `ListSdkKeysItem`; propagate through management service to DB; show name in list view.

### BUG-012: Create-flag body uses `value_type` but read response uses `flag_type` — four flags created with wrong type
- **Phase discovered:** Phase 3 — Flags + Segments
- **Component:** `crates/stitchd-gateway/src/routes/flags.rs:33`, `crates/stitchd-gateway/src/routes/flags.rs:543`
- **Reproduction:** `POST /v1/projects/{project_id}/flags` with `{"flag_type": "string"}` — flag silently created as `bool`
- **Expected:** `flag_type` field accepted on create; response field name is consistent
- **Actual:** Create body struct `FlagMutateRequest` at line 33 uses `value_type: Option<String>` field; response's `flag_to_admin_json` at line 543 uses `flag_type` key. Flags created with `{"flag_type": "string"}` parse `value_type` as `None` → default `bool`. The `test_string_flag`, `test_int_flag`, `test_double_flag`, `test_json_flag` are all persisted as `bool`.
- **Root cause:** Field name inconsistency — `FlagMutateRequest.value_type` ≠ `AdminFlagJson.flag_type`. Callers expecting `flag_type` on create (like the REST docs/OpenAPI) silently get a bool flag.
- **Fix:** Either rename `FlagMutateRequest.value_type` to `flag_type` (match response) or add a `#[serde(alias = "flag_type")]` to `value_type` to accept both.

### BUG-013: `update_flag` silently ignores `value_type`/`flag_type` — flag type cannot be changed
- **Phase discovered:** Phase 3 — Flags + Segments
- **Component:** `crates/stitchd-gateway/src/routes/flags.rs:719`
- **Reproduction:** `PUT /v1/projects/{project_id}/flags/{flag_key}` with `{"value_type": "string", "version": N}` — type does not change in response
- **Expected:** Flag type changeable (with appropriate validation)
- **Actual:** `update_flag` handler passes `value_type` to the flag service's `MutateFlagRequest` but the flag service's `Update` kind handler ignores the `value_type` field (it is only read during `Create`). Additionally, `enabled: body.enabled.unwrap_or(false)` at line 719 means any update omitting `enabled` silently disables the flag (see BUG-014).

---

## Medium

### BUG-014: `PUT /v1/flags/{flag_key}` silently disables the flag when `enabled` is omitted from request body
- **Phase discovered:** Phase 3 — Flags + Segments
- **Component:** `crates/stitchd-gateway/src/routes/flags.rs:719`
- **Reproduction:** `PUT /v1/projects/{project_id}/flags/{flag_key}` with `{"default_variant_key": "control", "version": 6}` (omitting `enabled`) — flag becomes disabled
- **Expected:** A partial update not specifying `enabled` should preserve the existing `enabled` state
- **Actual:** `enabled: body.enabled.unwrap_or(false)` defaults the `enabled` field to `false` when absent, converting any omitted `enabled` into an implicit disable. Any partial update (e.g., only changing `default_variant_key`, `name`, or `description`) disables the flag.
- **Fix:** Either (a) fetch current state and merge, or (b) require `enabled` in the body, or (c) use `unwrap_or_else` that reads current DB value.

### BUG-015: Flag archive is irreversible — no restore endpoint exists in the gateway
- **Phase discovered:** Phase 3 — Flags + Segments
- **Component:** `crates/stitchd-gateway/src/router.rs:187`, `admin/src/pages/flags/FlagDetail.tsx:786`
- **Reproduction:** Archive a flag via `POST /v1/projects/{project_id}/flags/{flag_key}/archive`; attempt to restore via `POST /v1/projects/{project_id}/flags/{flag_key}/restore` → 404
- **Expected:** Archived flags can be restored to active status
- **Actual:** The gateway router has no `/restore` route. Once archived, a flag cannot be un-archived via the API. The admin UI confirmation message at line 786 states "You can restore it with `?include_archived=true`" which is factually wrong — that query parameter only lists archived flags.
- **Fix:** Add `POST /v1/projects/{project_id}/flags/{flag_id}/restore` route in gateway router + handler; implement `MutationKind::Restore` (or reuse `Update` with `archived: false`) in flag service.

### BUG-016: `GET /v1/projects/{project_id}/flags/{flag_key}` returns 404 for archived flags
- **Phase discovered:** Phase 3 — Flags + Segments
- **Component:** `crates/stitchd-gateway/src/routes/flags.rs` (get_flag handler)
- **Reproduction:** Archive a flag; then `GET /v1/projects/{project_id}/flags/{flag_key}` → 404 Not Found
- **Expected:** Archived flags remain fetchable by key (with `status: "archived"`) for read/audit purposes
- **Actual:** The get_flag handler passes through to flag service which performs `find_by_key` — if the flag service filters out archived flags at the repository level, archived keys 404. Once archived, no individual flag detail is accessible.
- **Fix:** Allow `find_by_key` (or provide a separate overload) to return archived flags; return the full flag object with `status: "archived"`.

---

## Low

### BUG-017: Archive endpoint returns pre-archive flag state (status: "active") instead of post-archive state
- **Phase discovered:** Phase 3 — Flags + Segments
- **Component:** `crates/stitchd-gateway/src/routes/flags.rs:796`
- **Reproduction:** `POST /v1/projects/{project_id}/flags/{flag_key}/archive` → response body shows `"status": "active"` even though the flag was archived
- **Expected:** Response body shows `"status": "archived"` (or 204 No Content)
- **Actual:** The archive handler returns `flag_to_admin_json` of the flag returned by `MutateFlagRequest { kind: Archive }`. The returned `FeatureFlag` proto has `archived: false` (the flag service may return the pre-mutation state or not set the archived field in the response). The UI sees an "active" status in the response, then must re-fetch to discover the actual archived state.
- **Fix:** Either (a) ensure the flag service response for Archive mutation sets `archived: true` in the returned `FeatureFlag`, or (b) change the endpoint to return 204 No Content.

### BUG-018: Admin UI archive confirmation message incorrectly says "`?include_archived=true`" restores the flag
- **Phase discovered:** Phase 3 — Flags + Segments
- **Component:** `admin/src/pages/flags/FlagDetail.tsx:786`
- **Reproduction:** Open archive confirmation dialog on any flag
- **Expected:** Message accurately states that archive is irreversible (until BUG-015 is fixed) or gives correct restore instructions
- **Actual:** Message says "You can restore it with `?include_archived=true`" — this is not a restore operation, just a listing filter. This misleads users into thinking archiving is reversible.
- **Fix:** Change message to either (a) "This action is currently irreversible" until restore is implemented, or (b) explain the correct restore flow once BUG-015 is fixed.

### BUG-019: `GET /v1/environments/{env_id}/segments` returns 405 Method Not Allowed
- **Phase discovered:** Phase 3 — Flags + Segments
- **Component:** `crates/stitchd-gateway/src/router.rs` (`/v1/environments/{environment_id}/segments`)
- **Reproduction:** `GET /v1/environments/{env_id}/segments` → 405 with `allow: POST`
- **Expected:** Users can list segments by environment using the environment-scoped URL pattern (consistent with other environment-scoped resources)
- **Actual:** The route at `/v1/environments/{environment_id}/segments` only registers `POST` (`create_segment_in_env`). The GET must go to `/v1/segments?env_id={env_id}` instead. This breaks the API's URL symmetry — create uses the env-path but list requires the query-param form.
- **Fix:** Add `get(list_segments)` to the `/v1/environments/{environment_id}/segments` route, or document the correct list endpoint in the OpenAPI spec.

### BUG-020: Deleted segment error message is just the segment ID with no descriptive text
- **Phase discovered:** Phase 3 — Flags + Segments
- **Component:** `crates/stitchd-gateway/src/routes/segments.rs` (get_segment handler)
- **Reproduction:** Delete a segment; then `GET /v1/segments/{segment_id}` → HTTP 404 `{"error":"<uuid>"}`
- **Expected:** HTTP 404 `{"error":"segment not found"}` or `{"error":"segment 2f7ec... not found"}`
- **Actual:** The error message contains only the raw UUID with no context, making it hard to distinguish a "not found" from any other error.
- **Fix:** Update the error message in the segment service / gateway error mapping to include "segment not found" or similar descriptive text.

---

## Phase 4: Events + Metrics + Experiments

### BUG-021: Event definitions REST surface is entirely unimplemented (all stubs)
- **Phase discovered:** Phase 4 — Events + Metrics + Experiments
- **Component:** `crates/stitchd-gateway/src/routes/events.rs` (lines 160–263)
- **Reproduction:** Any call to event definition endpoints: `GET /v1/environments/{env_id}/event-definitions` returns empty list; `POST` returns 202 with no body; `GET /{id}` returns 501; `PUT /{id}` returns 202; `DELETE /{id}` returns 204 (fake success)
- **Expected:** Full CRUD — create event definition persists to PostgreSQL; read returns it; delete removes it; events can be fired against registered keys
- **Actual:** All five handlers are stubs. `create_event_definition` returns `StatusCode::ACCEPTED` without reading the body. `list_event_definitions` returns a hardcoded empty paginated response. `get_event_definition` returns 501 Not Implemented. `update_event_definition` and `delete_event_definition` return 202/204 silently.
- **Root cause:** Gateway handlers in `events.rs` were scaffolded but never wired to the analytics service's gRPC RPCs. The `EventDefinitionRepository` in `stitchd-db` is fully implemented (PostgreSQL-backed), and the proto contains the RPCs — only the gateway proxy calls are missing.
- **Fix:** Replace each stub handler with a proper gRPC proxy call to the analytics service.

### BUG-022: `events_v2` ClickHouse migration missing — table was never created
- **Phase discovered:** Phase 4 — Events + Metrics + Experiments
- **Component:** `crates/stitchd-db/clickhouse-migrations/` (directory)
- **Reproduction:** Run `SHOW TABLES` against the ClickHouse `stitchd` database — `events_v2` table absent. Any attempt to ingest events via SDK or preview metrics fails.
- **Expected:** `events_v2` table exists after migrations run (matching `EventV2Row` struct schema: `env_id UUID`, `contexts Array(Tuple(String, String))`, `metric_key String`, nullable value cols, `timestamp/occurred_at DateTime64`, `properties Array(Tuple(String, String))`)
- **Actual:** Migration directory contains only 4 files: `0001_events.sql` (creates old `events` table with incompatible schema), `0002_experiment_assignments.sql`, `0003_flag_evaluation_log.sql`, `0004_flag_evaluation_log_v2.sql`. No migration creates `events_v2`. The analytics service (ingestion, event_query, metric aggregation) and stats service all reference `events_v2`. The comment in `ingestion.rs` references a migration `20260520000001_events_v2_properties` that does not exist on disk.
- **Root cause:** The migration creating `events_v2` was written and referenced in code comments but never committed to the migrations directory.
- **Fix:** Add a migration file `0005_events_v2.sql` creating the `events_v2` table with the schema matching `EventV2Row`. **Workaround applied:** Table created manually in the test environment via `curl`.

### BUG-023: Metric preview endpoint fails — analytics service queries `events_v2` which didn't exist
- **Phase discovered:** Phase 4 — Events + Metrics + Experiments
- **Component:** `crates/stitchd-analytics-service/src/grpc/metric.rs` (aggregation query), root cause is BUG-022
- **Reproduction:** `GET /v1/analytics/environments/{env_id}/metrics/{metric_id}/preview` → 502 `DB::Exception: ... Unknown table identifier 'events_v2'`
- **Expected:** Metric preview returns 7-day ClickHouse sparkline data (empty for new metrics, not an error)
- **Actual:** ClickHouse returns table-not-found error because `events_v2` doesn't exist. After BUG-022 workaround (creating the table), metric preview returns an empty timeseries for new metrics as expected.
- **Root cause:** Missing `events_v2` table (BUG-022). Secondary note: `clickhouse_query.rs` in `stitchd-stats-service` still queries the old `FROM events` table (dead code path, but would also fail).
- **Fix:** Fix BUG-022.

### BUG-024: Experiment creation blocked by empty context type registry — stats-service context_refresher queries ClickHouse tables that didn't exist
- **Phase discovered:** Phase 4 — Events + Metrics + Experiments
- **Component:** `crates/stitchd-gateway/src/routes/experiments.rs` (validate_experiment_binding), `crates/stitchd-stats-service/src/context_refresher.rs`
- **Reproduction:** `POST /v1/environments/{env_id}/experiments` with `unit_context_types: ["user"]` → 502 `{"error":"unknown_context_type","message":"context type 'user' is not registered for this environment"}`
- **Expected:** Common context types (`user`, `device`, `account`) are pre-registered or auto-registered on first evaluation
- **Actual:** Context type registry in PostgreSQL is empty. The `context_refresher` in stats-service polls ClickHouse `flag_evaluation_log_v2` every 15 minutes to populate the registry. Until evaluations have occurred AND been processed, no context types are registered and experiment creation is always blocked.
- **Root cause:** (1) ClickHouse tables didn't exist (BUG-022), so no evaluations could be recorded; (2) even with tables present, a 15-minute polling cycle means new environments can't create experiments without a manual seed or a shorter poll cycle. **Workaround applied:** Seeded context types directly in PostgreSQL.
- **Fix:** (a) Seed default context types (`user`, `device`, `account`) on environment creation, or (b) allow `unit_context_types` values that aren't pre-registered if they follow a naming convention.

### BUG-025: Experiment creation always fails — `Experiment` proto missing binding fields; service uses random placeholder UUIDs
- **Phase discovered:** Phase 4 — Events + Metrics + Experiments
- **Component:** `crates/stitchd-experimentation-service/src/service.rs:266–292`, `proto/experiments/v1/experimentation_service.proto`
- **Reproduction:** `POST /v1/environments/{env_id}/experiments` with valid `flag_id`, `flag_rule_id`, `unit_context_types`, etc. → 502 `{"error":"unique violation on: experiments_flag_rule_id_fkey"}`
- **Expected:** Experiment created and stored with the provided binding fields
- **Actual:** The `Experiment` proto has no fields for `flag_id`, `flag_rule_id`, `targets_default_rule`, `unit_context_types`, `guardrail_metric_ids`, or `pre_period_days`. The gateway validates and accepts these in the request body but drops them all when constructing the proto. The experimentation service's `create_experiment` handler (with a comment "placeholder values land here. Phase 3 extends the proto schema…") inserts `flag_id: FlagId::new()` and `flag_rule_id: Some(RuleId::new())` — random UUIDs that fail the foreign key constraints on `experiments_flag_id_fkey` / `experiments_flag_rule_id_fkey`.
- **Root cause:** Proto schema and service handler were intentionally deferred ("Phase 3") but never implemented.
- **Fix:** Add `flag_id`, `flag_rule_id`, `targets_default_rule`, `unit_context_types`, `guardrail_metric_ids`, `pre_period_days` to the `Experiment` proto; update gateway to populate them; update service handler to use them.

### BUG-026: `map_experiment_db_err` misclassifies all DB constraint violations as unique violations
- **Phase discovered:** Phase 4 — Events + Metrics + Experiments
- **Component:** `crates/stitchd-db/src/repository/pg/experiment.rs:814–822`
- **Reproduction:** Any DB constraint violation during experiment INSERT — FK, check, or unique — reports as `{"error":"unique violation on: <constraint_name>"}` regardless of actual constraint type
- **Expected:** Foreign key violations → `{"error":"referenced entity does not exist: <constraint>"}`, check violations → appropriate error, unique violations → `{"error":"unique violation on: <field>"}`
- **Actual:** `map_experiment_db_err` checks only `dbe.constraint()` (the constraint name) and always maps to `RepositoryError::UniqueViolation`. Specifically: FK violation on `experiments_flag_rule_id_fkey` is reported as "unique violation on: experiments_flag_rule_id_fkey", mixing up error semantics and leaking internal constraint names.
- **Root cause:** Function uses a single `if let` that ignores the SQL error code (SQLSTATE 23505/23503/23514).
- **Fix:** Check `dbe.code()` before the constraint name: `"23505"` → `UniqueViolation`, `"23503"` → `ForeignKeyViolation`, others → `Database`.

### BUG-027: `PATCH /v1/metrics/{id}` requires full metric body — partial update (e.g. rename only) is not supported
- **Phase discovered:** Phase 4 — Events + Metrics + Experiments
- **Component:** `crates/stitchd-gateway/src/routes/metrics.rs` (update_metric handler)
- **Reproduction:** `PATCH /v1/metrics/{id}` with `{"name":"new name","expected_version":1}` → 400 `{"error":"invalid request body: missing field 'kind'"}`
- **Expected:** PATCH semantics — only include fields to change; omitted fields retain current values
- **Actual:** Handler deserializes a `CreateMetricBody`-equivalent struct where `kind` (and its associated aggregation/ratio/funnel sub-fields) is required even for simple name or description changes.
- **Fix:** Make `kind` and all metric-type-specific fields optional in the update body struct; merge only provided fields at the service layer.

---

## Phase 5: SDK Integration + UI/UX Polish

### BUG-028: Flag service ClickHouse env vars named differently from docker-compose — auth fails silently
- **Phase discovered:** Phase 5 — SDK Integration
- **Component:** `crates/stitchd-flag-service/src/main.rs:98–107`, `.claude/launch.json`
- **Reproduction:** Start flag service using `.claude/launch.json`; call `POST /v1/sdk/events:batch` → 502 `ClickHouse auth error: authentication failure`
- **Expected:** Flag service connects to ClickHouse using the same credentials as all other services
- **Actual:** Flag service reads `STITCHD_CLICKHOUSE_USER` / `STITCHD_CLICKHOUSE_PASSWORD`; the launch config sets `CLICKHOUSE_USER` / `CLICKHOUSE_PASSWORD` (no `STITCHD_` prefix). The env vars don't match, so the flag service falls back to default user `"default"` with no password → ClickHouse authentication failure.
- **Root cause:** `flag-service/src/main.rs` uses `STITCHD_CLICKHOUSE_*` naming while the launch config uses the docker-compose naming (`CLICKHOUSE_*`). All other services read `STITCHD_CLICKHOUSE_*` consistently.
- **Fix:** Update `.claude/launch.json` flag-service entry to use `STITCHD_CLICKHOUSE_USER` / `STITCHD_CLICKHOUSE_PASSWORD`.

### BUG-030: `flag_evaluation_log` captures only one context per row — cross-context evaluations lose bundle membership
- **Phase discovered:** Phase 5 — SDK Integration
- **Component:** `crates/stitchd-db/src/clickhouse/eval_log.rs` (`EvalLogRow`), `sdks/spec/proto/sdk/v1/service.proto` (`FlagEvaluationEvent`), `crates/stitchd-flag-service/src/eval_log_writer.rs`
- **Reproduction:** Evaluate a flag with a cross-context hash rule (e.g., `user.key + device.params.os`); inspect `flag_evaluation_log` — the evaluation produces separate rows for each context in the bundle with no identifier linking them.
- **Expected:** A flag evaluation against a context bundle (user + device + application) should be traceable as a single logical event, with all participating contexts and their parameters available together for experiment attribution and analytics.
- **Actual:** Two separate schema problems:
  1. **Server-side path** (`eval_log_writer.rs:83–98`): `build_eval_log_rows` iterates `eval_ctx.contexts` and emits one `EvalLogRow` per context. Each row has its own `context_type`/`context_key`/`params_json` but there is no `evaluation_id` or bundle grouping field. For a `user + device` evaluation, two rows land in the table — but there is no way to tell which "user" row and which "device" row came from the same evaluation vs. two independent single-context evaluations.
  2. **SDK-reported path** (`FlagEvaluationEvent` proto): has only `string context_type = 3` and `string context_key = 4` — singular. For a cross-context flag evaluation, the SDK can only report ONE context. All sibling contexts (e.g., `device.params.os` used as the hash input) are silently dropped from the event payload.
- **Downstream consequence:** The `experiment_assignments_mv` materialised view joins `flag_evaluation_log` to attribute exposures to the experiment's `unit_context_type` (e.g., "user"). Without a bundle ID, it cannot associate the "user" row from a cross-context evaluation with its paired "device" row — making cross-context experiment exposure attribution incorrect. Similarly, the context registry co-occurrence (which contexts appear together) is uncomputable.
- **Root cause:** The original schema was designed for single-context evaluations. Cross-context hash support was added to the flag evaluation engine but the ClickHouse schema and SDK proto were not updated to carry the full context bundle.
- **Fix:**
  - Add `evaluation_id UUID` column to `flag_evaluation_log` (and `flag_evaluation_log_v2`) as a shared identifier for all rows from the same evaluation call.
  - Add `repeated ContextEntry sibling_contexts = 12` (or equivalent) to `FlagEvaluationEvent` proto so the SDK can report all contexts that participated in the evaluation.
  - Update `eval_log_writer::build_eval_log_rows` to generate and stamp the same `evaluation_id` UUID across all rows from one call.
  - Update `experiment_assignments_mv` to use `evaluation_id` when joining unit context to sibling contexts.

### BUG-029: `EvalLogRow` struct fields don't match `flag_evaluation_log` ClickHouse schema — SDK events:batch always 502
- **Phase discovered:** Phase 5 — SDK Integration
- **Component:** `crates/stitchd-db/src/clickhouse/eval_log.rs`, `crates/stitchd-db/clickhouse-migrations/0003_flag_evaluation_log.sql`
- **Reproduction:** Fix BUG-028 and call `POST /v1/sdk/events:batch` with a valid SDK key → 502 `schema mismatch: While processing struct EvalLogRow: database schema has no column named targeting_on`
- **Expected:** Evaluation log events are inserted successfully into ClickHouse
- **Actual:** `EvalLogRow` struct declares `targeting_on: bool` and `matched_rule_id: Option<Uuid>`; the `flag_evaluation_log` table created by migration `0003` has `is_disabled Bool` instead of `targeting_on`, and has no `matched_rule_id` column. ClickHouse rejects the insert with a schema mismatch error.
- **Root cause:** The struct was updated to the v2 schema (`targeting_on`, `matched_rule_id`) but no ClickHouse migration was written to rename the column and add the new one. The migrations directory only has `0003_flag_evaluation_log.sql` (old schema) and `0004_flag_evaluation_log_v2.sql` (copies the table, but `EvalLogRow` still writes to the original `flag_evaluation_log` table).
- **Fix:** Add a ClickHouse migration (or alter statement) to: (1) rename `is_disabled` → `targeting_on` in `flag_evaluation_log`, (2) add `matched_rule_id Nullable(UUID)`. Also apply the same schema to `flag_evaluation_log_v2` for consistency.

### BUG-031: Dashboard welcome heading and sidebar breadcrumb show raw org UUID instead of org name
- **Phase discovered:** Phase 5 — UI/UX Polish
- **Component:** `admin/src/pages/Dashboard.tsx` (welcome heading), `admin/src/shell/Sidebar.tsx` (org breadcrumb)
- **Reproduction:** Log in as any org user; observe the dashboard welcome heading and the org label in the sidebar top section
- **Expected:** Both locations display the human-readable org name (e.g., "Acme Corp")
- **Actual:** Both show the raw org UUID (e.g., `02e6f3b1-4a2d-…`). The JWT `org_id` claim is rendered directly without resolving it to a display name. The sidebar fetches the user's profile but not the org detail.
- **Root cause:** The dashboard and sidebar components read `org_id` from the auth session but never call `GET /v1/management/orgs/{org_id}` (or a cached store slice) to retrieve `org_name`. The org name is available in the management API response but is never stored in client-side auth state.
- **Fix:** On login / org switch, fetch and cache the org detail (at minimum `name`). Populate it into the auth store; read `auth.org.name` in the dashboard welcome heading and sidebar org label instead of `auth.org_id`.

### BUG-032: Preview tab "Evaluate" button accepts empty context without validation — shows "(no key) —" result
- **Phase discovered:** Phase 5 — UI/UX Polish
- **Component:** `admin/src/pages/flags/FlagDetail.tsx` (Preview/Evaluate panel)
- **Reproduction:** Open any flag's Preview tab; leave the context `_type` and `key` fields blank; click "Evaluate"
- **Expected:** Inline validation error under the `_type` and `key` fields; "Evaluate" button disabled or rejected until both fields are populated
- **Actual:** The request is sent with `_type: ""` and `key: ""`; the server returns a (default) result; the UI renders it as "(no key) —" in the result row. There is no client-side guard requiring a non-empty context type and key before evaluation.
- **Fix:** Add required-field validation to the Preview panel context form: both `_type` and `key` must be non-empty before the Evaluate button is enabled (or show inline errors on submit).

### BUG-033: `display_name` is not validated as required on user creation — sidebar shows "Org User" for users created without a name
- **Phase discovered:** Phase 5 — UI/UX Polish
- **Component:** `crates/stitchd-gateway/src/routes/management.rs` (`CreateUserBody`), `admin/src/shell/Sidebar.tsx`
- **Reproduction:** Create a user omitting the `display_name` field (or sending `"display_name": ""`); log in as that user; observe the sidebar name label
- **Expected:** Either (a) `display_name` is required and the server returns 400 when absent/blank, or (b) the sidebar falls back to the user's email address
- **Actual:** `CreateUserBody.display_name` is `String` (not `Option<String>`) and has no `min_length` validation — sending `""` passes server-side checks. The sidebar renders `display_name` directly; a blank `display_name` shows "Org User" (the hardcoded fallback), not the user's email. This makes it impossible to distinguish multiple users without display names.
- **Fix:** Either (a) validate `display_name` as non-empty in the gateway handler, or (b) update the sidebar fallback to use `email` when `display_name` is blank.

### BUG-034: Flags/Segments/Experiments filter with no matching results shows a blank content area instead of an empty-state message
- **Phase discovered:** Phase 5 — UI/UX Polish
- **Component:** `admin/src/pages/flags/FlagList.tsx`, `admin/src/pages/segments/SegmentList.tsx`, `admin/src/pages/experiments/ExperimentList.tsx`
- **Reproduction:** On any list page with data, type a search term that matches nothing (e.g., "zzz") into the filter/search box
- **Expected:** A visible "No results found" (or similar) empty-state message with a suggestion to clear the filter
- **Actual:** The list area is completely blank — no rows, no message, no visual feedback. Users cannot tell whether the filter is working or the page is broken.
- **Fix:** Add an empty-state component that renders when the filtered result set is empty. Include a "Clear filter" action link.

---
