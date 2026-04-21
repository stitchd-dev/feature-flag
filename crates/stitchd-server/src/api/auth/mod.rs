//! JWT-based request authentication and authorisation middleware for the admin API.
//!
//! Provides Axum extractors:
//! - [`AuthenticatedUser`]: validates a Bearer JWT and injects the caller's identity.
//! - [`RequireOrgRole`]: additionally enforces a minimum [`OrgRole`].
//! - [`RequireProjectRole`]: enforces a minimum [`ProjectRole`] for the current project.
//! - [`RequireEnvRole`]: enforces a minimum [`EnvRole`] for the current environment.
//!
//! Also provides HTTP handlers for password auth and session management:
//! - [`password`]: login, refresh, logout, switch-org
//! - [`sessions`]: list sessions, revoke session, revoke all

pub mod middleware;
pub mod password;
pub mod sessions;

pub use middleware::{AuthenticatedUser, RequireEnvRole, RequireOrgRole, RequireProjectRole};

#[cfg(test)]
mod tests;
