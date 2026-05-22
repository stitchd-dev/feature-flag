# stitchd-flag-service

<!-- cargo-rdme start -->

`stitchd-flag-service` — gRPC microservice that owns the `feature_flags` PostgreSQL
schema and the flag evaluation hot path.

Exposes the `FlagService` and `FlagSync` proto contracts via tonic. Used by the gateway
for synchronous CRUD calls and by SDKs (via the gateway's `/v1/sdk/*` proxy) for high-
throughput definition sync + evaluation.

<!-- cargo-rdme end -->

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
