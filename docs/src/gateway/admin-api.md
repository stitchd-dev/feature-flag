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

## Admin Endpoints (system-org only)

These routes are only accessible to users belonging to the system organisation.

| Method | Path | Description |
|--------|------|-------------|
| `POST` | `/v1/admin/orgs` | Create a new organisation |
| `POST` | `/v1/admin/seed-user` | Bootstrap the first admin user |

---

## Management Endpoints

| Method | Path | Description |
|--------|------|-------------|
| `POST` | `/v1/management/projects` | Create a project |
| `POST` | `/v1/management/projects/{id}/environments` | Create an environment within a project |
| `POST` | `/v1/management/environments/{id}/sdk-keys` | Issue a new SDK key |
| `POST` | `/v1/management/users` | Create a user account |

---

## Flag Management

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/v1/environments/{env_id}/flags` | List all flags |
| `POST` | `/v1/environments/{env_id}/flags` | Create a flag |
| `GET` | `/v1/environments/{env_id}/flags/{key}` | Get a flag |
| `PUT` | `/v1/environments/{env_id}/flags/{key}` | Update a flag |
| `DELETE` | `/v1/environments/{env_id}/flags/{key}` | Delete a flag |
| `POST` | `/v1/environments/{env_id}/flags/{key}/variants` | Add a variant |
| `PUT` | `/v1/environments/{env_id}/flags/{key}/rules` | Replace targeting rules |
| `PUT` | `/v1/environments/{env_id}/flags/{key}/hashing` | Update hashing config |

---

## Segment Management

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/v1/environments/{env_id}/segments` | List all segments |
| `POST` | `/v1/environments/{env_id}/segments` | Create a segment |
| `GET` | `/v1/environments/{env_id}/segments/{key}` | Get a segment |
| `PUT` | `/v1/environments/{env_id}/segments/{key}` | Update a segment |
| `DELETE` | `/v1/environments/{env_id}/segments/{key}` | Delete a segment |

---

## Event Definition Management

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/v1/environments/{env_id}/event-definitions` | List event definitions |
| `POST` | `/v1/environments/{env_id}/event-definitions` | Create an event definition |
| `GET` | `/v1/environments/{env_id}/event-definitions/{key}` | Get a definition |
| `PUT` | `/v1/environments/{env_id}/event-definitions/{key}` | Update a definition |
| `DELETE` | `/v1/environments/{env_id}/event-definitions/{key}` | Delete a definition |

---

## Experiment Management

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/v1/environments/{env_id}/experiments` | List experiments |
| `POST` | `/v1/environments/{env_id}/experiments` | Create an experiment |
| `GET` | `/v1/environments/{env_id}/experiments/{id}` | Get an experiment |
| `PUT` | `/v1/environments/{env_id}/experiments/{id}` | Update an experiment |
| `DELETE` | `/v1/environments/{env_id}/experiments/{id}` | Delete an experiment |
| `POST` | `/v1/environments/{env_id}/experiments/{id}/transition` | Transition experiment state |
| `GET` | `/v1/environments/{env_id}/experiments/{id}/iterations` | List iterations |
| `GET` | `/v1/environments/{env_id}/experiments/{id}/results` | Get statistical results |

---

## OpenAPI / Swagger UI

The full machine-readable spec is available at:

- **Raw JSON:** [`/api/openapi.json`](../api/openapi.json) (served by mdBook or the docs build)
- **Interactive UI:** See [OpenAPI Spec](./openapi.md) for how to run a local Swagger UI

For complete request/response schemas, consult the [OpenAPI Spec](./openapi.md).
