//! Rust server-side SDK for Stitchd.
//!
//! Call [`SdkClient::init`] once at startup, then use [`SdkClient::evaluate`]
//! per-request. Rule-based segments are evaluated in-process; list-based
//! segments fall back to a REST call (optionally pre-warmed via LFU cache).
#![deny(warnings, clippy::all)]

pub mod cache;
pub mod client;
pub mod config;
pub mod error;
mod grpc_client;
mod http_client;

pub use client::SdkClient;
pub use config::{LfuConfig, SdkConfig};
pub use error::SdkError;

// Re-export context types so consumers don't need to depend on stitchd-core directly.
pub use stitchd_core::context::{Context, EvaluationContext, ParameterValue};
pub use stitchd_core::variants::VariantValue;
