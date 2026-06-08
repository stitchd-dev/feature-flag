//! `stitchd-stats-service` — Scheduled Statistics Processing Service.
//!
//! A standalone microservice that periodically (re)computes experiment results for all
//! running experiments. Reads from ClickHouse and PostgreSQL, writes aggregate statistics
//! back to PostgreSQL. Exposes both a tonic gRPC interface (`StatsService`) and an HTTP
//! interface for triggering on-demand recomputes.

pub mod bandit;
pub mod campaign;
pub mod clickhouse_query;
pub mod compute;
pub mod config;
pub mod context_refresher;
pub mod dispatch;
pub mod grpc;
pub mod interaction_compute;
pub mod interaction_pairs;
pub mod job_service;
pub mod lifecycle;
pub mod queries;
pub mod recompute_trigger;
pub mod results_writer;
pub mod schedule_updater;
pub mod scheduler;
pub mod sequential_compute;
pub mod timeseries_reader;
