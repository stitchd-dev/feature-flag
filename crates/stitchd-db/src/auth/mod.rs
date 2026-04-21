//! Auth database repositories.
//!
//! Provides:
//! - [`RefreshTokenRepository`] / [`PgRefreshTokenRepository`]
//! - [`AuthUserRepository`] / [`PgAuthUserRepository`]
//! - [`OrgMembershipRepository`] / [`PgOrgMembershipRepository`]
//! - [`MfaRepository`] / [`PgMfaRepository`]
//! - [`MfaChallengeRepository`] / [`PgMfaChallengeRepository`] (Phase 3 compat)

pub mod memberships;
pub mod mfa;
pub mod refresh_tokens;
pub mod users;

pub use memberships::{OrgMembershipRepository, PgOrgMembershipRepository};
pub use mfa::{
    MfaChallengeRepository, MfaRepository, PgMfaChallengeRepository, PgMfaRepository,
    challenge_token_hash,
};
pub use refresh_tokens::{PgRefreshTokenRepository, RefreshTokenRepository};
pub use users::{AuthUserRepository, PgAuthUserRepository};
