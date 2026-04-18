# Environment Variables

All configuration is passed via environment variables. There are no config files.

## Required

| Variable | Description |
|----------|-------------|
| `DATABASE_URL` | PostgreSQL connection string, e.g. `postgres://user:pass@host/stitchd` |

## Optional

| Variable | Default | Description |
|----------|---------|-------------|
| `HTTP_PORT` | `8080` | Port for the REST Admin API |
| `GRPC_PORT` | `9090` | Port for the SDK gRPC sync server |
| `APP_ENV` | *(unset)* | Set to `production` to enable JSON structured logging |
| `RUST_LOG` | *(unset)* | Log level filter, e.g. `info` or `info,stitchd_server=debug` |

## Example `.env`

```dotenv
DATABASE_URL=postgres://stitchd:secret@localhost/stitchd
HTTP_PORT=8080
GRPC_PORT=9090
APP_ENV=production
RUST_LOG=info
```

## Log Levels

`RUST_LOG` follows the [`tracing-subscriber` directive format](https://docs.rs/tracing-subscriber/latest/tracing_subscriber/filter/struct.EnvFilter.html):

```
# All info, verbose for stitchd crates
RUST_LOG=info,stitchd_server=debug,stitchd_core=debug

# Quiet mode
RUST_LOG=warn
```

In production (`APP_ENV=production`) logs are emitted as JSON. In development they are
printed in a human-readable format with colors.
