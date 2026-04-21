<<<<<<< HEAD
<<<<<<< HEAD
// Placeholder — Phase 5 implementation pending.
=======
//! Experimentation Event Service — gRPC service for event ingestion and definition registry.
//!
//! # Responsibilities
//! 1. **Event Definition Registry**: CRUD for event definitions in PostgreSQL (`events` schema).
//! 2. **Event Ingestion**: Accept `IngestEvent` gRPC calls → validate key against registry →
//!    write accepted events to ClickHouse; reject unknown keys with `INVALID_ARGUMENT`.
//!
//! # Authentication
//! The SDK key is supplied via gRPC metadata header `x-sdk-key`. It is hashed (SHA-256) and
//! looked up against the `sdk_keys` table to resolve the `environment_id`.
#![deny(warnings, missing_docs, clippy::all)]
#![warn(clippy::pedantic, clippy::nursery)]

pub mod grpc;
pub mod registry;

/// Re-export the service implementation for use in `main.rs`.
pub use grpc::event_ingestion::EventIngestionServiceImpl;
/// Re-export the event definition registry.
pub use registry::EventDefinitionRegistry;
>>>>>>> track/microservices_20260421_worker_phase5
=======
// placeholder
>>>>>>> track/microservices_20260421_worker_phase6
