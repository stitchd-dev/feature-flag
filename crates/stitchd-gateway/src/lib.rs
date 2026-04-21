//! `stitchd-gateway` — API Gateway for the stitchd microservices platform.
//!
//! Exposes the same REST surface as the former monolith (`stitchd-server`) but
//! delegates every request to the appropriate domain gRPC service after
//! authenticating the caller via the Auth Service.
//!
//! # Architecture
//! - Every request passes through [`middleware::auth::auth_middleware`], which
//!   calls the Auth Service and injects an [`RbacContext`] extension.
//! - Admin routes require a `Bearer` JWT token.
//! - SDK routes require an `x-sdk-key` header.
//! - No business logic lives here — the gateway is pure proxy.

#![deny(warnings, clippy::all)]

pub mod error;
pub mod middleware;
pub mod openapi;
pub mod router;
pub mod routes;
pub mod state;

#[cfg(test)]
pub mod tests;
