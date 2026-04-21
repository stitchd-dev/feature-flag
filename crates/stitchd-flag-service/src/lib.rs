//! `stitchd-flag-service` — gRPC service for feature flag management.
//!
//! This crate owns the `flags` PostgreSQL schema and provides the
//! [`FlagService`] gRPC implementation with the following RPCs:
//!
//! - `GetFlag` — fetch a single flag definition by key
//! - `ListFlags` — list all flag definitions for an environment
//! - `MutateFlag` — create, update, delete, or archive a flag (with optimistic locking)
//! - `GetFlagDefinitions` — server-streaming endpoint for SDK definition sync

#![deny(warnings, clippy::all)]
#![warn(clippy::pedantic, clippy::nursery)]

pub mod error;
pub mod mapping;
pub mod service;
