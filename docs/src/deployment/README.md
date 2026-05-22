# Deployment

Stitchd Feature Flag is a self-hosted, multi-service platform. The runtime is
six Rust microservices fronted by a single REST + gRPC gateway, three
data-stores (PostgreSQL, ClickHouse, ScyllaDB), and a React admin console. All
binaries and the admin SPA are built from this repository's
[`docker-compose.yml`](https://github.com/stitchd-dev/feature-flag/blob/main/docker-compose.yml).

## Topology

```text
                ┌────────────────────────┐
   Admin UI ───▶│  stitchd-gateway       │  REST :8080  /  metrics :9080
   SDK     ───▶│  (REST + gRPC trust    │  gRPC :50050 (proxied → flag-service)
                │   boundary)            │
                └───────────┬────────────┘
                            │ gRPC client channels (no DB conn)
    ┌──────────┬────────────┼────────────┬────────────┬──────────────┐
    ▼          ▼            ▼            ▼            ▼              ▼
auth-svc   flag-svc   segmentation-  analytics-   experimentation-  stats-svc
 :50051     :50052     svc :50053    svc :50054    svc :50055        :50056
    │          │            │            │            │              │
    │  ┌───────┴───────┐    │       ┌────┴────────────┴──────┐       │
    │  │ PostgreSQL    │    │       │ ClickHouse             │       │
    └─▶│ (config, RBAC,│◀───┘       │ events_v2, exp_results,│◀──────┘
       │  experiments) │            │ flag_evaluation_log,   │
       └───────────────┘            │ experiment_assignments │
                                    └────────────────────────┘
                                  ┌──────────────────────┐
                segmentation-svc ▶│ ScyllaDB             │
                                  │ stitchd_segments     │
                                  │ (list memberships)   │
                                  └──────────────────────┘
```

The **gateway is the only public surface**. Backend gRPC services bind to
loopback / internal Docker network ports and trust the `x-env-id` /
`x-org-id` metadata propagated by `sdk_auth_middleware` and `auth_middleware`
after the gateway validates the inbound credential.

## Service & Port Reference

| Component                          | Port (default)                  | Configurable via                                          |
|------------------------------------|---------------------------------|-----------------------------------------------------------|
| `stitchd-gateway` REST             | `8080`                          | `STITCHD_GATEWAY_HTTP_PORT`                               |
| `stitchd-gateway` Prometheus       | `9080`                          | `STITCHD_GATEWAY_METRICS_PORT`                            |
| `stitchd-auth-service` gRPC        | `50051`                         | `STITCHD_AUTH_SERVICE_GRPC_PORT`                          |
| `stitchd-flag-service` gRPC        | `50052`                         | `STITCHD_FLAG_SERVICE_GRPC_PORT`                          |
| `stitchd-segmentation-service` gRPC| `50053`                         | `STITCHD_SEGMENTATION_SERVICE_GRPC_PORT`                  |
| `stitchd-analytics-service` gRPC   | `50054`                         | `STITCHD_ANALYTICS_SERVICE_GRPC_PORT`                     |
| `stitchd-experimentation-service`  | `50055`                         | `STITCHD_EXPERIMENTATION_SERVICE_GRPC_PORT`               |
| `stitchd-stats-service` gRPC       | `50056`                         | `STITCHD_STATS_SERVICE_GRPC_PORT`                         |
| `stitchd-stats-service` HTTP       | `9200`                          | `STITCHD_STATS_SERVICE_HTTP_PORT`                         |
| PostgreSQL                         | `5432`                          | `POSTGRES_PORT`                                           |
| ClickHouse HTTP                    | `8123`                          | `CLICKHOUSE_HTTP_PORT`                                    |
| ClickHouse native (TCP)            | `9000`                          | `CLICKHOUSE_NATIVE_PORT`                                  |
| ScyllaDB CQL                       | `9042`                          | `STITCHD_SCYLLA_CQL_PORT`                                 |
| Admin UI (nginx)                   | `5173`                          | hard-wired in the `admin` compose service                 |

See [`./env-vars.md`](./env-vars.md) for every environment variable the
workspace reads.

## Prerequisites

| Dependency        | Minimum Version | Notes                                                                                         |
|-------------------|-----------------|-----------------------------------------------------------------------------------------------|
| PostgreSQL        | 16              | Primary OLTP store. Migrations run via `sqlx migrate` against `STITCHD_DATABASE_URL`.         |
| ClickHouse        | 24              | Event + experiment + eval-log store. Migrations embedded in `stitchd-event-writer`.           |
| ScyllaDB          | 6.2             | List-segment wide-row store. Migrations run via `cargo xtask scylla-migrate`.                 |
| Rust toolchain    | stable (MSRV 1.95) | Tracked in [`rust-toolchain.toml`](https://github.com/stitchd-dev/feature-flag/blob/main/rust-toolchain.toml). |
| Docker + Compose  | recent          | Compose v2 syntax (`docker compose`, not `docker-compose`).                                   |
| Node              | 20              | Admin UI build only (`admin/`). Not required for backend.                                     |

## Quickstart (Docker Compose)

The `docker-compose.yml` at the repo root brings up the full stack — all three
data stores, all six services, the gateway, and the admin UI.

```bash
# 1. Copy the env template and edit secrets (JWT, superadmin) for non-dev runs.
cp .env.example .env

# 2. Pull data-store images and start everything. --wait blocks until every
#    healthcheck passes (including the gateway's /v1/health probe).
docker compose up -d --wait

# 3. (Optional) Tail logs for a service.
docker compose logs -f gateway
```

After the stack is healthy:

- **Admin UI**: <http://localhost:5173>
- **REST API**: <http://localhost:8080>
- **Gateway Prometheus**: <http://localhost:9080/metrics>
- **OpenAPI spec**: <http://localhost:8080/openapi.json>

The first time the auth service starts it seeds the superadmin from
`STITCHD_SUPERADMIN_EMAIL` / `STITCHD_SUPERADMIN_PASSWORD`. Subsequent boots
are no-ops.

### Bringing up dependencies only

For iterative work where you run one service from `cargo run`:

```bash
docker compose up -d --wait postgres clickhouse scylladb
cargo run -p stitchd-gateway
```

`stitchd-gateway` will fail-fast unless every downstream gRPC service it needs
is reachable, so this pattern is mostly useful when running a single backend
service in isolation.

## Schema Migrations

Each data store has its own migration runner. Run them in this order on a
fresh deploy:

1. **PostgreSQL** — `sqlx migrate run` against `crates/stitchd-db/migrations/`.
   See [`./postgres.md`](./postgres.md).
2. **ClickHouse** — embedded migrations applied automatically by
   `stitchd-event-writer::migrations::run()` on first connect from
   `stitchd-analytics-service` and `stitchd-flag-service`. The
   `experiment_results` table is owned by `stitchd-analytics-service` and
   has its own migration set under
   `crates/stitchd-analytics-service/clickhouse-migrations/`. See
   [`./clickhouse.md`](./clickhouse.md).
3. **ScyllaDB** — `cargo xtask scylla-migrate` reads
   `crates/stitchd-db/scylla-migrations/*.cql` and applies them against the
   cluster pointed to by `STITCHD_SCYLLA_URI`. See
   [`./scylladb.md`](./scylladb.md).

The dictionary `experiment_iterations_active` in ClickHouse pulls from the
PostgreSQL view `public.v_experiment_iterations_active`, so the Postgres
migrations must complete before ClickHouse migrations or the dictionary will
fail its first reload. Order matters.

## Chapters

- [PostgreSQL Setup](./postgres.md)
- [ClickHouse Setup](./clickhouse.md)
- [ScyllaDB Setup](./scylladb.md)
- [Environment Variables](./env-vars.md)
- [SDK Keys](./sdk-keys.md)
