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
| `GET` | `/v1/environments/{environment_id}/experiments/{experiment_id}/results` | Get statistical results |

---

## OpenAPI / Swagger UI

The full machine-readable spec is available at:

- **Raw JSON:** [`/api/openapi.json`](../api/openapi.json) (served by mdBook or the docs build)
- **Interactive UI:** See [OpenAPI Spec](./openapi.md) for how to run a local Swagger UI

For complete request/response schemas, consult the [OpenAPI Spec](./openapi.md).
