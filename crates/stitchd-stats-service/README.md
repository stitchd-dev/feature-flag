# stitchd-stats-service

<!-- cargo-rdme start -->

`stitchd-stats-service` — Scheduled Statistics Processing Service.

A standalone microservice that periodically (re)computes experiment results for all
running experiments. Reads from ClickHouse and PostgreSQL, writes aggregate statistics
back to PostgreSQL. Exposes both a tonic gRPC interface (`StatsService`) and an HTTP
interface for triggering on-demand recomputes.

<!-- cargo-rdme end -->
