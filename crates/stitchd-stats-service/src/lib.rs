//! `stitchd-stats-service` — Scheduled Statistics Processing Service.
//!
//! A standalone microservice that periodically (re)computes experiment results for all
//! running experiments. Reads from ClickHouse and PostgreSQL, writes aggregate statistics
//! back to PostgreSQL. Exposes both a tonic gRPC interface (`StatsService`) and an HTTP
//! interface for triggering on-demand recomputes.
//!
//! Beyond fixed-split A/B analysis, the same scheduled tick drives **multi-armed
//! bandit** experiments: the [`bandit`] module recomputes arm weights from live
//! rewards and applies them via a privileged lock-bypass write (static
//! propagation) or publishes an in-memory posterior snapshot model (realtime
//! propagation); [`lifecycle`] detects convergence and autonomously commits /
//! rolls out the winner (releasing the whole-flag lock); [`campaign`] chains
//! successive bandit iterations. Cross-experiment [`interaction_compute`]
//! generalizes hierarchical interaction analysis to order 4+.

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
