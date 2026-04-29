//! RBAC context assembly.
//!
//! Maps the validated credential (JWT or SDK key) to a [`RbacContext`] proto
//! message, populating `tenant_id`, `environment_id`, roles, permissions, and
//! subject.

use stitchd_core::auth::OrgRole;
use stitchd_proto::auth::v1::RbacContext;

use crate::jwt::ValidatedJwt;
use crate::sdk_key::SdkKeyContext;

const ORG_ADMIN_PERMISSIONS: &[&str] = &[
    "project:create",
    "project:rename",
    "project:delete",
    "environment:read",
    "environment:create",
    "environment:rename",
    "environment:delete",
    "sdk_key:create",
    "sdk_key:revoke",
];

const ORG_MEMBER_PERMISSIONS: &[&str] = &["environment:read"];

/// Assemble an [`RbacContext`] from a validated JWT.
#[must_use]
pub fn rbac_context_from_jwt(validated: &ValidatedJwt) -> RbacContext {
    let (role_str, perms) = match validated.org_role {
        OrgRole::OrgAdmin => ("org_admin", ORG_ADMIN_PERMISSIONS),
        OrgRole::OrgMember => ("org_member", ORG_MEMBER_PERMISSIONS),
    };
    RbacContext {
        tenant_id: validated.org_id.clone(),
        environment_id: String::new(),
        roles: vec![role_str.to_string()],
        permissions: perms.iter().map(|s| s.to_string()).collect(),
        subject: validated.user.id.to_string(),
        is_system: validated.is_system,
    }
}

/// Assemble an [`RbacContext`] from a validated SDK key.
///
/// For SDK-authenticated callers:
/// - `tenant_id` — empty (SDK keys are not directly scoped to an org UUID).
/// - `environment_id` — the environment ID the key is scoped to.
/// - `roles` — `["sdk"]` to identify this as an SDK client.
/// - `permissions` — empty.
/// - `subject` — the SDK key ID (UUID string).
#[must_use]
pub fn rbac_context_from_sdk_key(ctx: &SdkKeyContext) -> RbacContext {
    RbacContext {
        tenant_id: String::new(),
        environment_id: ctx.environment_id.to_string(),
        roles: vec!["sdk".to_string()],
        permissions: vec![],
        subject: ctx.sdk_key_id.to_string(),
        is_system: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use stitchd_core::{
        auth::{OrgRole, User, UserStatus},
        id::{EnvironmentId, OrganisationId, SdkKeyId, UserId},
    };

    fn make_validated_jwt() -> ValidatedJwt {
        let org_id = OrganisationId::new();
        ValidatedJwt {
            user: User {
                id: UserId::new(),
                email: "user@example.com".to_string(),
                display_name: "Test".to_string(),
                avatar_url: None,
                password_hash: None,
                token_secret: uuid::Uuid::new_v4(),
                totp_secret: None,
                totp_enabled: false,
                status: UserStatus::Active,
                created_at: Utc::now(),
                updated_at: Utc::now(),
            },
            org_id: org_id.to_string(),
            org_role: OrgRole::OrgMember,
            is_system: false,
        }
    }

    #[test]
    fn jwt_rbac_context_has_correct_fields() {
        let validated = make_validated_jwt();
        let ctx = rbac_context_from_jwt(&validated);

        assert_eq!(ctx.tenant_id, validated.org_id);
        assert_eq!(ctx.environment_id, "");
        assert_eq!(ctx.subject, validated.user.id.to_string());
        assert!(!ctx.roles.is_empty());
        assert!(!ctx.permissions.is_empty());
    }

    #[test]
    fn sdk_key_rbac_context_has_correct_fields() {
        let env_id = EnvironmentId::new();
        let sdk_key_id = SdkKeyId::new();
        let ctx = rbac_context_from_sdk_key(&SdkKeyContext {
            environment_id: env_id,
            sdk_key_id,
        });

        assert_eq!(ctx.tenant_id, "");
        assert_eq!(ctx.environment_id, env_id.to_string());
        assert_eq!(ctx.subject, sdk_key_id.to_string());
        assert_eq!(ctx.roles, vec!["sdk".to_string()]);
        assert!(ctx.permissions.is_empty());
    }

    #[test]
    fn jwt_rbac_roles_not_empty() {
        let validated = make_validated_jwt();
        let ctx = rbac_context_from_jwt(&validated);
        assert!(!ctx.roles.is_empty());
    }
}
