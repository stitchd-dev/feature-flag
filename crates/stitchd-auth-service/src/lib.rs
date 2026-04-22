//! `stitchd-auth-service` — gRPC auth microservice.
//!
//! Implements the `AuthService` gRPC contract, accepting a [`CredentialRequest`]
//! (JWT bearer token or SDK key) and returning a [`RbacContext`] with tenant,
//! environment, roles, permissions, and subject identity.
//!
//! # Modules
//! - [`grpc`]: tonic `AuthService` trait implementation
//! - [`jwt`]: JWT validation logic (decode + signature verification)
//! - [`sdk_key`]: SDK key hash lookup and active-key constraint enforcement
//! - [`rbac`]: RBAC context assembly helpers

#![deny(warnings, clippy::all)]
#![warn(clippy::pedantic, clippy::nursery)]

pub mod app_state;
pub mod auth_provider;
pub mod bootstrap;
pub mod grpc;
pub mod jwt;
pub mod login;
pub mod management;
pub mod oidc_factory;
pub mod oidc_login;
pub mod provider_cache;
pub mod rbac;
pub mod saml_factory;
pub mod saml_login;
pub mod sdk_key;
