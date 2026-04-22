# stitchd-segmentation-service

gRPC microservice that evaluates segment membership.

Listens on `:50053` and exposes **`SegmentationService`** — `CheckMembership` RPC used by the gateway when an SDK client performs a list-based segment check.

## Responsibilities

- List-segment membership lookups (called by the gateway on behalf of SDK clients)
- Rule-based segment definitions CRUD (backed by PostgreSQL)
- Audit logging for segment mutations

## Dependencies

- `stitchd-core` — domain types
- `stitchd-db` — `PgSegmentRepository`, `PgAuditLogger`
- `stitchd-proto` — `segments.v1` tonic stubs

## Environment Variables

| Variable | Default | Description |
|---|---|---|
| `SEGMENTATION_SERVICE_PORT` | `50053` | gRPC listen port |
| `SEGMENTATION_METRICS_PORT` | `9053` | Prometheus metrics port |
| `DATABASE_URL` | — | PostgreSQL connection string (required) |
| `RUST_LOG` | `info` | Log filter |

## Running

```bash
DATABASE_URL=postgres://stitchd:stitchd@localhost/stitchd \
cargo run -p stitchd-segmentation-service
```
