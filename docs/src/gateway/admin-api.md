# Admin & Management APIs

REST endpoints behind `Authorization: Bearer <jwt>`. Called by the Admin UI
and the stitchd CLI. Two sub-trees enforce additional middleware: the
`/v1/superadmin/*` tree requires `is_system = true` (System-org users
only) and the `/v1/management/*` tree refuses System-org users so that
real-tenant management calls don't leak across the boundary.

## Auth

```
Authorization: Bearer eyJhbGci...
```

Obtain a token from `POST /v1/auth/login`, the OIDC callback, or the
SAML ACS callback. Tokens expire after the auth-service's configured
TTL; the SPA refreshes them via `POST /v1/auth/refresh`.

A `401 Unauthorized` means the token is missing, invalid, or expired.
A `403 Forbidden` from the superadmin or management trees means the
token authenticated successfully but failed the `is_system` check.

## Public auth endpoints

| Method | Path                                            | Auth | Purpose                                      |
|--------|-------------------------------------------------|------|----------------------------------------------|
| GET    | `/v1/health`                                    | none | Liveness probe (returns `200 OK`).           |
| POST   | `/v1/auth/login`                                | none | Email + password login → JWT.                |
| POST   | `/v1/auth/refresh`                              | none | Exchange refresh token for a new access JWT. |
| GET    | `/v1/auth/me/orgs`                              | none | List orgs the (token-identified) user belongs to. |
| POST   | `/v1/auth/switch-org`                           | none | Mint a new JWT scoped to a different org membership. |
| POST   | `/v1/auth/oidc/{provider_id}/authorize`         | none | Begin OIDC SSO for a configured provider.    |
| GET    | `/v1/auth/oidc/{provider_id}/callback`          | none | OIDC IdP redirect target.                    |
| POST   | `/v1/auth/saml/{provider_id}/sso`               | none | Begin SAML SSO for a configured provider.    |
| POST   | `/v1/auth/saml/{provider_id}/callback`          | none | SAML ACS POST target.                        |
| GET    | `/v1/auth/me/permissions`                       | JWT  | Return the caller's resolved RBAC permissions. |

```bash
curl -X POST http://localhost:8080/v1/auth/login \
  -H 'content-type: application/json' \
  -d '{"email":"admin@example.com","password":"hunter2"}'
```

```json
{ "token": "eyJhbGci...", "expires_at": "2026-05-23T08:30:00Z" }
```

## Superadmin (`/v1/superadmin/*`)

System-org JWT only. Used to provision tenant orgs and seed their first
users. Routes proxy to `ManagementService` (hosted on the auth-service
port).

| Method | Path                                                  | Purpose                                       |
|--------|-------------------------------------------------------|-----------------------------------------------|
| GET    | `/v1/superadmin/orgs`                                 | List every org in the deployment.             |
| POST   | `/v1/superadmin/orgs`                                 | Create a new org.                             |
| GET    | `/v1/superadmin/orgs/{org_id}`                        | Fetch a single org.                           |
| GET    | `/v1/superadmin/orgs/{org_id}/users`                  | List users in an org.                         |
| POST   | `/v1/superadmin/orgs/{org_id}/users`                  | Seed (invite) a new user into the org.        |
| DELETE | `/v1/superadmin/orgs/{org_id}/users/{user_id}`        | Remove a user from the org.                   |

## Management (`/v1/management/*`)

Tenant-scoped JWT, **not** System-org. The Admin UI's Settings pages call
into this tree. Routes proxy to `ManagementService`.

| Method | Path                                                                              | Purpose                                  |
|--------|-----------------------------------------------------------------------------------|------------------------------------------|
| GET    | `/v1/management/orgs/{org_id}/projects`                                           | List projects in the org.                |
| POST   | `/v1/management/orgs/{org_id}/projects`                                           | Create a project.                        |
| PATCH  | `/v1/management/projects/{project_id}`                                            | Rename a project.                        |
| DELETE | `/v1/management/projects/{project_id}`                                            | Delete a project.                        |
| GET    | `/v1/management/projects/{project_id}/environments`                               | List environments in a project.          |
| POST   | `/v1/management/projects/{project_id}/environments`                               | Create an environment.                   |
| PATCH  | `/v1/management/environments/{environment_id}`                                    | Rename an environment.                   |
| DELETE | `/v1/management/environments/{environment_id}`                                    | Delete an environment.                   |
| GET    | `/v1/management/environments/{environment_id}/sdk-keys`                           | List the env's SDK keys (key bodies are write-once). |
| POST   | `/v1/management/environments/{environment_id}/sdk-keys`                           | Issue a new SDK key. The response includes the key body — copy it on the spot. |
| DELETE | `/v1/management/environments/{environment_id}/sdk-keys/{sdk_key_id}`              | Revoke an SDK key.                       |
| POST   | `/v1/management/orgs/{org_id}/users`                                              | Create a user in the org.                |

### Auth providers (`/v1/orgs/{org_id}/auth-providers`)

The auth-provider CRUD lives under the same non-system-org guard as
`/v1/management/*` but is a sibling tree (the URL prefix is different
to match the Admin UI's URL space). Routes proxy to
`AuthProviderService`.

| Method | Path                                                                  | Purpose                                    |
|--------|-----------------------------------------------------------------------|--------------------------------------------|
| GET    | `/v1/orgs/{org_id}/auth-providers`                                    | List the org's auth providers.             |
| POST   | `/v1/orgs/{org_id}/auth-providers`                                    | Create an auth provider (OIDC or SAML).    |
| GET    | `/v1/orgs/{org_id}/auth-providers/{auth_provider_id}`                 | Get one provider.                          |
| PUT    | `/v1/orgs/{org_id}/auth-providers/{auth_provider_id}`                 | Update a provider.                         |
| DELETE | `/v1/orgs/{org_id}/auth-providers/{auth_provider_id}`                 | Delete a provider.                         |
| GET    | `/v1/orgs/{org_id}/auth-providers/{auth_provider_id}/saml/metadata`   | Download SP metadata XML for IdP config.   |
| POST   | `/v1/orgs/{org_id}/auth/oidc/authorize`                               | Org-scoped OIDC initiate (authed user picks their org's IdP). |
| POST   | `/v1/orgs/{org_id}/auth/saml/sso`                                     | Org-scoped SAML initiate.                  |

## Flags (`/v1/projects/{project_id}/flags`)

Flags are scoped to a **project** in the URL space — env-scoped overrides
are configured through rules. Routes proxy to `FlagService`.

| Method | Path                                                                                      | Purpose                                                       |
|--------|-------------------------------------------------------------------------------------------|---------------------------------------------------------------|
| GET    | `/v1/projects/{project_id}/flags`                                                         | List flags (paginated).                                       |
| POST   | `/v1/projects/{project_id}/flags`                                                         | Create a flag.                                                |
| GET    | `/v1/projects/{project_id}/flags/{flag_id}`                                               | Get one flag.                                                 |
| PUT    | `/v1/projects/{project_id}/flags/{flag_id}`                                               | Replace a flag (general `MutateFlag`).                        |
| DELETE | `/v1/projects/{project_id}/flags/{flag_id}`                                               | Delete a flag.                                                |
| POST   | `/v1/projects/{project_id}/flags/{flag_id}/archive`                                       | Archive a flag (soft-hide, preserves history).                |
| PUT    | `/v1/projects/{project_id}/flags/{flag_id}/variants`                                      | Replace the variant list.                                     |
| PUT    | `/v1/projects/{project_id}/flags/{flag_id}/rules`                                         | Replace the targeting-rule list.                              |
| POST   | `/v1/projects/{project_id}/flags/{flag_id}/default-rule-distribution`                     | Set or clear the flag's default-rule percentage distribution. |
| PUT    | `/v1/projects/{project_id}/flags/{flag_id}/hashing`                                       | Update the flag's hashing config.                             |
| POST   | `/v1/projects/{project_id}/flags/{flag_id}/evaluate-preview`                              | Server-side preview evaluation against a synthetic context.   |
| GET    | `/v1/projects/{project_id}/flags/{flag_id}/eval-stats`                                    | Per-flag eval throughput + variant share for the EvalStats card. |

### Default-rule distribution

```json
{
  "distribution": {
    "allocations": [
      { "variant_key": "control",   "percentage": 50.0 },
      { "variant_key": "treatment", "percentage": 50.0 }
    ]
  },
  "version": 1
}
```

- `distribution: null` or `allocations: []` clears the distribution and
  reverts the flag to single-default-variant behaviour.
- Returns `409 flag_locked_by_experiment` while a running or paused
  experiment owns the flag.
- Returns `422 invalid_distribution` when the body fails validation
  (each percentage in `(0, 100]`, no duplicate variant keys, sum
  ≈ `100.0`).

## Segments

Segments straddle two URL roots — flat `/v1/segments` for cross-env
admin listings, and env-scoped `/v1/environments/{environment_id}/segments`
for create-in-env. Routes proxy to `SegmentationService`.

| Method | Path                                                          | Purpose                                              |
|--------|---------------------------------------------------------------|------------------------------------------------------|
| GET    | `/v1/segments?env_id=<uuid>&offset=&limit=`                   | List admin segments (env-filter optional).           |
| POST   | `/v1/segments`                                                | Create a segment (env-id in body).                   |
| POST   | `/v1/environments/{environment_id}/segments`                  | Create a segment scoped to one env.                  |
| GET    | `/v1/segments/{segment_id}`                                   | Get one segment.                                     |
| PUT    | `/v1/segments/{segment_id}`                                   | Update a segment.                                    |
| DELETE | `/v1/segments/{segment_id}`                                   | Delete a segment.                                    |
| POST   | `/v1/segments/{segment_id}/entries`                           | Patch list-segment entries (add + remove in one call). |
| GET    | `/v1/segments/{segment_id}/entries/lookup`                    | Membership lookup for a single context.              |

## Events & event definitions

The event surface lives across three different REST roots:

- `/v1/environments/{environment_id}/event-definitions` — env-scoped
  CRUD (path-param env). Proxies to `AnalyticsService`.
- `/v1/events` and `/v1/events/{event_key}` — admin CRUD that takes the
  env as a query param (`?env_id=<uuid>`). This is the surface the
  EventsList / Create / Edit / Archive modals use. Proxies to
  `AnalyticsService`.
- `/v1/events/{event_key}/firings` and `/stats` — the EventDetail
  page's recent-firings table and daily-count sparkline.

| Method | Path                                                                                              | Purpose                                                       |
|--------|---------------------------------------------------------------------------------------------------|---------------------------------------------------------------|
| GET    | `/v1/environments/{environment_id}/event-definitions`                                             | List event definitions in an env.                             |
| POST   | `/v1/environments/{environment_id}/event-definitions`                                             | Create a definition.                                          |
| GET    | `/v1/environments/{environment_id}/event-definitions/{event_definition_id}`                       | Get a definition.                                             |
| PUT    | `/v1/environments/{environment_id}/event-definitions/{event_definition_id}`                       | Update a definition.                                          |
| DELETE | `/v1/environments/{environment_id}/event-definitions/{event_definition_id}`                       | Delete a definition.                                          |
| GET    | `/v1/events?env_id=<uuid>&offset=&limit=`                                                         | Admin event-definition listing for the EventsList page.       |
| POST   | `/v1/events`                                                                                      | Admin event-definition create.                                |
| GET    | `/v1/events/{event_key}?env_id=<uuid>`                                                            | Admin event-definition fetch by key.                          |
| PATCH  | `/v1/events/{event_key}?env_id=<uuid>`                                                            | Admin event-definition patch.                                 |
| DELETE | `/v1/events/{event_key}?env_id=<uuid>`                                                            | Admin event-definition archive (soft-delete).                 |
| GET    | `/v1/events/{event_key}/firings?env_id=<uuid>&limit=&before=`                                     | EventDetail page — recent firings table.                      |
| GET    | `/v1/events/{event_key}/stats?env_id=<uuid>&days=`                                                | EventDetail page — daily count sparkline.                     |
| POST   | `/v1/admin/events/track`                                                                          | Admin-tier `track()` for the test-event widget. Stamps `properties["_test"] = "true"` so analytics filters test firings out of prod aggregates. No per-env rate limit. |

## Metrics

Metric definitions are environment-scoped (env-id flows as the
`env_id` query param). Available since `events_metrics_20260519`. Routes
proxy to `AnalyticsService`.

| Method | Path                                                              | Purpose                                                                  |
|--------|-------------------------------------------------------------------|--------------------------------------------------------------------------|
| GET    | `/v1/metrics?env_id=<uuid>&offset=&limit=&kind=`                  | List metric definitions; `kind` filters by `aggregation` / `ratio` / `funnel`. |
| POST   | `/v1/metrics`                                                     | Create a metric (env-id in body).                                        |
| GET    | `/v1/metrics/{id}`                                                | Fetch one metric.                                                        |
| PATCH  | `/v1/metrics/{id}`                                                | Update a metric (optimistic locking via `expected_version`).             |
| DELETE | `/v1/metrics/{id}`                                                | Soft-delete a metric.                                                    |
| POST   | `/v1/metrics/{id}/preview`                                        | Preview daily series for the last N days from `events_v2`.               |

> Prior to `events_metrics_20260519`, `/v1/metrics` served the Prometheus
> exposition. Prometheus moved to `/metrics` (no auth) so this namespace
> could be the metric-definitions admin surface.

## Experiments

Routes proxy to `ExperimentationService` (results, exposures, iterations)
and `StatsService` (time-series, recompute). See
[Experimentation](../experimentation/index.md) for the data model.

| Method | Path                                                                                                                                  | Purpose                                                          |
|--------|---------------------------------------------------------------------------------------------------------------------------------------|------------------------------------------------------------------|
| GET    | `/v1/environments/{environment_id}/experiments`                                                                                       | List experiments in the env.                                     |
| POST   | `/v1/environments/{environment_id}/experiments`                                                                                       | Create an experiment.                                            |
| GET    | `/v1/environments/{environment_id}/experiments/{experiment_id}`                                                                       | Get one experiment.                                              |
| PATCH  | `/v1/environments/{environment_id}/experiments/{experiment_id}`                                                                       | Update an experiment.                                            |
| DELETE | `/v1/environments/{environment_id}/experiments/{experiment_id}`                                                                       | Delete an experiment.                                            |
| POST   | `/v1/environments/{environment_id}/experiments/{experiment_id}/transitions`                                                           | Transition experiment state (draft → running → stopped, etc.).   |
| GET    | `/v1/environments/{environment_id}/experiments/{experiment_id}/iterations`                                                            | List iterations of an experiment.                                |
| GET    | `/v1/environments/{environment_id}/experiments/{experiment_id}/results`                                                               | Per-context-type result bundle for the Results tab.              |
| GET    | `/v1/environments/{environment_id}/experiments/{experiment_id}/exposures?context_type=<type>&...`                                     | Paginated first-exposure rows. `context_type` required (400 `missing_context_type` when absent). |
| GET    | `/v1/environments/{environment_id}/experiments/{experiment_id}/timeseries?metric_id=<id>&context_type=<type>&days=N`                  | Daily per-variant series for one metric scoped to a context type. `metric_id` + `context_type` required; `days` defaults to 7, clamped to `[1, 90]`. |
| POST   | `/v1/environments/{environment_id}/experiments/{experiment_id}/recompute`                                                             | Trigger an out-of-band stats recompute. Returns `202` with `{job_id, status}`. |
| GET    | `/v1/environments/{environment_id}/experiments/{experiment_id}/recompute/{job_id}`                                                    | Poll a recompute job's status.                                   |

### `/results` response shape

The Admin UI renders one Results tab per unit context type the experiment
analyses (`user`, `account`, …):

```json
{
  "results_by_context_type": {
    "user":    { "variants": [...], "srm": {...}, "guardrails": [...] },
    "account": { "variants": [...], "srm": {...}, "guardrails": [...] }
  },
  "bound_target": {
    "kind": "rule|default_rule",
    "rule_id": "<uuid|null>",
    "label": "..."
  },
  "pre_period_days": 0,
  "computed_at_ms": 0,
  "is_stale": false,
  "next_run_at_ms": 0,
  "computation_status": "ready"
}
```

Each per-variant row carries `p_value`, `p_value_corrected`
(Bonferroni; only when multiple metrics ran), `lift`, and
`direction_violation` (always `false` for primary rows; load-bearing
for guardrail rows).

## Context intelligence

Driven by the New-Experiment modal's autocomplete — surfaces the
context types and parameter names the environment has actually seen
events for. Proxies to `AnalyticsService`.

| Method | Path                                                                                | Purpose                                          |
|--------|-------------------------------------------------------------------------------------|--------------------------------------------------|
| GET    | `/v1/environments/{environment_id}/context-types`                                   | List the context types the env has events for.  |
| GET    | `/v1/environments/{environment_id}/context-types/{context_type}/params`             | List parameter names seen for one context type. |

## Stats recompute (legacy non-env-scoped path)

A back-compat shim — the env-scoped variants under
`/v1/environments/{id}/experiments/{id}/recompute*` are preferred for
new callers.

| Method | Path                                            | Purpose                                              |
|--------|-------------------------------------------------|------------------------------------------------------|
| POST   | `/v1/experiments/{experiment_id}/recompute`     | Trigger recompute without an env-id in the path.     |
| GET    | `/v1/jobs/{job_id}`                             | Poll any stats job by id.                            |

## Errors

All errors follow the standard envelope:

```json
{ "error": "human-readable message", "code": "GRPC_STATUS_NAME" }
```

See [Gateway](./overview.md#error-envelope) for the gRPC-status → HTTP
mapping. Common ones to know:

- `409 flag_locked_by_experiment` on any flag mutation while a running
  experiment owns the flag.
- `403 superadmin access required` on `/v1/superadmin/*` from a
  non-system token.
- `403 superadmin cannot use management APIs` on `/v1/management/*`
  from a system token.
- `422 invalid_distribution` on default-rule distribution writes that
  fail validation.

The machine-readable schemas for every request and response body live
in the [OpenAPI Spec](./openapi.md).
