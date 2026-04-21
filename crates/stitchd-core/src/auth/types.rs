//! Auth domain types: users, roles, tokens, providers, invites.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::id::{AuthProviderId, InviteId, OrganisationId, RefreshTokenId, UserId};

/// The authentication mechanism used by a provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, sqlx::Type)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
#[sqlx(rename_all = "snake_case", type_name = "provider_type")]
pub enum ProviderType {
    /// Username/password authentication.
    Password,
    /// OpenID Connect / OAuth2 authentication.
    Oidc,
    /// SAML 2.0 authentication.
    Saml,
}

/// Organisation-level role.
///
/// Higher variants have more privilege: `OrgAdmin > OrgMember`.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, sqlx::Type,
)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
#[sqlx(rename_all = "snake_case", type_name = "org_role")]
pub enum OrgRole {
    /// Lowest privilege — standard member.
    OrgMember,
    /// Highest privilege — full administrative access.
    OrgAdmin,
}

/// Project-level role.
///
/// Higher variants have more privilege: `ProjectAdmin > ProjectViewer`.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, sqlx::Type,
)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
#[sqlx(rename_all = "snake_case", type_name = "project_role")]
pub enum ProjectRole {
    /// Read-only access.
    ProjectViewer,
    /// Full project administrative access.
    ProjectAdmin,
}

/// Environment-level role.
///
/// Higher variants have more privilege: `EnvPublisher > EnvViewer`.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, sqlx::Type,
)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
#[sqlx(rename_all = "snake_case", type_name = "env_role")]
pub enum EnvRole {
    /// Read-only access.
    EnvViewer,
    /// Can publish flag changes.
    EnvPublisher,
}

/// Lifecycle status of a user account.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, sqlx::Type)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
#[sqlx(rename_all = "snake_case", type_name = "user_status")]
pub enum UserStatus {
    /// Account is active.
    Active,
    /// Account has been deactivated.
    Deactivated,
}

/// A platform user.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct User {
    /// Unique identifier.
    pub id: UserId,
    /// Email address — globally unique.
    pub email: String,
    /// Display name shown in the UI.
    pub display_name: String,
    /// Optional avatar URL.
    pub avatar_url: Option<String>,
    /// Argon2id password hash; `None` for SSO-only users.
    #[serde(skip_serializing)]
    pub password_hash: Option<String>,
    /// Per-user secret used to sign JWTs.
    pub token_secret: uuid::Uuid,
    /// TOTP shared secret — stored as encrypted bytes.
    #[serde(skip_serializing)]
    pub totp_secret: Option<Vec<u8>>,
    /// Whether TOTP MFA is enabled for this user.
    pub totp_enabled: bool,
    /// Lifecycle status.
    pub status: UserStatus,
    /// When the record was created.
    pub created_at: DateTime<Utc>,
    /// When the record was last modified.
    pub updated_at: DateTime<Utc>,
}

/// Membership of a [`User`] in an organisation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct OrgMembership {
    /// User identifier.
    pub user_id: UserId,
    /// Organisation identifier.
    pub org_id: OrganisationId,
    /// Role in the organisation.
    pub role: OrgRole,
    /// When the user joined.
    pub joined_at: DateTime<Utc>,
}

/// An opaque refresh token issued to a user session.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct RefreshToken {
    /// Unique identifier.
    pub id: RefreshTokenId,
    /// Owning user.
    pub user_id: UserId,
    /// Org context for this session.
    pub org_id: OrganisationId,
    /// SHA-256 hash of the raw token value.
    pub token_hash: String,
    /// Optional hint identifying the client device.
    pub device_hint: Option<String>,
    /// When the token was issued.
    pub issued_at: DateTime<Utc>,
    /// When the token expires.
    pub expires_at: DateTime<Utc>,
    /// When the token was revoked, if applicable.
    pub revoked_at: Option<DateTime<Utc>>,
    /// When the token was last used.
    pub last_used_at: Option<DateTime<Utc>>,
}

/// An external or built-in authentication provider configured for an organisation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct AuthProvider {
    /// Unique identifier.
    pub id: AuthProviderId,
    /// Owning organisation.
    pub org_id: OrganisationId,
    /// Authentication mechanism type.
    pub provider_type: ProviderType,
    /// Human-readable display name.
    pub display_name: String,
    /// Provider-specific configuration (encrypted at rest).
    pub config: serde_json::Value,
    /// Whether this provider is currently enabled.
    pub enabled: bool,
    /// When the record was created.
    pub created_at: DateTime<Utc>,
    /// When the record was last modified.
    pub updated_at: DateTime<Utc>,
}

/// An invitation sent to a prospective user.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Invite {
    /// Unique identifier.
    pub id: InviteId,
    /// Target organisation.
    pub org_id: OrganisationId,
    /// Email address of the invitee.
    pub email: String,
    /// Role to grant on acceptance.
    pub org_role: OrgRole,
    /// User who issued the invite, if any.
    pub invited_by_user_id: Option<UserId>,
    /// Hash of the raw invite token.
    pub token_hash: String,
    /// When the invite expires.
    pub expires_at: DateTime<Utc>,
    /// When the invite was accepted, if applicable.
    pub accepted_at: Option<DateTime<Utc>>,
    /// When the record was created.
    pub created_at: DateTime<Utc>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn org_role_ordering_admin_gt_member() {
        assert!(OrgRole::OrgAdmin > OrgRole::OrgMember);
    }

    #[test]
    fn project_role_ordering_admin_gt_viewer() {
        assert!(ProjectRole::ProjectAdmin > ProjectRole::ProjectViewer);
    }

    #[test]
    fn env_role_ordering_publisher_gt_viewer() {
        assert!(EnvRole::EnvPublisher > EnvRole::EnvViewer);
    }

    #[test]
    fn provider_type_password_serde_round_trip() {
        let json = serde_json::to_string(&ProviderType::Password).unwrap();
        assert_eq!(json, r#""password""#);
        let back: ProviderType = serde_json::from_str(&json).unwrap();
        assert_eq!(back, ProviderType::Password);
    }

    #[test]
    fn provider_type_oidc_serde() {
        let json = serde_json::to_string(&ProviderType::Oidc).unwrap();
        assert_eq!(json, r#""oidc""#);
    }

    #[test]
    fn user_id_newtype_create_serialize_compare() {
        let id1 = UserId::new();
        let id2 = UserId::new();
        assert_ne!(id1, id2);

        let json = serde_json::to_string(&id1).unwrap();
        let back: UserId = serde_json::from_str(&json).unwrap();
        assert_eq!(id1, back);
    }

    #[test]
    fn user_id_from_uuid_roundtrip() {
        let uuid = uuid::Uuid::new_v4();
        let id = UserId::from_uuid(uuid);
        assert_eq!(id.as_uuid(), uuid);
    }
}
