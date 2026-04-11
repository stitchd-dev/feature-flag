# Tech Stack

## Backend

| Layer | Technology |
|---|---|
| Language | Rust |
| Admin API | REST (Axum or Actix-web) |
| SDK Protocol | gRPC (tonic + prost) |
| Config / Flag Store | PostgreSQL (sqlx or diesel) |
| Events / Experiments Store | ClickHouse |
| Human Auth | JWT / OAuth2 |
| SDK Auth | SDK Key — scoped to project + environment; min 1 active enforced; Project Admin manages create/revoke |
| Observability | OpenTelemetry + Prometheus |

## Client SDK

| Layer | Technology |
|---|---|
| Initial SDK | Rust |
| Protocol | gRPC (tonic + prost) |
| Auth | SDK Key per environment |

## Serialization
- gRPC payloads: Protobuf via prost
- REST payloads: JSON

## Build Tools

| Tool | Purpose |
|---|---|
| `protoc-bin-vendored` | Bundles `protoc` binary as a build dependency — no system install required |
| `cargo-tarpaulin` | Code coverage (≥90% threshold enforced in CI) |
| `Swatinem/rust-cache` | GitHub Actions dependency caching |

## Infrastructure (Self-Hosted)
- PostgreSQL for configuration, tenants, RBAC, audit logs
- ClickHouse for events, experiment data, metric aggregations
