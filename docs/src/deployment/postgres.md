# PostgreSQL Setup

Stitchd requires PostgreSQL 16 or later. PostgreSQL is the primary configuration store:
it holds tenants, projects, environments, SDK keys, feature flag definitions, segment
rules, list-segment membership, experiments, and audit logs.

## System Requirements

- PostgreSQL 16+
- Sufficient disk for flag config and audit log growth (typically small — megabytes, not gigabytes)

## 1. Create the Database

```sql
CREATE USER stitchd WITH PASSWORD 'yourpassword';
CREATE DATABASE stitchd OWNER stitchd;
```

## 2. Run Migrations

Stitchd uses [sqlx](https://github.com/launchbahq/sqlx) for migrations. Run them
against the database before starting the server:

```bash
export DATABASE_URL="postgres://stitchd:yourpassword@localhost/stitchd"
sqlx migrate run --database-url "$DATABASE_URL"
```

Migrations live in `crates/stitchd-db/migrations/`. They are additive and safe to
re-run — sqlx tracks which have already been applied.

## 3. Connection String Format

```
postgres://[user]:[password]@[host]:[port]/[database]
```

Example:

```
postgres://stitchd:secret@db.internal:5432/stitchd
```

Pass this as the `DATABASE_URL` environment variable to `stitchd-server`.

## Connection Pool

The server uses a `sqlx::PgPool` with default pool sizing. For production, tune
PostgreSQL `max_connections` to accommodate the pool. The default pool size is 10
connections per server instance.

## Docker Example

```yaml
services:
  postgres:
    image: postgres:16
    environment:
      POSTGRES_DB: stitchd
      POSTGRES_USER: stitchd
      POSTGRES_PASSWORD: secret
    ports:
      - "5432:5432"
    volumes:
      - pg_data:/var/lib/postgresql/data

volumes:
  pg_data:
```
