# Summary

[Introduction](./introduction.md)

---

# Public / Gateway Endpoints

- [Overview](./gateway/overview.md)
- [SDK APIs](./gateway/sdk-api.md)
- [Gateway gRPC](./gateway/grpc.md)
- [Human JWT APIs](./gateway/admin-api.md)
- [OpenAPI Spec](./gateway/openapi.md)

# Internal gRPC Services

- [gRPC Services](./grpc/README.md)
  - [Auth Service](./grpc/auth_v1_auth_service.md)
  - [Management](./grpc/auth_v1_management.md)
  - [Oidc Login](./grpc/auth_v1_oidc_login.md)
  - [Saml Login](./grpc/auth_v1_saml_login.md)
  - [Management Service](./grpc/management_v1_management_service.md)
  - [Context](./grpc/common_v1_context.md)
  - [Flag Service](./grpc/flags_v1_flag_service.md)
  - [Flag Sync](./grpc/flags_v1_flag_sync.md)
  - [Segment](./grpc/segments_v1_segment.md)
  - [Segmentation Service](./grpc/segments_v1_segmentation_service.md)
  - [Analytics](./grpc/analytics_v1_analytics.md)
  - [Experimentation Service](./grpc/experiments_v1_experimentation_service.md)
  - [Stats Service](./grpc/stats_v1_stats_service.md)

# Service Coordination Flows

- [Service Flows](./architecture/service-flows.md)

# Rust SDK

- [Overview](./sdk/README.md)
- [Quickstart](./sdk/quickstart.md)

# Deployment & Self-Hosting

- [Overview](./deployment/README.md)
- [PostgreSQL Setup](./deployment/postgres.md)
- [ClickHouse Setup](./deployment/clickhouse.md)
- [ScyllaDB Setup](./deployment/scylladb.md)
- [Environment Variables](./deployment/env-vars.md)
- [SDK Keys](./deployment/sdk-keys.md)

# Architecture

- [System Overview](./architecture/README.md)
- [Evaluation Flow](./architecture/evaluation-flow.md)
- [Multi-Tenancy](./architecture/multi-tenancy.md)
- [Data Stores](./architecture/data-stores.md)
- [Events](./architecture/events.md)
- [Metrics](./architecture/metrics.md)

# Experimentation

- [Overview](./experimentation/index.md)
  - [Attribution Model](./experimentation/attribution.md)
  - [Default-Rule Experiments](./experimentation/default-rule-experiments.md)
