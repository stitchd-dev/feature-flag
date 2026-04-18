//! Rust server-side SDK for Stitchd.
//!
//! Call [`SdkClient::init`] once at startup, then use [`SdkClient::evaluate`]
//! per-request. Rule-based segments are evaluated in-process; list-based
//! segments fall back to a REST call (optionally pre-warmed via LFU cache).
//!
//! # Quickstart
//!
//! Add the SDK to your `Cargo.toml`:
//!
//! ```toml
//! [dependencies]
//! stitchd-sdk = "0.1"
//! ```
//!
//! Initialize the client once at application startup:
//!
//! ```rust,no_run
//! use stitchd_sdk::{SdkClient, SdkConfig};
//! use stitchd_sdk::{Context, EvaluationContext};
//! use std::sync::Arc;
//!
//! #[tokio::main]
//! async fn main() {
//!     let config = SdkConfig::new(
//!         "http://localhost:9090",   // gRPC endpoint
//!         "http://localhost:8080",   // REST endpoint for list-checks
//!         "sk_live_...",             // SDK key
//!     );
//!
//!     let client: Arc<SdkClient> = SdkClient::init(config).await.expect("SDK init failed");
//!
//!     // Evaluate a flag for a specific user context
//!     let ctx = EvaluationContext {
//!         contexts: vec![
//!             Context::new("user", "user-123"),
//!         ],
//!     };
//!
//!     let variant = client.evaluate("my-feature-flag", &ctx).await.expect("evaluation failed");
//!     println!("Flag variant: {}", variant.key);
//! }
//! ```
//!
//! See [`SdkClient`], [`SdkConfig`], and [`EvaluationContext`] for full API details.
#![deny(warnings, missing_docs, clippy::all)]

pub mod cache;
/// SDK client implementation.
pub mod client;
/// SDK configuration types.
pub mod config;
/// SDK error types.
pub mod error;
mod grpc_client;
mod http_client;
pub mod lfu;

pub use client::SdkClient;
pub use config::{LfuConfig, SdkConfig};
pub use error::SdkError;

// Re-export context types so consumers don't need to depend on stitchd-core directly.
pub use stitchd_core::context::{Context, EvaluationContext, ParameterValue};
pub use stitchd_core::variants::VariantValue;
