//! # Stitchd Feature Flag — Server-Side Rust SDK
//!
//! Embed this crate in a backend service to evaluate flags in-process. Pulls
//! flag and segment definitions from `stitchd-gateway` via polling; caches
//! list-segment membership in a bounded LRU.
//!
//! This SDK conforms to the language-agnostic contract under `sdks/spec/`.
//! See the behavioural spec in `sdks/spec/docs/` for the canonical algorithm,
//! cache semantics, and event-delivery rules.
//!
//! # Quickstart
//!
//! ```ignore
//! use stitchd_sdk::{SdkClient, SdkConfig, EvalRequest, Context};
//! use std::time::Duration;
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
//!
//!     let context = Context::user("alice")
//!         .with_param("plan", "pro");
//!
//!     let results = client
//!         .evaluate(&[EvalRequest::new("checkout-flow", context)])
//!         .await;
//!
//!     println!("variant = {}", results[0].variant_key);
//!     Ok(())
//! }
//! ```
//!
//! # Modules
//!
//! - [`config`]  — [`SdkConfig`] and defaults
//! - [`error`]   — [`SdkError`] taxonomy
//!
//! Subsequent phases of the `sdk_rewrite_20260516` track add the
//! evaluation engine, polling loops, LRU cache, and event flush — each
//! introducing its own module here.

pub mod client;
pub mod config;
pub mod error;
pub mod events;
pub mod lru;
pub mod polling;
pub mod refresh;
pub mod snapshot;

pub use client::{
    EvalOutcome, EvalRequest, EvalResult, EvalResultWithReasoning, ReasoningTrace, SdkClient,
};
pub use config::SdkConfig;
pub use error::SdkError;
pub use events::{EventQueue, EventSink, FlagEvaluationEvent, FlushTask, ParameterValue};
pub use lru::{ContextKey, MembershipCache, MembershipMap};
pub use polling::{DefinitionFetcher, PollTask};
pub use refresh::{MembershipBatchFetcher, RefreshTask};
pub use snapshot::{DefinitionSnapshot, DefinitionStore};
