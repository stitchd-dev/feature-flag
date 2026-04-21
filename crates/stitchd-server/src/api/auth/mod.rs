//! JWT-based request authentication and authorisation middleware for the admin API.
//!
//! Provides Axum extractors:
//! - [`AuthenticatedUser`]: validates a Bearer JWT and injects the caller's identity.
//! - [`RequireOrgRole`]: additionally enforces a minimum [`OrgRole`].
//! - [`RequireProjectRole`]: enforces a minimum [`ProjectRole`] for the current project.
//! - [`RequireEnvRole`]: enforces a minimum [`EnvRole`] for the current environment.

pub mod middleware;

pub use middleware::{AuthenticatedUser, RequireEnvRole, RequireOrgRole, RequireProjectRole};
