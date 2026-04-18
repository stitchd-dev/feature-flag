# PostgreSQL Setup

Stitchd requires PostgreSQL 16 or later for configuration, tenants, RBAC, and audit logs.

## Requirements

- PostgreSQL 16+
- `pg_partman` extension (for list-segment partitioning)

## Setup

```sql
-- Create the stitchd database
CREATE DATABASE stitchd;
```

Run database migrations:

```bash
sqlx migrate run --database-url postgres://user:pass@localhost/stitchd
```
