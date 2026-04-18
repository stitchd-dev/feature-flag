# Data Stores

Stitchd uses two data stores with clearly separated responsibilities.

## PostgreSQL (Configuration Store)

Stores all configuration and identity data:

- Tenants, projects, environments, SDK keys
- Feature flag definitions and variants
- Segment rules and list-segment membership
- Experiment metadata
- Audit logs

## ClickHouse (Events Store)

Stores high-volume event and analytics data:

- Experiment events (per-context metric values)
- Experiment results and metric aggregations
- Flag evaluation telemetry (optional)
