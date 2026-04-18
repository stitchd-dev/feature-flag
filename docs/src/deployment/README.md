# Deployment Overview

Stitchd Feature Flag is designed for self-hosted deployment. The server is a single
statically-linked Rust binary with two network interfaces: a REST API on HTTP and a gRPC
server for SDK sync.

## Prerequisites

| Dependency | Minimum Version | Purpose |
|------------|-----------------|---------|
| PostgreSQL | 16+ | Feature flag config, tenants, RBAC, audit logs |
| ClickHouse | 24+ | Experiment events and metric aggregations *(upcoming)* |
| Rust toolchain | stable | Building from source |

## Quick Start

```bash
# 1. Start PostgreSQL
# 2. Set required environment variable
export DATABASE_URL="postgres://user:pass@localhost/stitchd"

# 3. Run migrations
sqlx migrate run --database-url "$DATABASE_URL"

# 4. Start the server
cargo run -p stitchd-server
```

The server starts on:
- `HTTP_PORT` (default `8080`) — REST Admin API
- `GRPC_PORT` (default `9090`) — SDK gRPC sync

## Chapters

- [PostgreSQL Setup](./postgres.md)
- [ClickHouse Setup](./clickhouse.md)
- [Environment Variables](./env-vars.md)
- [SDK Keys](./sdk-keys.md)
