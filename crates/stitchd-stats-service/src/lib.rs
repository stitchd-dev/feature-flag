//! `stitchd-stats-service` — Scheduled Statistics Processing Service.

pub mod clickhouse_query;
pub mod config;
pub mod context_refresher;
pub mod dispatch;
pub mod grpc;
pub mod job_service;
pub mod queries;
pub mod results_writer;
pub mod schedule_updater;
pub mod scheduler;
