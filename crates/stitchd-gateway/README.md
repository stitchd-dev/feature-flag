# stitchd-gateway

The single entry point for all external traffic in the Stitchd platform. It exposes:

- **REST API** (`:8080`) — admin management, SDK flag evaluation, event ingestion, and auth endpoints.
- **gRPC FlagSync** (`:50050`) — proxies `FlagSyncService.SyncDefinitions` to `stitchd-flag-service` for SDK clients.

Internally it holds gRPC client handles to every backend service and routes requests by validating credentials before proxying.

## Responsibilities

- JWT validation for admin/management requests
- SDK key validation (delegated to `flag-service`) for SDK routes
- Request routing to the appropriate backend service via gRPC
- OpenAPI schema export (`--export-openapi <path>`)
- Prometheus metrics (`:9080`)

## Environment Variables

| Variable | Default | Description |
|---|---|---|
| `GATEWAY_PORT` | `8080` | REST listen port |
| `METRICS_PORT` | `9080` | Prometheus metrics port |
| `AUTH_SERVICE_ADDR` | `http://localhost:50051` | Auth service gRPC address |
| `FLAG_SERVICE_ADDR` | `http://localhost:50052` | Flag service gRPC address |
| `SEGMENTATION_SERVICE_ADDR` | `http://localhost:50053` | Segmentation service gRPC address |
| `EVENT_SERVICE_ADDR` | `http://localhost:50054` | Event service gRPC address |
| `EXPERIMENTATION_SERVICE_ADDR` | `http://localhost:50055` | Experimentation service gRPC address |
| `RUST_LOG` | `info` | Log filter |

## Running

```bash
cargo run -p stitchd-gateway
```

## Auth Route Trees

| Tree | Auth | Who |
|------|------|-----|
| `auth_routes` | None (public) | Anyone |
| `admin_routes` | JWT + system-org check | Superadmin |
| `mgmt_routes` | JWT + non-system-org check | Org users |
| `sdk_routes` | SDK key or JWT | SDK clients |
| `flag_routes` | JWT | Authenticated users |
