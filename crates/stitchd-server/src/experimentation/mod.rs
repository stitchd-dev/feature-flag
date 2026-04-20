//! Experimentation server-side logic.
//!
//! This module hosts the async compute pipeline and any other server-specific
//! experimentation helpers (e.g. scheduling, result retrieval).

/// Async stats compute pipeline for experiment result generation.
pub mod compute;
/// Response types for the experiment results REST API.
pub mod results_api;
