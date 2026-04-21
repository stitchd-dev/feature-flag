//! Invite repository — trait definition and Postgres implementation.
//!
//! Invites are one-time-use tokens that allow a prospective user to join an organisation.
//! The raw token is returned once at creation; only the SHA-256 hash is stored.

use async_trait::async_trait;
use chrono::{Duration, Utc};
use sqlx::PgPool;
use stitchd_core::{
    auth::{Invite, OrgRole},
    id::{InviteId, OrganisationId, UserId},
};

use crate::RepositoryError;

// ---------------------------------------------------------------------------
// Trait
// ---------------------------------------------------------------------------

/// Operations on the `invites` table.
#[async_trait]
pub trait InviteRepository: Send + Sync {
    /// Create a new invite for `email` to join `org_id` with `org_role`.
    ///
    /// Returns `(invite, raw_token)` where `raw_token` is the one-time plaintext
    /// token to send to the recipient. The database stores only the SHA-256 hash.
    async fn create(
        &self,
        org_id: OrganisationId,
        email: &str,
        org_role: OrgRole,
        invited_by: Option<UserId>,
        ttl_hours: i64,
    ) -> Result<(Invite, String), RepositoryError>;

    /// Find a pending (not accepted, not expired) invite by the SHA-256 hash of its token.
    async fn find_by_token_hash(
        &self,
        token_hash: &str,
    ) -> Result<Option<Invite>, RepositoryError>;

    /// Mark an invite as accepted by setting `accepted_at = now()`.
    async fn accept(&self, id: InviteId) -> Result<(), RepositoryError>;

    /// List all invites for an organisation.
    async fn list_for_org(&self, org_id: OrganisationId) -> Result<Vec<Invite>, RepositoryError>;

    /// Delete (revoke) an invite row entirely.
    async fn revoke(&self, id: InviteId) -> Result<(), RepositoryError>;
}

// ---------------------------------------------------------------------------
// Postgres implementation
// ---------------------------------------------------------------------------

/// Postgres-backed [`InviteRepository`].
pub struct PgInviteRepository {
    /// Shared Postgres connection pool.
    pub pool: PgPool,
}

impl PgInviteRepository {
    /// Construct a new repository bound to `pool`.
    #[must_use]
    pub const fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

/// Convert a DB `org_role` TEXT value to [`OrgRole`].
fn parse_org_role(s: &str) -> Result<OrgRole, RepositoryError> {
    match s {
        "org_admin" => Ok(OrgRole::OrgAdmin),
        "org_member" => Ok(OrgRole::OrgMember),
        other => Err(RepositoryError::Unexpected(anyhow::anyhow!(
            "unknown org role: {other}"
        ))),
    }
}

/// Convert [`OrgRole`] to its DB TEXT representation.
const fn org_role_str(r: OrgRole) -> &'static str {
    match r {
        OrgRole::OrgAdmin => "org_admin",
        OrgRole::OrgMember => "org_member",
    }
}

/// Map a raw DB row to an [`Invite`].
macro_rules! map_invite_row {
    ($r:expr) => {
        Invite {
            id: InviteId::from_uuid($r.id),
            org_id: OrganisationId::from_uuid($r.org_id),
            email: $r.email,
            org_role: parse_org_role(&$r.org_role)?,
            invited_by_user_id: $r.invited_by_user_id.map(UserId::from_uuid),
            token_hash: $r.token_hash,
            expires_at: $r.expires_at,
            accepted_at: $r.accepted_at,
            created_at: $r.created_at,
        }
    };
}

#[async_trait]
impl InviteRepository for PgInviteRepository {
    async fn create(
        &self,
        org_id: OrganisationId,
        email: &str,
        org_role: OrgRole,
        invited_by: Option<UserId>,
        ttl_hours: i64,
    ) -> Result<(Invite, String), RepositoryError> {
        let (raw_token, token_hash) =
            stitchd_core::auth::crypto::generate_opaque_token();
        let expires_at = Utc::now() + Duration::hours(ttl_hours);
        let invited_by_uuid = invited_by.map(|u| u.as_uuid());

        let row = sqlx::query!(
            r#"
            INSERT INTO invites
                (org_id, email, org_role, invited_by_user_id, token_hash, expires_at)
            VALUES ($1, $2, $3, $4, $5, $6)
            RETURNING
                id,
                org_id,
                email,
                org_role,
                invited_by_user_id,
                token_hash,
                expires_at,
                accepted_at,
                created_at
            "#,
            org_id.as_uuid(),
            email,
            org_role_str(org_role),
            invited_by_uuid,
            token_hash,
            expires_at,
        )
        .fetch_one(&self.pool)
        .await
        .map_err(RepositoryError::Database)?;

        Ok((map_invite_row!(row), raw_token))
    }

    async fn find_by_token_hash(
        &self,
        token_hash: &str,
    ) -> Result<Option<Invite>, RepositoryError> {
        let row = sqlx::query!(
            r#"
            SELECT
                id,
                org_id,
                email,
                org_role,
                invited_by_user_id,
                token_hash,
                expires_at,
                accepted_at,
                created_at
            FROM invites
            WHERE token_hash = $1
            "#,
            token_hash,
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(RepositoryError::Database)?;

        row.map(|r| Ok(map_invite_row!(r))).transpose()
    }

    async fn accept(&self, id: InviteId) -> Result<(), RepositoryError> {
        let affected = sqlx::query!(
            r#"
            UPDATE invites
            SET accepted_at = now()
            WHERE id = $1 AND accepted_at IS NULL
            "#,
            id.as_uuid(),
        )
        .execute(&self.pool)
        .await
        .map_err(RepositoryError::Database)?
        .rows_affected();

        if affected == 0 {
            return Err(RepositoryError::NotFound {
                id: id.to_string(),
            });
        }
        Ok(())
    }

    async fn list_for_org(&self, org_id: OrganisationId) -> Result<Vec<Invite>, RepositoryError> {
        let rows = sqlx::query!(
            r#"
            SELECT
                id,
                org_id,
                email,
                org_role,
                invited_by_user_id,
                token_hash,
                expires_at,
                accepted_at,
                created_at
            FROM invites
            WHERE org_id = $1
            ORDER BY created_at DESC
            "#,
            org_id.as_uuid(),
        )
        .fetch_all(&self.pool)
        .await
        .map_err(RepositoryError::Database)?;

        rows.into_iter().map(|r| Ok(map_invite_row!(r))).collect()
    }

    async fn revoke(&self, id: InviteId) -> Result<(), RepositoryError> {
        sqlx::query!(
            r#"DELETE FROM invites WHERE id = $1"#,
            id.as_uuid(),
        )
        .execute(&self.pool)
        .await
        .map_err(RepositoryError::Database)?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Integration tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    async fn seed_org(pool: &PgPool) -> OrganisationId {
        let org_id = OrganisationId::new();
        sqlx::query!(
            "INSERT INTO organisations (id, name) VALUES ($1, $2)",
            org_id.as_uuid(),
            "test-org"
        )
        .execute(pool)
        .await
        .unwrap();
        org_id
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn create_invite_returns_invite_and_raw_token(pool: PgPool) {
        let repo = PgInviteRepository::new(pool.clone());
        let org_id = seed_org(&pool).await;

        let (invite, raw_token) = repo
            .create(org_id, "alice@example.com", OrgRole::OrgMember, None, 72)
            .await
            .unwrap();

        assert_eq!(invite.email, "alice@example.com");
        assert_eq!(invite.org_role, OrgRole::OrgMember);
        assert!(invite.accepted_at.is_none());
        assert_eq!(raw_token.len(), 64); // 32 bytes as hex
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn find_by_token_hash_returns_invite(pool: PgPool) {
        let repo = PgInviteRepository::new(pool.clone());
        let org_id = seed_org(&pool).await;

        let (invite, raw_token) = repo
            .create(org_id, "bob@example.com", OrgRole::OrgAdmin, None, 72)
            .await
            .unwrap();

        // Compute the same SHA-256 hash
        use sha2::{Digest, Sha256};
        let raw_bytes = hex::decode(&raw_token).unwrap();
        let hash = hex::encode(Sha256::digest(raw_bytes));

        let found = repo.find_by_token_hash(&hash).await.unwrap();
        assert!(found.is_some());
        assert_eq!(found.unwrap().id, invite.id);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn accept_sets_accepted_at(pool: PgPool) {
        let repo = PgInviteRepository::new(pool.clone());
        let org_id = seed_org(&pool).await;

        let (invite, _) = repo
            .create(org_id, "carol@example.com", OrgRole::OrgMember, None, 72)
            .await
            .unwrap();

        repo.accept(invite.id).await.unwrap();

        // Re-fetch via list
        let invites = repo.list_for_org(org_id).await.unwrap();
        let updated = invites.iter().find(|i| i.id == invite.id).unwrap();
        assert!(updated.accepted_at.is_some());
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn list_for_org_returns_all_invites(pool: PgPool) {
        let repo = PgInviteRepository::new(pool.clone());
        let org_id = seed_org(&pool).await;

        repo.create(org_id, "d@example.com", OrgRole::OrgMember, None, 72)
            .await
            .unwrap();
        repo.create(org_id, "e@example.com", OrgRole::OrgAdmin, None, 72)
            .await
            .unwrap();

        let invites = repo.list_for_org(org_id).await.unwrap();
        assert_eq!(invites.len(), 2);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn revoke_deletes_invite(pool: PgPool) {
        let repo = PgInviteRepository::new(pool.clone());
        let org_id = seed_org(&pool).await;

        let (invite, _) = repo
            .create(org_id, "frank@example.com", OrgRole::OrgMember, None, 72)
            .await
            .unwrap();

        repo.revoke(invite.id).await.unwrap();

        let invites = repo.list_for_org(org_id).await.unwrap();
        assert!(invites.is_empty());
    }
}
