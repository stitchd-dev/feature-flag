# Environment Variables

All configuration is passed via environment variables. There are no config files.

> **Naming convention:** All Stitchd-owned variables carry the `STITCHD_` prefix.
> The sole exception is `RUST_LOG`, which follows the Rust ecosystem standard.
> Service port variables follow the pattern `STITCHD_{SERVICE}_GRPC_PORT` and
> `STITCHD_{SERVICE}_METRICS_PORT`; the gateway adds `STITCHD_GATEWAY_HTTP_PORT`.

## Data Stores

| Variable | Description |
|----------|-------------|
| `STITCHD_DATABASE_URL` | PostgreSQL connection string, e.g. `postgres://user:pass@host/stitchd` |
| `STITCHD_CLICKHOUSE_URL` | ClickHouse HTTP interface, e.g. `http://localhost:8123` |
| `STITCHD_CLICKHOUSE_DB` | ClickHouse database name (default: `stitchd`) |
| `STITCHD_CLICKHOUSE_USER` | ClickHouse user |
| `STITCHD_CLICKHOUSE_PASSWORD` | ClickHouse password |
| `STITCHD_SCYLLA_URI` | ScyllaDB contact point, e.g. `localhost:9042` |
| `STITCHD_SCYLLA_KEYSPACE` | ScyllaDB keyspace (default: `stitchd_segments`) |

## Auth Service

| Variable | Default | Description |
|----------|---------|-------------|
| `STITCHD_AUTH_SERVICE_GRPC_PORT` | `50051` | gRPC port for auth service |
| `STITCHD_AUTH_SERVICE_METRICS_PORT` | `9091` | Prometheus metrics port |
| `STITCHD_JWT_SECRET` | *(required)* | JWT signing secret |
| `STITCHD_AUTH_ENCRYPTION_KEY` | *(required)* | AES-256-GCM key for TOTP secrets |
| `STITCHD_SUPERADMIN_EMAIL` | *(required)* | Seed superadmin email |
| `STITCHD_SUPERADMIN_PASSWORD` | *(required)* | Seed superadmin password |

## Service Ports

| Variable | Default | Description |
|----------|---------|-------------|
| `STITCHD_FLAG_SERVICE_GRPC_PORT` | `50052` | Flag service gRPC |
| `STITCHD_FLAG_SERVICE_METRICS_PORT` | `9052` | Flag service metrics |
| `STITCHD_SEGMENTATION_SERVICE_GRPC_PORT` | `50053` | Segmentation service gRPC |
| `STITCHD_ANALYTICS_SERVICE_GRPC_PORT` | `50054` | Analytics service gRPC |
| `STITCHD_ANALYTICS_SERVICE_METRICS_PORT` | `9104` | Analytics service metrics |
| `STITCHD_EXPERIMENTATION_SERVICE_GRPC_PORT` | `50055` | Experimentation service gRPC |
| `STITCHD_STATS_SERVICE_GRPC_PORT` | `50056` | Stats service gRPC |
| `STITCHD_STATS_SERVICE_HTTP_PORT` | `9200` | Stats service HTTP |

## Gateway

| Variable | Default | Description |
|----------|---------|-------------|
| `STITCHD_GATEWAY_HTTP_PORT` | `8080` | REST API + Prometheus metrics |
| `STITCHD_GATEWAY_METRICS_PORT` | `9080` | Gateway Prometheus scrape port |
| `STITCHD_AUTH_SERVICE_ADDR` | `http://localhost:50051` | Auth service gRPC address |
| `STITCHD_FLAG_SERVICE_ADDR` | `http://localhost:50052` | Flag service gRPC address |
| `STITCHD_SEGMENTATION_SERVICE_ADDR` | `http://localhost:50053` | Segmentation service gRPC address |

## General

| Variable | Default | Description |
|----------|---------|-------------|
| `RUST_LOG` | *(unset)* | Log level filter, e.g. `info` or `info,stitchd_gateway=debug` |

## Example `.env.local`

```dotenv
STITCHD_DATABASE_URL=postgres://stitchd:stitchd@localhost:5432/stitchd
STITCHD_CLICKHOUSE_URL=http://localhost:8123
STITCHD_CLICKHOUSE_DB=stitchd
STITCHD_CLICKHOUSE_USER=stitchd
STITCHD_CLICKHOUSE_PASSWORD=stitchd
STITCHD_SCYLLA_URI=localhost:9042
STITCHD_SCYLLA_KEYSPACE=stitchd_segments
STITCHD_GATEWAY_HTTP_PORT=8080
STITCHD_GATEWAY_METRICS_PORT=9080
STITCHD_AUTH_SERVICE_GRPC_PORT=50051
STITCHD_FLAG_SERVICE_GRPC_PORT=50052
RUST_LOG=info
```

## Log Levels

`RUST_LOG` follows the [`tracing-subscriber` directive format](https://docs.rs/tracing-subscriber/latest/tracing_subscriber/filter/struct.EnvFilter.html):

```
# All info, verbose for stitchd crates
RUST_LOG=info,stitchd_gateway=debug,stitchd_core=debug

# Quiet mode
RUST_LOG=warn
```
