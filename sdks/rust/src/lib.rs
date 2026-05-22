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
//! ```no_run
//! use std::time::Duration;
//! use stitchd_sdk_rust::{EvalRequest, SdkClient, SdkConfig, TraceLevel};
//! use stitchd_core::context::Context;
//!
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     // Required: gateway base URL + SDK key (provisioned in the admin UI).
//!     // All other fields fall back to spec-defined defaults — see `SdkConfig`.
//!     let config = SdkConfig::new(
//!         std::env::var("STITCHD_GATEWAY_URL")
//!             .unwrap_or_else(|_| "http://localhost:8081".to_string()),
//!         std::env::var("STITCHD_SDK_KEY")?,
//!     );
//!
//!     // `init` validates config, performs the first definition sync, and
//!     // spawns the three background tasks (poll, LRU refresh, event flush).
//!     let client = SdkClient::init(config).await?;
//!
//!     // Evaluate a flag for a `(context_type, key)` tuple. Each `EvalRequest`
//!     // carries a context BUNDLE (`contexts: Vec<Context>`) — supply every
//!     // context type referenced by the flag's rules and/or its
//!     // percentage-hash selectors.
//!     let context = Context::new("user", "alice");
//!     let results = client
//!         .evaluate(
//!             &[EvalRequest::single("checkout-flow", context)],
//!             TraceLevel::Off,
//!         )
//!         .await;
//!     println!("variant = {}", results[0].variant_key);
//!
//!     // Drain the track-event buffer + stop the background tasks.
//!     client.shutdown(Duration::from_secs(5)).await?;
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
pub(crate) mod metrics;
pub mod polling;
pub mod refresh;
pub mod snapshot;

pub use client::{EvalOutcome, EvalRequest, EvalResult, SdkClient};
// Re-export the core trace types so callers of the unified
// `evaluate(&[EvalRequest], TraceLevel)` (Phase 6 of
// `flag_eval_unify_20260522`) can pattern-match on the rich trace bundle
// without depending on `stitchd-core` directly.
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
pub use stitchd_core::evaluation::{EvaluationTrace, TraceLevel};
