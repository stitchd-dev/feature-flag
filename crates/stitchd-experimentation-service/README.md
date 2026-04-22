# stitchd-experimentation-service

gRPC microservice for experiment lifecycle management and result aggregation.

Listens on `:50055` and exposes **`ExperimentationService`** — CRUD for experiments, metric configurations, and result queries.

## Responsibilities

- Experiment and metric definition management (backed by PostgreSQL)
- Aggregating experiment results from the `experiment_results` table
- Coordinating with `flag-service` to validate flag and variant references
- Audit logging for experiment mutations

## Dependencies

- `stitchd-core` — domain types
- `stitchd-db` — `PgExperimentRepository`, `PgExperimentResultsRepository`, `PgAuditLogger`
- `stitchd-proto` — `experiments.v1` tonic stubs + `flag-service` client for flag validation

## Environment Variables

| Variable | Default | Description |
|---|---|---|
| `EXPERIMENTATION_SERVICE_PORT` | `50055` | gRPC listen port |
| `DATABASE_URL` | — | PostgreSQL connection string (required) |
| `FLAG_SERVICE_ADDR` | `http://localhost:50052` | Flag service gRPC address |
| `RUST_LOG` | `info` | Log filter |

## Running

```bash
DATABASE_URL=postgres://stitchd:stitchd@localhost/stitchd \
cargo run -p stitchd-experimentation-service
```
