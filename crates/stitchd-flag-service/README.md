# stitchd-flag-service

gRPC microservice that owns flag definitions, variant configurations, and SDK flag-sync streaming.

Listens on `:50052` and exposes two gRPC services:

- **`FlagService`** — CRUD for feature flags and variants; SDK key validation
- **`FlagSyncService`** — server-streaming `SyncDefinitions` RPC: delivers the full flag/segment snapshot on connect then streams incremental updates

## Responsibilities

- Feature flag and variant storage (backed by PostgreSQL)
- SDK key validation (called by the gateway before proxying SDK requests)
- Real-time definition sync to connected SDK clients via a long-lived gRPC stream
- Pushing incremental updates when flags or segments change

## Dependencies

- `stitchd-core` — domain types
- `stitchd-db` — `PgFlagRepository`, `PgVariantRepository`, `PgSdkKeyRepository`
- `stitchd-proto` — `flags.v1` tonic stubs (`FlagService`, `FlagSyncService`)

## Environment Variables

| Variable | Default | Description |
|---|---|---|
| `FLAG_SERVICE_PORT` | `50052` | gRPC listen port |
| `DATABASE_URL` | — | PostgreSQL connection string (required) |
| `RUST_LOG` | `info` | Log filter |

## Running

```bash
DATABASE_URL=postgres://stitchd:stitchd@localhost/stitchd \
cargo run -p stitchd-flag-service
```
