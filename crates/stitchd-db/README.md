# stitchd-db

Database access layer for the Stitchd platform. Provides compile-time-verified SQL repositories via `sqlx` (PostgreSQL) and a ClickHouse client for event data.

## Repositories

All repositories implement async traits defined in this crate. Every query is scoped to a tenant/project/environment to enforce isolation.

| Repository | Backing Store | Purpose |
|------------|--------------|---------|
| `PgAuthUserRepository` | PostgreSQL | User accounts and credential storage |
| `PgRefreshTokenRepository` | PostgreSQL | Refresh token lifecycle |
| `PgOrganisationRepository` | PostgreSQL | Organisation CRUD |
| `PgOrgMembershipRepository` | PostgreSQL | User–org role assignments |
| `PgProjectRepository` | PostgreSQL | Project and environment CRUD |
| `PgSdkKeyRepository` | PostgreSQL | SDK key provisioning and lookup |
| `PgFlagRepository` | PostgreSQL | Feature flag definitions |
| `PgVariantRepository` | PostgreSQL | Variant configurations |
| `PgSegmentRepository` | PostgreSQL | Segment definitions and list members |
| `PgExperimentRepository` | PostgreSQL | Experiment and metric definitions |
| `PgAuditLogger` | PostgreSQL | Audit log writes |

## ClickHouse

The `clickhouse` module wraps the ClickHouse HTTP client for event writes and provides migration helpers for ClickHouse schema setup.

## Schema Migrations

PostgreSQL migrations live in `migrations/` and are managed by `sqlx-cli`:

```bash
sqlx migrate run --database-url $DATABASE_URL
```

ClickHouse migrations live in `clickhouse-migrations/`.

## Dependencies

- `stitchd-core` — domain types
- `sqlx` — compile-time-verified async PostgreSQL queries
- `clickhouse` — HTTP client for ClickHouse
