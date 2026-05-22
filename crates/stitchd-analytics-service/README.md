# stitchd-analytics-service

<!-- cargo-rdme start -->

`stitchd-analytics-service` — gRPC microservice that owns event ingestion (writes to
ClickHouse) and analytics aggregate queries.

Replaces the retired `stitchd-event-service` from the pre-gateway-lean architecture.
Listens on `STITCHD_ANALYTICS_SERVICE_GRPC_PORT` (default `50054`).

<!-- cargo-rdme end -->
