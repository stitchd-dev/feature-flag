//! Auth database repositories.
//!
//! Provides:
//! - [`RefreshTokenRepository`] / [`PgRefreshTokenRepository`]
//! - [`AuthUserRepository`] / [`PgAuthUserRepository`]
//! - [`OrgMembershipRepository`] / [`PgOrgMembershipRepository`]
//! - [`MfaRepository`] / [`PgMfaRepository`]
//! - [`AuthProviderRepository`] / [`PgAuthProviderRepository`]
//! - [`InviteRepository`] / [`PgInviteRepository`]
//! - [`OtpRepository`] / [`PgOtpRepository`]

pub mod invites;
pub mod memberships;
pub mod mfa;
pub mod password_reset;
pub mod providers;
pub mod refresh_tokens;
pub mod users;

pub use invites::{InviteRepository, PgInviteRepository};
pub use memberships::{OrgMembershipRepository, PgOrgMembershipRepository};
pub use mfa::{MfaRepository, PgMfaRepository, challenge_token_hash};
pub use password_reset::{OtpRepository, PgOtpRepository};
pub use providers::{AuthProviderRepository, PgAuthProviderRepository};
pub use refresh_tokens::{PgRefreshTokenRepository, RefreshTokenRepository};
pub use users::{AuthUserRepository, PgAuthUserRepository};
