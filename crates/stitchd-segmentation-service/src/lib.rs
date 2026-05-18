//! Segmentation Service — gRPC service for segment definitions and membership evaluation.
//!
//! Owns the `segments` PostgreSQL schema. Exposes `SegmentationService` via tonic.
#![deny(warnings, missing_docs, clippy::all)]
#![warn(clippy::pedantic, clippy::nursery)]

pub mod error;
pub mod grpc;
pub mod segment;
pub mod sweeper;
