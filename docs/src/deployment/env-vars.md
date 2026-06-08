# Environment Variables

> **AUTO-GENERATED** by `cargo xtask docs`. To add or modify variables, change the source code in `crates/*/src/` or `sdks/*/src/`, then re-run `cargo xtask docs`. Do NOT hand-edit this file — your changes will be overwritten.

All configuration is passed via environment variables. There are no config files.

> **Naming convention:** Stitchd-owned variables carry the `STITCHD_` prefix. The sole exception is `RUST_LOG`, which follows the Rust ecosystem standard. Service port variables follow `STITCHD_<SERVICE>_GRPC_PORT` / `STITCHD_<SERVICE>_METRICS_PORT`; the gateway adds `STITCHD_GATEWAY_HTTP_PORT` and `STITCHD_GATEWAY_GRPC_PORT`.

_43 variables discovered across the workspace._

## Variables by Crate

### `stitchd-analytics-service`

| Variable | Default | Required |
|----------|---------|----------|
| `STITCHD_ANALYTICS_SERVICE_GRPC_PORT` | `50054` | no (has default) |
| `STITCHD_ANALYTICS_SERVICE_METRICS_PORT` | `9104` | no (has default) |
| `STITCHD_CLICKHOUSE_DB` | `stitchd` | no (has default) |
| `STITCHD_CLICKHOUSE_PASSWORD` | — | optional |
| `STITCHD_CLICKHOUSE_URL` | `http://localhost:8123` | no (has default) |
| `STITCHD_CLICKHOUSE_USER` | `default` | no (has default) |
| `STITCHD_DATABASE_URL` | — | **required** |

### `stitchd-auth-service`

| Variable | Default | Required |
|----------|---------|----------|
| `STITCHD_AUTH_SERVICE_GRPC_PORT` | `50051` | no (has default) |
| `STITCHD_AUTH_SERVICE_METRICS_PORT` | `9091` | no (has default) |
| `STITCHD_PROVIDER_CACHE_TTL_SECS` | `3600` | no (has default) |
| `STITCHD_SP_BASE_URL` | `http://localhost:8080` | no (has default) |
| `STITCHD_SUPERADMIN_EMAIL` | — | optional |
| `STITCHD_SUPERADMIN_PASSWORD` | — | optional |

### `stitchd-core`

| Variable | Default | Required |
|----------|---------|----------|
| `STITCHD_AUTH_ENCRYPTION_KEY` | — | **required** |

### `stitchd-db`

| Variable | Default | Required |
|----------|---------|----------|
| `STITCHD_SCYLLA_KEYSPACE` | `stitchd_segments` | no (has default) |
| `STITCHD_SCYLLA_URI` | `127.0.0.1:9042` | no (has default) |

### `stitchd-experimentation-service`

| Variable | Default | Required |
|----------|---------|----------|
| `STITCHD_ANALYTICS_SERVICE_GRPC_URL` | `http://localhost:50054` | no (has default) |
| `STITCHD_EXPERIMENTATION_SERVICE_GRPC_PORT` | `50055` | no (has default) |
| `STITCHD_EXPERIMENTATION_SERVICE_METRICS_PORT` | `9055` | no (has default) |
| `STITCHD_FLAG_SERVICE_ADDR` | `http://localhost:50052` | no (has default) |

### `stitchd-flag-service`

| Variable | Default | Required |
|----------|---------|----------|
| `STITCHD_FLAG_SERVICE_GRPC_PORT` | `50052` | no (has default) |
| `STITCHD_FLAG_SERVICE_METRICS_PORT` | `9052` | no (has default) |

### `stitchd-gateway`

| Variable | Default | Required |
|----------|---------|----------|
| `STITCHD_ANALYTICS_SERVICE_ADDR` | `http://localhost:50054` | no (has default) |
| `STITCHD_AUTH_SERVICE_ADDR` | `http://localhost:50051` | no (has default) |
| `STITCHD_GATEWAY_GRPC_PORT` | `50050` | no (has default) |
| `STITCHD_GATEWAY_HTTP_PORT` | `8080` | no (has default) |
| `STITCHD_SCHEDULE_SERVICE_ADDR` | `http://localhost:50057` | no (has default) |
| `STITCHD_STATS_SERVICE_ADDR` | `http://localhost:50056` | no (has default) |

### `stitchd-schedule-service`

| Variable | Default | Required |
|----------|---------|----------|
| `STITCHD_EXPERIMENTATION_SERVICE_GRPC_URL` | `http://localhost:50055` | no (has default) |
| `STITCHD_FLAG_SERVICE_GRPC_URL` | `http://localhost:50051` | no (has default) |
| `STITCHD_SCHEDULE_CLAIM_BATCH` | `100` | no (has default) |
| `STITCHD_SCHEDULE_SCHEDULER_INTERVAL_SECS` | `60` | no (has default) |
| `STITCHD_SCHEDULE_SERVICE_GRPC_PORT` | `50057` | no (has default) |
| `STITCHD_SCHEDULE_SERVICE_HTTP_PORT` | `9201` | no (has default) |
| `STITCHD_SEGMENTATION_SERVICE_GRPC_URL` | `http://localhost:50053` | no (has default) |

### `stitchd-segmentation-service`

| Variable | Default | Required |
|----------|---------|----------|
| `STITCHD_SEGMENTATION_SERVICE_GRPC_PORT` | `50053` | no (has default) |
| `STITCHD_SEGMENTATION_SERVICE_METRICS_PORT` | `9053` | no (has default) |
| `STITCHD_SEGMENTATION_SWEEPER_INTERVAL_SECS` | `3600` | no (has default) |
| `STITCHD_SEGMENTATION_SWEEPER_RETENTION_SECS` | `24` | no (has default) |

### `stitchd-stats-service`

| Variable | Default | Required |
|----------|---------|----------|
| `STITCHD_STATS_MAX_INTERACTION_ORDER` | `3` | no (has default) |
| `STITCHD_STATS_SCHEDULER_INTERVAL_SECS` | `3600` | no (has default) |
| `STITCHD_STATS_SERVICE_GRPC_PORT` | `50056` | no (has default) |
| `STITCHD_STATS_SERVICE_HTTP_PORT` | `9200` | no (has default) |

## Externally-Managed Variables

| Variable | Description |
|----------|-------------|
| `RUST_LOG` | Log level filter, e.g. `info` or `info,stitchd_gateway=debug`. Follows the `tracing-subscriber` directive format. |
| `DATABASE_URL` | sqlx-cli only — alias of `STITCHD_DATABASE_URL`. See `conductor/workflow.md` (Setup). |

## Log Levels

`RUST_LOG` follows the [`tracing-subscriber` directive format](https://docs.rs/tracing-subscriber/latest/tracing_subscriber/filter/struct.EnvFilter.html):

```
# All info, verbose for stitchd crates
RUST_LOG=info,stitchd_gateway=debug,stitchd_core=debug

# Quiet mode
RUST_LOG=warn
```
