//! Authentication handlers for the admin API.
//!
//! Exposes SAML 2.0 SP endpoints:
//! - `GET  /auth/saml/{org_slug}/login`    — SP-initiated SSO
//! - `POST /auth/saml/{org_slug}/acs`      — Assertion Consumer Service
//! - `GET  /auth/saml/{org_slug}/metadata` — SP metadata XML
//! - `POST /auth/saml/{org_slug}/slo`      — Single Logout

pub mod saml;
