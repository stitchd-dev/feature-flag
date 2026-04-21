//! Auth database repositories.
//!
//! Provides:
//! - [`RefreshTokenRepository`] / [`PgRefreshTokenRepository`]
//! - [`AuthUserRepository`] / [`PgAuthUserRepository`]
//! - [`OrgMembershipRepository`] / [`PgOrgMembershipRepository`]
//! - [`MfaChallengeRepository`] / [`PgMfaChallengeRepository`]
//! - [`InviteRepository`] / [`PgInviteRepository`]
//! - [`OtpRepository`] / [`PgOtpRepository`]

pub mod invites;
pub mod memberships;
pub mod mfa;
pub mod password_reset;
pub mod refresh_tokens;
pub mod users;

pub use invites::{InviteRepository, PgInviteRepository};
pub use memberships::{OrgMembershipRepository, PgOrgMembershipRepository};
pub use mfa::{MfaChallengeRepository, PgMfaChallengeRepository};
pub use password_reset::{OtpRepository, PgOtpRepository};
pub use refresh_tokens::{PgRefreshTokenRepository, RefreshTokenRepository};
pub use users::{AuthUserRepository, PgAuthUserRepository};
