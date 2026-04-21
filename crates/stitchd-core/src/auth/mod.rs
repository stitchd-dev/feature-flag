//! Authentication and authorisation domain types.
//!
//! Provides newtypes, role enums, and structs for users, memberships,
//! refresh tokens, auth providers, and invites. Also exposes the SAML engine.

mod types;
pub mod crypto;
pub mod jwt;
pub mod oidc;
pub mod saml;
pub mod totp;

pub use types::{
    AuthProvider, EnvRole, Invite, OrgMembership, OrgRole, ProjectRole, ProviderType,
    RefreshToken, User, UserStatus,
};

pub use crate::id::{AuthProviderId, InviteId, MfaChallengeId, RefreshTokenId, UserId};
pub use crypto::CryptoKey;
pub use jwt::JwtEngine;
pub use oidc::{OidcError, OidcProvider};
pub use totp::{TotpEngine, TotpError};
