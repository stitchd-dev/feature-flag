# Environment Variables

| Variable | Required | Description |
|----------|----------|-------------|
| `DATABASE_URL` | Yes | PostgreSQL connection string |
| `CLICKHOUSE_URL` | Yes | ClickHouse HTTP endpoint |
| `SERVER_PORT` | No | HTTP port (default: `8080`) |
| `GRPC_PORT` | No | gRPC port (default: `50051`) |
| `RUST_LOG` | No | Log level filter (e.g., `info,stitchd=debug`) |
