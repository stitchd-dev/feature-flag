//! Auth user repository — trait definition and Postgres implementation.
//!
//! Provides [`AuthUserRepository`] trait with focused auth-specific mutations
//! (create, find, rotate token secret, update status/password/profile) and
//! [`PgAuthUserRepository`] backed by PostgreSQL.

use async_trait::async_trait;
use chrono::Utc;
use sqlx::PgPool;
use stitchd_core::{
    auth::{OrgRole, User, UserStatus},
    id::{OrganisationId, UserId},
};

use crate::RepositoryError;

// ---------------------------------------------------------------------------
// Trait
// ---------------------------------------------------------------------------

/// Auth-focused user repository — distinct from the existing [`crate::repository::UserRepository`]
/// which handles RBAC/permission queries. This trait covers auth-specific operations.
#[async_trait]
pub trait AuthUserRepository: Send + Sync {
    /// Create a new platform user.
    async fn create(
        &self,
        email: &str,
        display_name: &str,
        password_hash: Option<&str>,
    ) -> Result<User, RepositoryError>;

    /// Find a user by email address (globally unique). Returns `None` if absent.
    async fn find_by_email(&self, email: &str) -> Result<Option<User>, RepositoryError>;

    /// Find a user by ID. Returns `None` if absent.
    async fn find_by_id(&self, id: UserId) -> Result<Option<User>, RepositoryError>;

    /// Rotate the per-user `token_secret` UUID, immediately invalidating all
    /// previously-issued JWTs. Returns the new secret.
    async fn rotate_token_secret(&self, user_id: UserId) -> Result<uuid::Uuid, RepositoryError>;

    /// Change a user's lifecycle status.
    async fn update_status(
        &self,
        user_id: UserId,
        status: UserStatus,
    ) -> Result<(), RepositoryError>;

    /// Replace a user's stored Argon2id password hash.
    async fn update_password_hash(
        &self,
        user_id: UserId,
        hash: &str,
    ) -> Result<(), RepositoryError>;

    /// Update displayable profile fields.
    async fn update_profile(
        &self,
        user_id: UserId,
        display_name: &str,
        avatar_url: Option<&str>,
    ) -> Result<(), RepositoryError>;

    /// List all users that are members of `org_id`, along with their org role.
    async fn list_org_users(
        &self,
        org_id: OrganisationId,
    ) -> Result<Vec<(User, OrgRole)>, RepositoryError>;
}

// ---------------------------------------------------------------------------
// Postgres implementation
// ---------------------------------------------------------------------------

/// Postgres-backed [`AuthUserRepository`].
pub struct PgAuthUserRepository {
    /// Shared Postgres connection pool.
    pub pool: PgPool,
}

impl PgAuthUserRepository {
    /// Construct a new repository bound to `pool`.
    #[must_use]
    pub const fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

/// Convert a DB `status` TEXT value to [`UserStatus`].
fn parse_status(s: &str) -> Result<UserStatus, RepositoryError> {
    match s {
        "active" => Ok(UserStatus::Active),
        "deactivated" => Ok(UserStatus::Deactivated),
        other => Err(RepositoryError::Unexpected(anyhow::anyhow!(
            "unknown user status: {other}"
        ))),
    }
}

/// Convert [`UserStatus`] to its DB TEXT representation.
const fn status_str(s: UserStatus) -> &'static str {
    match s {
        UserStatus::Active => "active",
        UserStatus::Deactivated => "deactivated",
    }
}

/// Map a raw DB row to a [`User`].
macro_rules! map_row {
    ($r:expr) => {
        User {
            id: UserId::from_uuid($r.id),
            email: $r.email,
            display_name: $r.display_name,
            avatar_url: $r.avatar_url,
            password_hash: $r.password_hash,
            token_secret: $r.token_secret,
            totp_secret: $r.totp_secret,
            totp_enabled: $r.totp_enabled,
            status: parse_status(&$r.status)?,
            created_at: $r.created_at,
            updated_at: $r.updated_at,
        }
    };
}

#[async_trait]
impl AuthUserRepository for PgAuthUserRepository {
    async fn create(
        &self,
        email: &str,
        display_name: &str,
        password_hash: Option<&str>,
    ) -> Result<User, RepositoryError> {
        let now = Utc::now();
        let row = sqlx::query!(
            r#"
            INSERT INTO users
                (email, display_name, password_hash, status, created_at, updated_at)
            VALUES ($1, $2, $3, 'active', $4, $4)
            RETURNING
                id,
                email,
                display_name,
                avatar_url,
                password_hash,
                token_secret,
                totp_secret,
                totp_enabled,
                status,
                created_at,
                updated_at
            "#,
            email,
            display_name,
            password_hash,
            now,
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

        Ok(map_row!(row))
    }

    async fn find_by_email(&self, email: &str) -> Result<Option<User>, RepositoryError> {
        let row = sqlx::query!(
            r#"
            SELECT
                id,
                email,
                display_name,
                avatar_url,
                password_hash,
                token_secret,
                totp_secret,
                totp_enabled,
                status,
                created_at,
                updated_at
            FROM users
            WHERE email = $1
            "#,
            email,
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(RepositoryError::Database)?;

        row.map(|r| Ok(map_row!(r))).transpose()
    }

    async fn find_by_id(&self, id: UserId) -> Result<Option<User>, RepositoryError> {
        let row = sqlx::query!(
            r#"
            SELECT
                id,
                email,
                display_name,
                avatar_url,
                password_hash,
                token_secret,
                totp_secret,
                totp_enabled,
                status,
                created_at,
                updated_at
            FROM users
            WHERE id = $1
            "#,
            id.as_uuid(),
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(RepositoryError::Database)?;

        row.map(|r| Ok(map_row!(r))).transpose()
    }

    async fn rotate_token_secret(&self, user_id: UserId) -> Result<uuid::Uuid, RepositoryError> {
        let row = sqlx::query!(
            r#"
            UPDATE users
            SET token_secret = gen_random_uuid(), updated_at = now()
            WHERE id = $1
            RETURNING token_secret
            "#,
            user_id.as_uuid(),
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(RepositoryError::Database)?;

        row.map(|r| r.token_secret)
            .ok_or_else(|| RepositoryError::NotFound {
                id: user_id.to_string(),
            })
    }

    async fn update_status(
        &self,
        user_id: UserId,
        status: UserStatus,
    ) -> Result<(), RepositoryError> {
        let affected = sqlx::query!(
            r#"
            UPDATE users
            SET status = $1, updated_at = now()
            WHERE id = $2
            "#,
            status_str(status),
            user_id.as_uuid(),
        )
        .execute(&self.pool)
        .await
        .map_err(RepositoryError::Database)?
        .rows_affected();

        if affected == 0 {
            return Err(RepositoryError::NotFound {
                id: user_id.to_string(),
            });
        }
        Ok(())
    }

    async fn update_password_hash(
        &self,
        user_id: UserId,
        hash: &str,
    ) -> Result<(), RepositoryError> {
        let affected = sqlx::query!(
            r#"
            UPDATE users
            SET password_hash = $1, updated_at = now()
            WHERE id = $2
            "#,
            hash,
            user_id.as_uuid(),
        )
        .execute(&self.pool)
        .await
        .map_err(RepositoryError::Database)?
        .rows_affected();

        if affected == 0 {
            return Err(RepositoryError::NotFound {
                id: user_id.to_string(),
            });
        }
        Ok(())
    }

    async fn update_profile(
        &self,
        user_id: UserId,
        display_name: &str,
        avatar_url: Option<&str>,
    ) -> Result<(), RepositoryError> {
        let affected = sqlx::query!(
            r#"
            UPDATE users
            SET display_name = $1, avatar_url = $2, updated_at = now()
            WHERE id = $3
            "#,
            display_name,
            avatar_url,
            user_id.as_uuid(),
        )
        .execute(&self.pool)
        .await
        .map_err(RepositoryError::Database)?
        .rows_affected();

        if affected == 0 {
            return Err(RepositoryError::NotFound {
                id: user_id.to_string(),
            });
        }
        Ok(())
    }

    async fn list_org_users(
        &self,
        org_id: OrganisationId,
    ) -> Result<Vec<(User, OrgRole)>, RepositoryError> {
        let rows = sqlx::query!(
            r#"
            SELECT
                u.id,
                u.email,
                u.display_name,
                u.avatar_url,
                u.password_hash,
                u.token_secret,
                u.totp_secret,
                u.totp_enabled,
                u.status,
                u.created_at,
                u.updated_at,
                m.role AS org_role
            FROM users u
            JOIN org_memberships m ON u.id = m.user_id
            WHERE m.org_id = $1
            ORDER BY u.email
            "#,
            org_id.as_uuid(),
        )
        .fetch_all(&self.pool)
        .await
        .map_err(RepositoryError::Database)?;

        rows.into_iter()
            .map(|r| {
                let org_role = match r.org_role.as_str() {
                    "org_admin" => OrgRole::OrgAdmin,
                    _ => OrgRole::OrgMember,
                };
                let user = User {
                    id: UserId::from_uuid(r.id),
                    email: r.email,
                    display_name: r.display_name,
                    avatar_url: r.avatar_url,
                    password_hash: r.password_hash,
                    token_secret: r.token_secret,
                    totp_secret: r.totp_secret,
                    totp_enabled: r.totp_enabled,
                    status: parse_status(&r.status)?,
                    created_at: r.created_at,
                    updated_at: r.updated_at,
                };
                Ok((user, org_role))
            })
            .collect()
    }
}

// ---------------------------------------------------------------------------
// Integration tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[allow(dead_code)]
    async fn seed_org(pool: &PgPool) -> stitchd_core::id::OrganisationId {
        let org_id = stitchd_core::id::OrganisationId::new();
        sqlx::query!(
            "INSERT INTO organisations (id, name, is_system) VALUES ($1, $2, false)",
            org_id.as_uuid(),
            "test-org"
        )
        .execute(pool)
        .await
        .unwrap();
        org_id
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn create_user_returns_user(pool: PgPool) {
        let repo = PgAuthUserRepository::new(pool);
        let user = repo
            .create("alice@example.com", "Alice", Some("hash123"))
            .await
            .unwrap();

        assert_eq!(user.email, "alice@example.com");
        assert_eq!(user.display_name, "Alice");
        assert_eq!(user.password_hash.as_deref(), Some("hash123"));
        assert_eq!(user.status, UserStatus::Active);
        assert!(!user.totp_enabled);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn create_duplicate_email_returns_unique_violation(pool: PgPool) {
        let repo = PgAuthUserRepository::new(pool);
        repo.create("dup@example.com", "First", None).await.unwrap();
        let err = repo
            .create("dup@example.com", "Second", None)
            .await
            .unwrap_err();
        assert!(
            matches!(err, RepositoryError::UniqueViolation { .. }),
            "expected UniqueViolation, got {err:?}"
        );
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn find_by_email_existing_user(pool: PgPool) {
        let repo = PgAuthUserRepository::new(pool);
        let created = repo.create("bob@example.com", "Bob", None).await.unwrap();
        let found = repo.find_by_email("bob@example.com").await.unwrap();
        assert!(found.is_some());
        assert_eq!(found.unwrap().id, created.id);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn find_by_email_missing_returns_none(pool: PgPool) {
        let repo = PgAuthUserRepository::new(pool);
        let found = repo.find_by_email("nobody@example.com").await.unwrap();
        assert!(found.is_none());
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn find_by_id_existing_user(pool: PgPool) {
        let repo = PgAuthUserRepository::new(pool);
        let created = repo
            .create("carol@example.com", "Carol", None)
            .await
            .unwrap();
        let found = repo.find_by_id(created.id).await.unwrap();
        assert!(found.is_some());
        assert_eq!(found.unwrap().id, created.id);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn find_by_id_missing_returns_none(pool: PgPool) {
        let repo = PgAuthUserRepository::new(pool);
        let found = repo.find_by_id(UserId::new()).await.unwrap();
        assert!(found.is_none());
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn rotate_token_secret_changes_value(pool: PgPool) {
        let repo = PgAuthUserRepository::new(pool);
        let user = repo.create("dave@example.com", "Dave", None).await.unwrap();
        let original = user.token_secret;

        let new_secret = repo.rotate_token_secret(user.id).await.unwrap();
        assert_ne!(new_secret, original);

        // Verify persisted
        let refreshed = repo.find_by_id(user.id).await.unwrap().unwrap();
        assert_eq!(refreshed.token_secret, new_secret);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn rotate_token_secret_unknown_user_returns_not_found(pool: PgPool) {
        let repo = PgAuthUserRepository::new(pool);
        let err = repo.rotate_token_secret(UserId::new()).await.unwrap_err();
        assert!(matches!(err, RepositoryError::NotFound { .. }));
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn update_status_deactivates_user(pool: PgPool) {
        let repo = PgAuthUserRepository::new(pool);
        let user = repo.create("eve@example.com", "Eve", None).await.unwrap();
        repo.update_status(user.id, UserStatus::Deactivated)
            .await
            .unwrap();

        let found = repo.find_by_id(user.id).await.unwrap().unwrap();
        assert_eq!(found.status, UserStatus::Deactivated);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn update_password_hash_stores_new_hash(pool: PgPool) {
        let repo = PgAuthUserRepository::new(pool);
        let user = repo
            .create("frank@example.com", "Frank", None)
            .await
            .unwrap();
        repo.update_password_hash(user.id, "newhash456")
            .await
            .unwrap();

        let found = repo.find_by_id(user.id).await.unwrap().unwrap();
        assert_eq!(found.password_hash.as_deref(), Some("newhash456"));
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn update_profile_stores_new_values(pool: PgPool) {
        let repo = PgAuthUserRepository::new(pool);
        let user = repo
            .create("grace@example.com", "Grace", None)
            .await
            .unwrap();
        repo.update_profile(
            user.id,
            "Grace Updated",
            Some("https://example.com/avatar.png"),
        )
        .await
        .unwrap();

        let found = repo.find_by_id(user.id).await.unwrap().unwrap();
        assert_eq!(found.display_name, "Grace Updated");
        assert_eq!(
            found.avatar_url.as_deref(),
            Some("https://example.com/avatar.png")
        );
    }
}
