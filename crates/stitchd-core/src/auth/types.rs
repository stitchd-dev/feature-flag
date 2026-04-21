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
    Password,
    Oidc,
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
    Active,
    Deactivated,
}

/// A platform user.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct User {
    pub id: UserId,
    pub email: String,
    pub display_name: String,
    pub avatar_url: Option<String>,
    #[serde(skip_serializing)]
    pub password_hash: Option<String>,
    pub token_secret: uuid::Uuid,
    /// TOTP shared secret — stored as encrypted bytes.
    #[serde(skip_serializing)]
    pub totp_secret: Option<Vec<u8>>,
    pub totp_enabled: bool,
    pub status: UserStatus,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Membership of a [`User`] in an organisation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct OrgMembership {
    pub user_id: UserId,
    pub org_id: OrganisationId,
    pub role: OrgRole,
    pub joined_at: DateTime<Utc>,
}

/// An opaque refresh token issued to a user session.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct RefreshToken {
    pub id: RefreshTokenId,
    pub user_id: UserId,
    pub org_id: OrganisationId,
    /// Hash of the raw token. The raw value is never stored.
    pub token_hash: String,
    /// Optional hint identifying the client device.
    pub device_hint: Option<String>,
    pub issued_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub revoked_at: Option<DateTime<Utc>>,
    pub last_used_at: Option<DateTime<Utc>>,
}

/// An external or built-in authentication provider configured for an organisation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct AuthProvider {
    pub id: AuthProviderId,
    pub org_id: OrganisationId,
    pub provider_type: ProviderType,
    pub display_name: String,
    pub config: serde_json::Value,
    pub enabled: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// An invitation sent to a prospective user.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Invite {
    pub id: InviteId,
    pub org_id: OrganisationId,
    pub email: String,
    pub org_role: OrgRole,
    pub invited_by_user_id: Option<UserId>,
    /// Hash of the raw invite token.
    pub token_hash: String,
    pub expires_at: DateTime<Utc>,
    pub accepted_at: Option<DateTime<Utc>>,
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
