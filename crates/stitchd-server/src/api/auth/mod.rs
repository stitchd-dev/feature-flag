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
//!
//! Also exposes SAML 2.0 SP endpoints:
//! - `GET  /auth/saml/{org_slug}/login`    — SP-initiated SSO
//! - `POST /auth/saml/{org_slug}/acs`      — Assertion Consumer Service
//! - `GET  /auth/saml/{org_slug}/metadata` — SP metadata XML
//! - `POST /auth/saml/{org_slug}/slo`      — Single Logout

pub mod invites;
pub mod middleware;
pub mod mfa;
pub mod oidc;
pub mod password;
pub mod password_reset;
pub mod profile;
pub mod providers;
pub mod rate_limit;
pub mod saml;
pub mod sessions;
pub mod user_management;

pub use middleware::{AuthenticatedUser, RequireEnvRole, RequireOrgRole, RequireProjectRole};

#[cfg(test)]
mod tests;
