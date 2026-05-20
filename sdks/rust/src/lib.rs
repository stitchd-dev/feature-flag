//! # stitchd-sdk-rust — Server-Side Rust SDK for Stitchd Feature Flags
//!
//! Embed this crate (`stitchd-sdk-rust`) in a backend service to evaluate
//! feature flags entirely in-process. Flag and segment definitions are pulled
//! from `stitchd-gateway` via gRPC polling and cached locally; list-segment
//! membership is maintained in a bounded LRU cache. Evaluation events are
//! submitted asynchronously via a fire-and-forget batch flush, keeping the
//! hot evaluation path free of network I/O.
//!
//! This crate conforms to the language-agnostic SDK contract defined in
//! `sdks/spec/`. See `sdks/spec/docs/` for the canonical evaluation algorithm,
//! caching rules, polling lifecycle, and event-delivery semantics.
//!
//! # Quickstart
//!
//! ```ignore
//! use stitchd_sdk_rust::{SdkClient, SdkConfig, EvalRequest};
//!
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     let config = SdkConfig {
//!         gateway_url: "http://localhost:8081".to_string(),
//!         sdk_key:     std::env::var("STITCHD_SDK_KEY")?,
//!         ..Default::default()
//!     };
//!
//!     let client = SdkClient::init(config).await?;
//!     let ctx    = stitchd_core::context::Context::new("user", "alice");
//!     let results = client
//!         .evaluate(&[EvalRequest::flag("my-flag", ctx)])
//!         .await;
//!     println!("variant = {}", results[0].variant_key);
//!     client.shutdown().await;
//!     Ok(())
//! }
//! ```
//!
//! # Modules
//!
//! - [`config`]       — [`SdkConfig`] and defaults
//! - [`client`]       — [`SdkClient`], [`EvalRequest`], [`EvalResult`]
//! - [`error`]        — [`SdkError`] taxonomy
//! - [`event_buffer`] — client-side track-event buffer + flush (Phase 5)
//! - [`events`]       — fire-and-forget flag-evaluation event queue
//! - [`lru`]          — bounded list-segment membership cache
//! - [`polling`]      — gRPC definition sync loop
//! - [`snapshot`]     — immutable [`DefinitionSnapshot`] and [`DefinitionStore`]

pub mod client;
pub mod config;
pub mod error;
pub mod event_buffer;
pub mod events;
pub mod lru;
pub mod polling;
pub mod refresh;
pub mod snapshot;

pub use client::{
    EvalOutcome, EvalRequest, EvalResult, EvalResultWithReasoning, ReasoningTrace, SdkClient,
};
pub use config::SdkConfig;
pub use error::{SdkError, TrackError};
pub use event_buffer::{
    BufferedEvent, EventBuffer, EventBufferConfig, FlushError, FlushReport, TypedValue,
};
pub use events::{EventQueue, EventSink, FlagEvaluationEvent, FlushTask, ParameterValue};
pub use lru::{ContextKey, MembershipCache, MembershipMap};
pub use polling::{DefinitionFetcher, PollTask};
pub use refresh::{MembershipBatchFetcher, RefreshTask};
pub use snapshot::{DefinitionSnapshot, DefinitionStore, EventValueType};
