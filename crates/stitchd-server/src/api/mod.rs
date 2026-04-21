//! API router and handlers.

/// Authentication handlers (JWT middleware, password, OIDC, MFA, SAML 2.0).
pub mod auth;
/// Event definition registration API.
pub mod event_definitions;
/// Event ingestion API.
pub mod events;
/// Experiment management API.
pub mod experiments;
/// Feature Flag management API.
pub mod flags;
/// OpenAPI schema aggregator.
pub mod openapi;
/// Router implementation.
pub mod router;
/// SDK key authentication extractor.
pub mod sdk_auth;
/// Segment management API.
pub mod segments;
