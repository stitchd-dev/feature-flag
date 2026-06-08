//! `stitchd-core` — Pure domain library for the Stitchd feature-flag and experimentation
//! platform.
//!
//! No I/O, no database, no network. All other workspace crates depend on this. Holds the
//! canonical types for feature flags, segments, contexts, evaluation, events, experiments
//! (including multi-armed bandit algorithms, posteriors, and convergence detection),
//! and the rule engine that combines them.
//!
//! See the [Architecture] chapter of the mdBook for how this crate sits relative to the
//! `*-service` microservices and the gateway.
//!
//! [Architecture]: ../architecture/README.html

pub mod auth;
pub mod context;
pub mod evaluation;
pub mod event;
pub mod experimentation;
pub mod flag;
pub mod hashing;
pub mod id;
pub mod metric;
pub mod prerequisite;
pub mod rollout;
pub mod rule_engine;
pub mod schedule;
pub mod segment;
pub mod tenant;
pub mod user;
pub mod util;
pub mod variants;
