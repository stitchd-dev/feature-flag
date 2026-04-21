//! Org membership repository — trait definition and Postgres implementation.
//!
//! Manages the `org_memberships` join table that links platform users to
//! organisations with a specific [`OrgRole`].

use async_trait::async_trait;
use sqlx::PgPool;
use stitchd_core::{
    auth::{OrgMembership, OrgRole},
    id::{OrganisationId, UserId},
};

use crate::RepositoryError;

// ---------------------------------------------------------------------------
// Trait
// ---------------------------------------------------------------------------

/// Operations on the `org_memberships` table.
#[async_trait]
pub trait OrgMembershipRepository: Send + Sync {
    /// Add `user_id` to `org_id` with `role`. Fails with [`RepositoryError::UniqueViolation`]
    /// if the membership already exists.
    async fn add_member(
        &self,
        user_id: UserId,
        org_id: OrganisationId,
        role: OrgRole,
    ) -> Result<OrgMembership, RepositoryError>;

    /// Look up a single membership record. Returns `None` if the user is not a member.
    async fn find_membership(
        &self,
        user_id: UserId,
        org_id: OrganisationId,
    ) -> Result<Option<OrgMembership>, RepositoryError>;

    /// List all organisation memberships for `user_id`.
    async fn list_orgs_for_user(
        &self,
        user_id: UserId,
    ) -> Result<Vec<OrgMembership>, RepositoryError>;

    /// Remove a user from an organisation. No-ops if not a member.
    async fn remove_member(
        &self,
        user_id: UserId,
        org_id: OrganisationId,
    ) -> Result<(), RepositoryError>;

    /// Change the role of an existing member. Returns [`RepositoryError::NotFound`] if
    /// no membership exists for the given pair.
    async fn update_role(
        &self,
        user_id: UserId,
        org_id: OrganisationId,
        role: OrgRole,
    ) -> Result<(), RepositoryError>;
}

// ---------------------------------------------------------------------------
// Postgres implementation
// ---------------------------------------------------------------------------

/// Postgres-backed [`OrgMembershipRepository`].
pub struct PgOrgMembershipRepository {
    /// Shared Postgres connection pool.
    pub pool: PgPool,
}

impl PgOrgMembershipRepository {
    /// Construct a new repository bound to `pool`.
    #[must_use]
    pub const fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

/// Convert a DB `role` TEXT value to [`OrgRole`].
fn parse_role(s: &str) -> Result<OrgRole, RepositoryError> {
    match s {
        "org_admin" => Ok(OrgRole::OrgAdmin),
        "org_member" => Ok(OrgRole::OrgMember),
        other => Err(RepositoryError::Unexpected(anyhow::anyhow!(
            "unknown org role: {other}"
        ))),
    }
}

/// Convert [`OrgRole`] to its DB TEXT representation.
const fn role_str(r: OrgRole) -> &'static str {
    match r {
        OrgRole::OrgAdmin => "org_admin",
        OrgRole::OrgMember => "org_member",
    }
}

#[async_trait]
impl OrgMembershipRepository for PgOrgMembershipRepository {
    async fn add_member(
        &self,
        user_id: UserId,
        org_id: OrganisationId,
        role: OrgRole,
    ) -> Result<OrgMembership, RepositoryError> {
        let row = sqlx::query!(
            r#"
            INSERT INTO org_memberships (user_id, org_id, role)
            VALUES ($1, $2, $3)
            RETURNING user_id, org_id, role, joined_at
            "#,
            user_id.as_uuid(),
            org_id.as_uuid(),
            role_str(role),
        )
        .fetch_one(&self.pool)
        .await
        .map_err(|e| {
            if let sqlx::Error::Database(ref dbe) = e {
                if let Some(constraint) = dbe.constraint() {
                    return RepositoryError::UniqueViolation {
                        field: constraint.to_string(),
                    };
                }
            }
            RepositoryError::Database(e)
        })?;

        Ok(OrgMembership {
            user_id: UserId::from_uuid(row.user_id),
            org_id: OrganisationId::from_uuid(row.org_id),
            role: parse_role(&row.role)?,
            joined_at: row.joined_at,
        })
    }

    async fn find_membership(
        &self,
        user_id: UserId,
        org_id: OrganisationId,
    ) -> Result<Option<OrgMembership>, RepositoryError> {
        let row = sqlx::query!(
            r#"
            SELECT user_id, org_id, role, joined_at
            FROM org_memberships
            WHERE user_id = $1 AND org_id = $2
            "#,
            user_id.as_uuid(),
            org_id.as_uuid(),
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(RepositoryError::Database)?;

        row.map(|r| {
            Ok(OrgMembership {
                user_id: UserId::from_uuid(r.user_id),
                org_id: OrganisationId::from_uuid(r.org_id),
                role: parse_role(&r.role)?,
                joined_at: r.joined_at,
            })
        })
        .transpose()
    }

    async fn list_orgs_for_user(
        &self,
        user_id: UserId,
    ) -> Result<Vec<OrgMembership>, RepositoryError> {
        let rows = sqlx::query!(
            r#"
            SELECT user_id, org_id, role, joined_at
            FROM org_memberships
            WHERE user_id = $1
            ORDER BY joined_at
            "#,
            user_id.as_uuid(),
        )
        .fetch_all(&self.pool)
        .await
        .map_err(RepositoryError::Database)?;

        rows.into_iter()
            .map(|r| {
                Ok(OrgMembership {
                    user_id: UserId::from_uuid(r.user_id),
                    org_id: OrganisationId::from_uuid(r.org_id),
                    role: parse_role(&r.role)?,
                    joined_at: r.joined_at,
                })
            })
            .collect()
    }

    async fn remove_member(
        &self,
        user_id: UserId,
        org_id: OrganisationId,
    ) -> Result<(), RepositoryError> {
        sqlx::query!(
            r#"
            DELETE FROM org_memberships
            WHERE user_id = $1 AND org_id = $2
            "#,
            user_id.as_uuid(),
            org_id.as_uuid(),
        )
        .execute(&self.pool)
        .await
        .map_err(RepositoryError::Database)?;
        Ok(())
    }

    async fn update_role(
        &self,
        user_id: UserId,
        org_id: OrganisationId,
        role: OrgRole,
    ) -> Result<(), RepositoryError> {
        let affected = sqlx::query!(
            r#"
            UPDATE org_memberships
            SET role = $1
            WHERE user_id = $2 AND org_id = $3
            "#,
            role_str(role),
            user_id.as_uuid(),
            org_id.as_uuid(),
        )
        .execute(&self.pool)
        .await
        .map_err(RepositoryError::Database)?
        .rows_affected();

        if affected == 0 {
            return Err(RepositoryError::NotFound {
                id: format!("user={user_id} org={org_id}"),
            });
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Integration tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: insert a minimal user and org, return (`org_id`, `user_id`).
    async fn seed(pool: &PgPool) -> (OrganisationId, UserId) {
        let org_id = OrganisationId::new();
        sqlx::query!(
            "INSERT INTO organisations (id, name) VALUES ($1, $2)",
            org_id.as_uuid(),
            "test-org"
        )
        .execute(pool)
        .await
        .unwrap();

        let user_id = UserId::new();
        sqlx::query!(
            r#"INSERT INTO users (id, email, display_name) VALUES ($1, $2, $3)"#,
            user_id.as_uuid(),
            format!("u-{}@example.com", user_id.as_uuid()),
            "Test User"
        )
        .execute(pool)
        .await
        .unwrap();

        (org_id, user_id)
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn add_member_returns_membership(pool: PgPool) {
        let repo = PgOrgMembershipRepository::new(pool.clone());
        let (org_id, user_id) = seed(&pool).await;

        let membership = repo
            .add_member(user_id, org_id, OrgRole::OrgMember)
            .await
            .unwrap();

        assert_eq!(membership.user_id, user_id);
        assert_eq!(membership.org_id, org_id);
        assert_eq!(membership.role, OrgRole::OrgMember);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn add_member_duplicate_returns_unique_violation(pool: PgPool) {
        let repo = PgOrgMembershipRepository::new(pool.clone());
        let (org_id, user_id) = seed(&pool).await;

        repo.add_member(user_id, org_id, OrgRole::OrgMember)
            .await
            .unwrap();
        let err = repo
            .add_member(user_id, org_id, OrgRole::OrgAdmin)
            .await
            .unwrap_err();
        assert!(matches!(err, RepositoryError::UniqueViolation { .. }));
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn find_membership_existing(pool: PgPool) {
        let repo = PgOrgMembershipRepository::new(pool.clone());
        let (org_id, user_id) = seed(&pool).await;

        repo.add_member(user_id, org_id, OrgRole::OrgAdmin)
            .await
            .unwrap();
        let found = repo.find_membership(user_id, org_id).await.unwrap();
        assert!(found.is_some());
        assert_eq!(found.unwrap().role, OrgRole::OrgAdmin);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn find_membership_missing_returns_none(pool: PgPool) {
        let repo = PgOrgMembershipRepository::new(pool.clone());
        let found = repo
            .find_membership(UserId::new(), OrganisationId::new())
            .await
            .unwrap();
        assert!(found.is_none());
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn list_orgs_for_user_multiple(pool: PgPool) {
        let repo = PgOrgMembershipRepository::new(pool.clone());

        // Two orgs
        let org1_id = OrganisationId::new();
        let org2_id = OrganisationId::new();
        sqlx::query!(
            "INSERT INTO organisations (id, name) VALUES ($1, $2), ($3, $4)",
            org1_id.as_uuid(),
            "org1",
            org2_id.as_uuid(),
            "org2"
        )
        .execute(&pool)
        .await
        .unwrap();

        let user_id = UserId::new();
        sqlx::query!(
            r#"INSERT INTO users (id, email, display_name) VALUES ($1, $2, $3)"#,
            user_id.as_uuid(),
            "multi@example.com",
            "Multi User"
        )
        .execute(&pool)
        .await
        .unwrap();

        repo.add_member(user_id, org1_id, OrgRole::OrgMember)
            .await
            .unwrap();
        repo.add_member(user_id, org2_id, OrgRole::OrgAdmin)
            .await
            .unwrap();

        let memberships = repo.list_orgs_for_user(user_id).await.unwrap();
        assert_eq!(memberships.len(), 2);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn remove_member_deletes_record(pool: PgPool) {
        let repo = PgOrgMembershipRepository::new(pool.clone());
        let (org_id, user_id) = seed(&pool).await;

        repo.add_member(user_id, org_id, OrgRole::OrgMember)
            .await
            .unwrap();
        repo.remove_member(user_id, org_id).await.unwrap();

        let found = repo.find_membership(user_id, org_id).await.unwrap();
        assert!(found.is_none());
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn update_role_changes_role(pool: PgPool) {
        let repo = PgOrgMembershipRepository::new(pool.clone());
        let (org_id, user_id) = seed(&pool).await;

        repo.add_member(user_id, org_id, OrgRole::OrgMember)
            .await
            .unwrap();
        repo.update_role(user_id, org_id, OrgRole::OrgAdmin)
            .await
            .unwrap();

        let found = repo
            .find_membership(user_id, org_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(found.role, OrgRole::OrgAdmin);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn update_role_missing_membership_returns_not_found(pool: PgPool) {
        let repo = PgOrgMembershipRepository::new(pool.clone());
        let err = repo
            .update_role(UserId::new(), OrganisationId::new(), OrgRole::OrgAdmin)
            .await
            .unwrap_err();
        assert!(matches!(err, RepositoryError::NotFound { .. }));
    }
}
