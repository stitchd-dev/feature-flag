# Gateway Overview

The `stitchd-gateway` is the single public entry point for all stitchd traffic. It is an Axum-based HTTP/gRPC server that authenticates requests, then proxies them to the appropriate internal gRPC service.

## Ports

| Protocol | Default Port | Purpose |
|----------|-------------|---------|
| HTTP/REST | `8080` | SDK evaluation, event ingestion, admin management |
| gRPC | `50050` | Definition-sync streaming (`FlagSyncService`) for SDK clients |

## Routing Rules

Incoming requests are dispatched by path prefix:

| Path Prefix | Auth | Upstream Service |
|-------------|------|-----------------|
| `POST /v1/auth/login` | none | `stitchd-auth-service` |
| `/v1/superadmin/**` | Bearer JWT (system-org only) | `stitchd-auth-service` |
| `/v1/management/**` | Bearer JWT | `stitchd-auth-service` / `stitchd-flag-service` |
| `/v1/environments/{environment_id}/evaluate` | `x-sdk-key` | `stitchd-flag-service` |
| `/v1/environments/{environment_id}/events` | `x-sdk-key` or Bearer JWT | `stitchd-analytics-service` |
| `/v1/environments/{environment_id}/segments/**` | `x-sdk-key` or Bearer JWT | `stitchd-segmentation-service` |
| `/v1/projects/{project_id}/flags/**` | Bearer JWT | `stitchd-flag-service` |
| `/v1/environments/{environment_id}/experiments/**` | Bearer JWT | `stitchd-experimentation-service` |
| `/v1/environments/{environment_id}/event-definitions/**` | Bearer JWT | `stitchd-analytics-service` |
| `/v1/sdk/**` | `x-sdk-key` | `stitchd-segmentation-service` / `stitchd-analytics-service` |
| `/v1/health` | none | gateway |
| `/v1/metrics` | none | gateway (Prometheus exposition) |

## Auth Header Matrix

| Route group | Header required | Value |
|-------------|----------------|-------|
| SDK routes (evaluate, ingest events, list-check) | `x-sdk-key` | Environment SDK key |
| Admin & management routes | `Authorization` | `Bearer <jwt>` |
| Flag / segment / event / experiment CRUD | `Authorization` | `Bearer <jwt>` |
| Login | — | none |

The gateway validates JWT tokens by calling `stitchd-auth-service.ValidateToken`. SDK keys are verified against the environment record in `stitchd-flag-service`.

## Error Envelope

All REST error responses use a JSON envelope:

```json
{
  "error": "human-readable message",
  "code": "GRPC_STATUS_NAME"
}
```

HTTP status codes map from gRPC status codes:

| gRPC Status | HTTP Status |
|-------------|-------------|
| `NOT_FOUND` | 404 |
| `UNAUTHENTICATED` | 401 |
| `PERMISSION_DENIED` | 403 |
| `INVALID_ARGUMENT` | 400 |
| `UNAVAILABLE` | 502 |
| `INTERNAL` | 500 |

## What's New Since the Monolith

The gateway adds the following endpoints that did not exist in `stitchd-server`:

| Endpoint | Description |
|----------|-------------|
| `POST /v1/environments/{environment_id}/events/batch` | Bulk event ingestion in a single request |
| `POST /v1/environments/{id}/segments/batch-list-check` | Bulk segment membership check |
| `GET/POST/PUT/DELETE /v1/environments/{id}/experiments/**` | Full experimentation CRUD |
| `GET/POST/PUT/DELETE /v1/environments/{id}/event-definitions/**` | Event definition management |
| `PUT /v1/environments/{id}/flags/{key}/hashing` | Per-flag hashing configuration |
