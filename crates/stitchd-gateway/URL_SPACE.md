# Stitchd Gateway — Canonical URL Space

This document is the single source of truth for the gateway's URL structure.
It drives Phase 2 (`boundaries_20260518`) of the URL-canonicalisation refactor.

---

## Path-parameter naming conventions

| Resource         | Path param name      |
|------------------|----------------------|
| Organisation     | `{org_id}`           |
| Project          | `{project_id}`       |
| Environment      | `{environment_id}`   |
| Feature flag     | `{flag_id}`          |
| Segment          | `{segment_id}`       |
| Experiment       | `{experiment_id}`    |
| Event definition | `{event_definition_id}` |
| SDK key          | `{sdk_key_id}`       |
| User             | `{user_id}`          |
| Auth provider    | `{auth_provider_id}` |
| Async job        | `{job_id}`           |
| OIDC/SAML provider | `{provider_id}`    |
| Context type     | `{context_type}`     |

**Rules:**
- All path params use `snake_case` full-form names.
- No abbreviations: no `{id}`, `{def_id}`, `{env_id}`.
- Segment IDs come from the `{segment_id}` path param (replacing old `{id}`).
- Auth provider IDs come from `{auth_provider_id}` (replacing old `{id}`).

---

## Route trees

### 1. Public auth routes (no middleware)

| Method | New path (canonical) | Old path | Handler |
|--------|----------------------|----------|---------|
| GET    | `/v1/health`         | `/health` | inline 200 OK |
| POST   | `/v1/auth/login`     | `/v1/auth/login` | `auth::login` |
| POST   | `/v1/auth/refresh`   | `/v1/auth/refresh` | `auth::refresh` |
| GET    | `/v1/auth/me/orgs`   | `/v1/auth/me/orgs` | `auth::list_user_orgs` |
| POST   | `/v1/auth/switch-org` | `/v1/auth/switch-org` | `auth::switch_org` |
| POST   | `/v1/auth/oidc/{provider_id}/authorize` | same | `oidc::oidc_authorize_by_provider` |
| GET    | `/v1/auth/oidc/{provider_id}/callback`  | same | `oidc::oidc_callback` |
| POST   | `/v1/auth/saml/{provider_id}/sso`       | same | `saml::saml_sso_by_provider` |
| POST   | `/v1/auth/saml/{provider_id}/callback`  | same | `saml::saml_acs_callback` |

**Changes:** `/health` → `/v1/health`. All others unchanged.

---

### 2. Superadmin-only routes (`/v1/superadmin/*`, JWT + `require_system_org`)

| Method | New path (canonical) | Old path | Handler |
|--------|----------------------|----------|---------|
| GET    | `/v1/superadmin/orgs` | `/v1/admin/orgs` | `admin::list_orgs` |
| POST   | `/v1/superadmin/orgs` | `/v1/admin/orgs` | `admin::create_org` |
| GET    | `/v1/superadmin/orgs/{org_id}` | `/v1/admin/orgs/{org_id}` | `admin::get_org` |
| GET    | `/v1/superadmin/orgs/{org_id}/users` | `/v1/admin/orgs/{org_id}/users` | `admin::list_org_users` |
| POST   | `/v1/superadmin/orgs/{org_id}/users` | `/v1/admin/orgs/{org_id}/users` | `admin::seed_user` |
| DELETE | `/v1/superadmin/orgs/{org_id}/users/{user_id}` | `/v1/admin/orgs/{org_id}/users/{user_id}` | `admin::remove_org_user` |

**Changes:** `/v1/admin/*` → `/v1/superadmin/*`. Path params unchanged (already full-form).

---

### 3. Management routes (`/v1/management/*`, JWT + `require_non_system_org`)

| Method | New path (canonical) | Old path | Handler |
|--------|----------------------|----------|---------|
| GET    | `/v1/management/orgs/{org_id}/projects` | same | `management::list_projects` |
| POST   | `/v1/management/orgs/{org_id}/projects` | same | `management::create_project` |
| PATCH  | `/v1/management/projects/{project_id}` | same | `management::rename_project` |
| DELETE | `/v1/management/projects/{project_id}` | same | `management::delete_project` |
| GET    | `/v1/management/projects/{project_id}/environments` | same | `management::list_environments` |
| POST   | `/v1/management/projects/{project_id}/environments` | same | `management::create_environment` |
| PATCH  | `/v1/management/environments/{environment_id}` | same | `management::rename_environment` |
| DELETE | `/v1/management/environments/{environment_id}` | same | `management::delete_environment` |
| GET    | `/v1/management/environments/{environment_id}/sdk-keys` | same | `management::list_sdk_keys` |
| POST   | `/v1/management/environments/{environment_id}/sdk-keys` | same | `management::create_sdk_key` |
| DELETE | `/v1/management/environments/{environment_id}/sdk-keys/{sdk_key_id}` | same | `management::revoke_sdk_key` |
| POST   | `/v1/management/orgs/{org_id}/users` | same | `management::create_user` |

**Changes:** None — management paths were already canonical.

---

### 4. SDK backend routes (`/v1/sdk/*`, `sdk_auth_middleware`)

| Method | New path (canonical) | Old path | Handler |
|--------|----------------------|----------|---------|
| POST   | `/v1/sdk/segments/list:batch` | same | `sdk_backend::segments_list_batch` |
| POST   | `/v1/sdk/events:batch`        | same | `sdk_backend::events_batch` |

**Changes:** None — these are the new canonical SDK paths.

---

### 5. Legacy SDK routes — RETIRED (Task 2.3)

These routes are removed entirely. The SDK must use `/v1/sdk/*` instead.

| Method | Retired path | Notes |
|--------|--------------|-------|
| POST   | `/v1/environments/{env_id}/evaluate` | Retired. SDK uses `/v1/sdk/segments/list:batch`. |
| POST   | `/v1/environments/{env_id}/events` | Retired. SDK uses `/v1/sdk/events:batch`. |
| POST   | `/v1/environments/{env_id}/events/batch` | Retired. SDK uses `/v1/sdk/events:batch`. |
| POST   | `/v1/environments/{env_id}/segments/list-check` | Retired. SDK uses `/v1/sdk/segments/list:batch`. |
| POST   | `/v1/environments/{env_id}/segments/list-check/batch` | Retired. SDK uses `/v1/sdk/segments/list:batch`. |

`routes/sdk.rs` is deleted. `routes/sdk_backend.rs` is the only remaining SDK REST surface.

---

### 6. JWT-authenticated resource routes

#### Flags

| Method | New path (canonical) | Old path | Handler |
|--------|----------------------|----------|---------|
| GET    | `/v1/projects/{project_id}/flags` | same | `flags::list_flags` |
| POST   | `/v1/projects/{project_id}/flags` | same | `flags::create_flag` |
| GET    | `/v1/projects/{project_id}/flags/{flag_id}` | same | `flags::get_flag` |
| PUT    | `/v1/projects/{project_id}/flags/{flag_id}` | same | `flags::update_flag` |
| DELETE | `/v1/projects/{project_id}/flags/{flag_id}` | same | `flags::delete_flag` |
| POST   | `/v1/projects/{project_id}/flags/{flag_id}/archive` | same | `flags::archive_flag` |
| PUT    | `/v1/projects/{project_id}/flags/{flag_id}/variants` | same | `flags::update_variants` |
| PUT    | `/v1/projects/{project_id}/flags/{flag_id}/rules` | same | `flags::update_rules` |
| PUT    | `/v1/projects/{project_id}/flags/{flag_id}/hashing` | same | `flags::update_flag_hashing` |
| POST   | `/v1/projects/{project_id}/flags/{flag_id}/evaluate-preview` | same | `flags::evaluate_preview` |
| GET    | `/v1/projects/{project_id}/flags/{flag_id}/eval-stats` | same | `eval_stats::get_eval_stats` |

**Changes:** None — flag paths were already canonical.

#### Segments

| Method | New path (canonical) | Old path | Handler |
|--------|----------------------|----------|---------|
| GET    | `/v1/segments?env_id=<uuid>` | same | `segments::list_segments` |
| POST   | `/v1/segments` | same | `segments::create_segment` |
| GET    | `/v1/segments/{segment_id}` | `/v1/segments/{id}` | `segments::get_segment` |
| PUT    | `/v1/segments/{segment_id}` | `/v1/segments/{id}` | `segments::update_segment` |
| DELETE | `/v1/segments/{segment_id}` | `/v1/segments/{id}` | `segments::delete_segment` |
| POST   | `/v1/segments/{segment_id}/entries` | `/v1/segments/{id}/entries` | `segments::patch_segment_entries` |
| GET    | `/v1/segments/{segment_id}/entries/lookup` | `/v1/segments/{id}/entries/lookup` | `segments::lookup_segment_entry` |
| POST   | `/v1/environments/{environment_id}/segments` | `/v1/environments/{env_id}/segments` | `segments::create_segment_in_env` |

**Changes:** `{id}` → `{segment_id}`; `{env_id}` → `{environment_id}`.

#### Event definitions

| Method | New path (canonical) | Old path | Handler |
|--------|----------------------|----------|---------|
| POST   | `/v1/environments/{environment_id}/events` | `/v1/environments/{env_id}/events` | `events::ingest_event` |
| POST   | `/v1/environments/{environment_id}/events/batch` | `/v1/environments/{env_id}/events/batch` | `events::ingest_batch` |
| GET    | `/v1/environments/{environment_id}/event-definitions` | `/v1/environments/{env_id}/event-definitions` | `events::list_event_definitions` |
| POST   | `/v1/environments/{environment_id}/event-definitions` | `/v1/environments/{env_id}/event-definitions` | `events::create_event_definition` |
| GET    | `/v1/environments/{environment_id}/event-definitions/{event_definition_id}` | `/v1/environments/{env_id}/event-definitions/{def_id}` | `events::get_event_definition` |
| PUT    | `/v1/environments/{environment_id}/event-definitions/{event_definition_id}` | `/v1/environments/{env_id}/event-definitions/{def_id}` | `events::update_event_definition` |
| DELETE | `/v1/environments/{environment_id}/event-definitions/{event_definition_id}` | `/v1/environments/{env_id}/event-definitions/{def_id}` | `events::delete_event_definition` |

**Changes:** `{env_id}` → `{environment_id}`; `{def_id}` → `{event_definition_id}`.

#### Experiments

| Method | New path (canonical) | Old path | Handler |
|--------|----------------------|----------|---------|
| GET    | `/v1/environments/{environment_id}/experiments` | `/v1/environments/{env_id}/experiments` | `experiments::list_experiments` |
| POST   | `/v1/environments/{environment_id}/experiments` | `/v1/environments/{env_id}/experiments` | `experiments::create_experiment` |
| GET    | `/v1/environments/{environment_id}/experiments/{experiment_id}` | same (env_id→environment_id) | `experiments::get_experiment` |
| PATCH  | `/v1/environments/{environment_id}/experiments/{experiment_id}` | same (env_id→environment_id) | `experiments::update_experiment` |
| DELETE | `/v1/environments/{environment_id}/experiments/{experiment_id}` | same (env_id→environment_id) | `experiments::delete_experiment` |
| POST   | `/v1/environments/{environment_id}/experiments/{experiment_id}/transitions` | same (env_id→environment_id) | `experiments::transition_experiment` |
| GET    | `/v1/environments/{environment_id}/experiments/{experiment_id}/iterations` | same (env_id→environment_id) | `experiments::list_iterations` |
| GET    | `/v1/environments/{environment_id}/experiments/{experiment_id}/results` | same (env_id→environment_id) | `experiments::get_results` |

**Changes:** `{env_id}` → `{environment_id}`.

#### Context intelligence

| Method | New path (canonical) | Old path | Handler |
|--------|----------------------|----------|---------|
| GET    | `/v1/environments/{environment_id}/context-types` | `/v1/environments/{env_id}/context-types` | `context_intel::list_context_types` |
| GET    | `/v1/environments/{environment_id}/context-types/{context_type}/params` | `/v1/environments/{env_id}/context-types/{context_type}/params` | `context_intel::list_context_params` |

**Changes:** `{env_id}` → `{environment_id}`.

#### Stats / Jobs

| Method | New path (canonical) | Old path | Handler |
|--------|----------------------|----------|---------|
| POST   | `/v1/experiments/{experiment_id}/recompute` | same | `stats::trigger_recompute` |
| GET    | `/v1/jobs/{job_id}` | same | `stats::get_job_status` |

**Changes:** None — already canonical.

---

### 7. Auth-provider management routes (JWT + `require_non_system_org`)

| Method | New path (canonical) | Old path | Handler |
|--------|----------------------|----------|---------|
| GET    | `/v1/orgs/{org_id}/auth-providers` | same | `auth_providers::list_auth_providers` |
| POST   | `/v1/orgs/{org_id}/auth-providers` | same | `auth_providers::create_auth_provider` |
| GET    | `/v1/orgs/{org_id}/auth-providers/{auth_provider_id}` | `/v1/orgs/{org_id}/auth-providers/{id}` | `auth_providers::get_auth_provider` |
| PUT    | `/v1/orgs/{org_id}/auth-providers/{auth_provider_id}` | `/v1/orgs/{org_id}/auth-providers/{id}` | `auth_providers::update_auth_provider` |
| DELETE | `/v1/orgs/{org_id}/auth-providers/{auth_provider_id}` | `/v1/orgs/{org_id}/auth-providers/{id}` | `auth_providers::delete_auth_provider` |
| GET    | `/v1/orgs/{org_id}/auth-providers/{auth_provider_id}/saml/metadata` | `/v1/orgs/{org_id}/auth-providers/{id}/saml/metadata` | `auth_providers::get_saml_sp_metadata` |
| POST   | `/v1/orgs/{org_id}/auth/oidc/authorize` | same | `oidc::oidc_authorize_by_org` |
| POST   | `/v1/orgs/{org_id}/auth/saml/sso` | same | `saml::saml_sso_by_org` |

**Changes:** `{id}` → `{auth_provider_id}`.

---

### 8. Versioned health and metrics

| Method | New path | Old path |
|--------|----------|----------|
| GET    | `/v1/health` | `/health` |
| GET    | `/v1/metrics` | `/metrics` (not previously registered; added now) |

---

## utoipa tag assignments

| Module               | Tag name          |
|----------------------|-------------------|
| `routes/admin.rs`    | `superadmin`      |
| `routes/auth.rs`     | `auth`            |
| `routes/auth_providers.rs` | `auth-providers` |
| `routes/context_intel.rs` | `context-intel` |
| `routes/eval_stats.rs` | `flags`         |
| `routes/events.rs`   | `event-definitions` |
| `routes/experiments.rs` | `experiments`  |
| `routes/flags.rs`    | `flags`           |
| `routes/management.rs` | `management`    |
| `routes/oidc.rs`     | `auth`            |
| `routes/saml.rs`     | `auth`            |
| `routes/sdk_backend.rs` | `sdk`          |
| `routes/segments.rs` | `segments`        |
| `routes/stats.rs`    | `stats`           |

---

## Summary of changes by task

| Task | Change |
|------|--------|
| 2.1  | This document |
| 2.2  | `{id}` → `{segment_id}` in segments; `{id}` → `{auth_provider_id}` in auth-providers; `{env_id}` → `{environment_id}` in events/experiments/context-intel/segments; `{def_id}` → `{event_definition_id}` in events |
| 2.3  | Delete `routes/sdk.rs`; remove from `mod.rs` and `router.rs` |
| 2.4  | `/v1/admin/*` → `/v1/superadmin/*` in router and admin handlers |
| 2.5  | `/health` → `/v1/health`; add `/v1/metrics`; update docker-compose healthcheck |
| 2.6  | Add `tag = "..."` to every `#[utoipa::path]` missing one |
