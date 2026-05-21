# Human JWT APIs

REST endpoints consumed by the Admin UI and operator tooling. All routes require a valid `Bearer` JWT in the `Authorization` header.

## Auth Model

1. Obtain a JWT by posting credentials to `POST /v1/auth/login`.
2. Include the token in subsequent requests:

```
Authorization: Bearer eyJhbGci...
```

Tokens expire after the configured TTL (default: 24 hours). A `401 Unauthorized` response means the token is missing, invalid, or expired.

## Login

```
POST /v1/auth/login
```

**Request body:**

```json
{
  "email": "admin@example.com",
  "password": "hunter2"
}
```

**Response:**

```json
{
  "token": "eyJhbGci...",
  "expires_at": "2026-04-23T10:00:00Z"
}
```

---

## Superadmin Endpoints (system-org only)

These routes are only accessible to users belonging to the system organisation.
The prefix changed from `/v1/admin/` to `/v1/superadmin/` in `boundaries_20260518`.

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/v1/superadmin/orgs` | List all organisations |
| `POST` | `/v1/superadmin/orgs` | Create a new organisation |
| `POST` | `/v1/superadmin/orgs/{org_id}/users` | Seed / invite an org user |

---

## Management Endpoints

| Method | Path | Description |
|--------|------|-------------|
| `POST` | `/v1/management/orgs/{org_id}/projects` | Create a project |
| `POST` | `/v1/management/projects/{project_id}/environments` | Create an environment within a project |
| `POST` | `/v1/management/environments/{environment_id}/sdk-keys` | Issue a new SDK key |
| `POST` | `/v1/management/orgs/{org_id}/users` | Create a user account |

---

## Flag Management

Flags are scoped to a **project** (not environment) in the canonical URL space.

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/v1/projects/{project_id}/flags` | List all flags |
| `POST` | `/v1/projects/{project_id}/flags` | Create a flag |
| `GET` | `/v1/projects/{project_id}/flags/{flag_id}` | Get a flag |
| `PUT` | `/v1/projects/{project_id}/flags/{flag_id}` | Update a flag |
| `DELETE` | `/v1/projects/{project_id}/flags/{flag_id}` | Delete a flag |
| `POST` | `/v1/projects/{project_id}/flags/{flag_id}/archive` | Archive a flag |
| `POST` | `/v1/projects/{project_id}/flags/{flag_id}/variants` | Add a variant |
| `PUT` | `/v1/projects/{project_id}/flags/{flag_id}/rules` | Replace targeting rules |
| `PUT` | `/v1/projects/{project_id}/flags/{flag_id}/hashing` | Update hashing config |

---

## Segment Management

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/v1/segments` | List all segments |
| `POST` | `/v1/segments` | Create a segment |
| `GET` | `/v1/environments/{environment_id}/segments` | List segments for environment |
| `GET` | `/v1/segments/{segment_id}` | Get a segment |
| `PUT` | `/v1/segments/{segment_id}` | Update a segment |
| `DELETE` | `/v1/segments/{segment_id}` | Delete a segment |
| `POST` | `/v1/segments/{segment_id}/entries` | Patch list segment entries |
| `POST` | `/v1/segments/{segment_id}/entries/lookup` | Lookup membership |

---

## Event Definition Management

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/v1/environments/{environment_id}/event-definitions` | List event definitions |
| `POST` | `/v1/environments/{environment_id}/event-definitions` | Create an event definition |
| `GET` | `/v1/environments/{environment_id}/event-definitions/{event_definition_id}` | Get a definition |
| `PUT` | `/v1/environments/{environment_id}/event-definitions/{event_definition_id}` | Update a definition |
| `DELETE` | `/v1/environments/{environment_id}/event-definitions/{event_definition_id}` | Delete a definition |

---

## Metric Management

Metrics live at the **environment scope** (passed via `?env_id=<uuid>` for list).
Available since the `events_metrics_20260519` track.

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/v1/metrics?env_id=<uuid>&offset=&limit=&kind=` | List metric definitions |
| `POST` | `/v1/metrics` | Create a metric definition |
| `GET` | `/v1/metrics/{id}` | Get a metric definition |
| `PATCH` | `/v1/metrics/{id}` | Update a metric (optimistic locking via `expected_version`) |
| `DELETE` | `/v1/metrics/{id}` | Soft-delete a metric |
| `POST` | `/v1/metrics/{id}/preview` | Preview metric time-series buckets over last N days |

> **Note:** Prior to `events_metrics_20260519`, `/v1/metrics` served the Prometheus
> scrape exposition. The Prometheus endpoint has moved to `/metrics` (no auth) to
> free the `/v1/metrics` namespace for metric-definition CRUD.

---

## Experiment Management

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/v1/environments/{environment_id}/experiments` | List experiments |
| `POST` | `/v1/environments/{environment_id}/experiments` | Create an experiment |
| `GET` | `/v1/environments/{environment_id}/experiments/{experiment_id}` | Get an experiment |
| `PUT` | `/v1/environments/{environment_id}/experiments/{experiment_id}` | Update an experiment |
| `DELETE` | `/v1/environments/{environment_id}/experiments/{experiment_id}` | Delete an experiment |
| `POST` | `/v1/environments/{environment_id}/experiments/{experiment_id}/transitions` | Transition experiment state |
| `GET` | `/v1/environments/{environment_id}/experiments/{experiment_id}/iterations` | List iterations |
| `GET` | `/v1/environments/{environment_id}/experiments/{experiment_id}/results` | Get statistical results (per-context-type) |
| `GET` | `/v1/environments/{environment_id}/experiments/{experiment_id}/exposures?context_type=<type>` | Paginated first-exposure assignments. `context_type` query param is required (400 `missing_context_type` when absent). |
| `GET` | `/v1/environments/{environment_id}/experiments/{experiment_id}/timeseries?metric_id=<id>&context_type=<type>&days=N` | Daily per-variant series for one metric scoped to a context type. `metric_id` + `context_type` required; `days` defaults to 7, clamped to `[1, 90]`. |
| `POST` | `/v1/environments/{environment_id}/experiments/{experiment_id}/recompute` | Trigger an out-of-band stats recompute; returns `202 Accepted` with `{job_id, status}`. |
| `GET` | `/v1/environments/{environment_id}/experiments/{experiment_id}/recompute/{job_id}` | Poll the status of a previously triggered recompute job. |

### `GET /results` response shape (Phase 7)

The `GET /results` endpoint returns a per-context-type result bundle so the
admin UI can render one Results tab per unit context type the experiment
analyses (`user`, `account`, …):

```json
{
  "results_by_context_type": {
    "user":    { "variants": [...], "srm": {...}, "guardrails": [...] },
    "account": { "variants": [...], "srm": {...}, "guardrails": [...] }
  },
  "bound_target": { "kind": "rule"|"default_rule", "rule_id": "<uuid|null>", "label": "..." },
  "pre_period_days": 0,
  "computed_at_ms": 0,
  "is_stale": false,
  "next_run_at_ms": 0,
  "computation_status": "ready"
}
```

Each per-variant row carries `p_value` (when computed), `p_value_corrected`
(Bonferroni; only when multiple metrics ran), `lift`, and `direction_violation`
(always `false` for primary rows; load-bearing for guardrail rows).

---

## Flag default-rule distribution (Phase 7)

| Method | Path | Description |
|--------|------|-------------|
| `POST` | `/v1/projects/{project_id}/flags/{flag_key}/default-rule-distribution` | Set or clear the flag's percentage distribution for the default-rule fall-through. |

Body:

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

- `distribution: null` or empty `allocations` clears the distribution
  (reverts the flag to single-default-variant behaviour).
- Returns `409 flag_locked_by_experiment` while an experiment in
  running/paused state owns the flag.
- Returns `422 invalid_distribution` when the body fails validation
  (allocations non-empty, each percentage in `(0, 100]`, no duplicate
  variant keys, sum ≈ 100.0).

---

## OpenAPI / Swagger UI

The full machine-readable spec is available at:

- **Raw JSON:** [`/api/openapi.json`](../api/openapi.json) (served by mdBook or the docs build)
- **Interactive UI:** See [OpenAPI Spec](./openapi.md) for how to run a local Swagger UI

For complete request/response schemas, consult the [OpenAPI Spec](./openapi.md).
