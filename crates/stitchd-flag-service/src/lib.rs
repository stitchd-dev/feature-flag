//! `stitchd-flag-service` — gRPC microservice that owns the `feature_flags` PostgreSQL
//! schema and the flag evaluation hot path.
//!
//! Exposes the `FlagService` and `FlagSync` proto contracts via tonic. Used by the gateway
//! for synchronous CRUD calls and by SDKs (via the gateway's `/v1/sdk/*` proxy) for high-
//! throughput definition sync + evaluation.

#![deny(warnings, clippy::all)]

pub mod error;
pub mod flag_lock;
pub mod mapping;
pub mod prerequisites;
pub mod sdk_backend;
pub mod service;
