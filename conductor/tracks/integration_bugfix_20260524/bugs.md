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
