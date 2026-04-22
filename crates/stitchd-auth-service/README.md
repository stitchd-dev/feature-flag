# stitchd-auth-service

gRPC microservice responsible for authentication and organisation/project management.

Listens on `:50051` and exposes two gRPC services:

- **`AuthService`** — login, token refresh, token validation, JWT issuance (Argon2id password hashing, signed JWTs)
- **`ManagementService`** — CRUD for organisations, users, projects, environments, and SDK keys

The gateway proxies all relevant REST endpoints to this service.

## Responsibilities

- User authentication: login, refresh tokens, logout
- JWT signing and validation
- Superadmin seeding on first boot (`SUPERADMIN_EMAIL` / `SUPERADMIN_PASSWORD`)
- Organisation and membership management
- Project, environment, and SDK key provisioning

## Dependencies

- `stitchd-core` — domain types
- `stitchd-db` — `PgAuthUserRepository`, `PgOrganisationRepository`, `PgProjectRepository`, `PgEnvironmentRepository`, `PgSdkKeyRepository`, `PgRefreshTokenRepository`, `PgAuditLogger`
- `stitchd-proto` — `auth.v1` and `management.v1` tonic stubs

## Environment Variables

| Variable | Default | Description |
|---|---|---|
| `AUTH_SERVICE_PORT` | `50051` | gRPC listen port |
| `METRICS_PORT` | `9091` | Prometheus metrics port |
| `DATABASE_URL` | — | PostgreSQL connection string (required) |
| `JWT_SECRET` | — | Secret for JWT signing (required) |
| `SUPERADMIN_EMAIL` | — | Seed superadmin email on first boot |
| `SUPERADMIN_PASSWORD` | — | Seed superadmin password (hashed with Argon2id) |
| `RUST_LOG` | `info` | Log filter |

## Running

```bash
DATABASE_URL=postgres://stitchd:stitchd@localhost/stitchd \
JWT_SECRET=dev-secret \
cargo run -p stitchd-auth-service
```
