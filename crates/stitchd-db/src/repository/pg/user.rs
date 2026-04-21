//! Postgres implementation for the `user` repository (new auth schema).
//!
//! Uses the platform-level `users` table introduced in migration 20260421000001.

use std::sync::Arc;

use async_trait::async_trait;
use sqlx::PgPool;

use stitchd_core::{
    auth::{User, UserStatus},
    id::{OrganisationId, ProjectId, UserId},
    user::Permission,
};

use crate::{
    RepositoryError,
    repository::{AuditLogger, UserRepository},
};

/// Postgres-backed implementation of [`UserRepository`] (new auth schema).
pub struct PgUserRepository {
    pool: PgPool,
    audit: Arc<dyn AuditLogger>,
}

impl PgUserRepository {
    /// Construct a new repository bound to `pool` and `audit`.
    pub fn new(pool: PgPool, audit: Arc<dyn AuditLogger>) -> Self {
        Self { pool, audit }
    }
}

/// Map a DB text `status` column to [`UserStatus`].
fn parse_user_status(s: &str) -> Result<UserStatus, RepositoryError> {
    match s {
        "active" => Ok(UserStatus::Active),
        "deactivated" => Ok(UserStatus::Deactivated),
        other => Err(RepositoryError::Unexpected(anyhow::anyhow!(
            "unknown user status: {other}"
        ))),
    }
}

#[async_trait]
impl UserRepository for PgUserRepository {
    async fn find_by_id(&self, id: UserId) -> Result<User, RepositoryError> {
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
            id.as_uuid()
        )
        .fetch_one(&self.pool)
        .await
        .map_err(|e| match e {
            sqlx::Error::RowNotFound => RepositoryError::NotFound { id: id.to_string() },
            other => RepositoryError::Database(other),
        })?;

        Ok(User {
            id: UserId::from_uuid(row.id),
            email: row.email,
            display_name: row.display_name,
            avatar_url: row.avatar_url,
            password_hash: row.password_hash,
            token_secret: row.token_secret,
            totp_secret: row.totp_secret,
            totp_enabled: row.totp_enabled,
            status: parse_user_status(&row.status)?,
            created_at: row.created_at,
            updated_at: row.updated_at,
        })
    }

    async fn find_by_email(&self, email: &str) -> Result<User, RepositoryError> {
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
            email
        )
        .fetch_one(&self.pool)
        .await
        .map_err(|e| match e {
            sqlx::Error::RowNotFound => RepositoryError::NotFound {
                id: email.to_string(),
            },
            other => RepositoryError::Database(other),
        })?;

        Ok(User {
            id: UserId::from_uuid(row.id),
            email: row.email,
            display_name: row.display_name,
            avatar_url: row.avatar_url,
            password_hash: row.password_hash,
            token_secret: row.token_secret,
            totp_secret: row.totp_secret,
            totp_enabled: row.totp_enabled,
            status: parse_user_status(&row.status)?,
            created_at: row.created_at,
            updated_at: row.updated_at,
        })
    }

    async fn list_by_organisation(
        &self,
        organisation_id: OrganisationId,
    ) -> Result<Vec<User>, RepositoryError> {
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
                u.updated_at
            FROM users u
            JOIN org_memberships om ON om.user_id = u.id
            WHERE om.org_id = $1
            ORDER BY u.created_at
            "#,
            organisation_id.as_uuid()
        )
        .fetch_all(&self.pool)
        .await
        .map_err(RepositoryError::Database)?;

        rows.into_iter()
            .map(|r| {
                Ok(User {
                    id: UserId::from_uuid(r.id),
                    email: r.email,
                    display_name: r.display_name,
                    avatar_url: r.avatar_url,
                    password_hash: r.password_hash,
                    token_secret: r.token_secret,
                    totp_secret: r.totp_secret,
                    totp_enabled: r.totp_enabled,
                    status: parse_user_status(&r.status)?,
                    created_at: r.created_at,
                    updated_at: r.updated_at,
                })
            })
            .collect()
    }

    async fn create(&self, user: &User) -> Result<(), RepositoryError> {
        sqlx::query!(
            r#"
            INSERT INTO users
                (id, email, display_name, avatar_url, password_hash,
                 token_secret, totp_secret, totp_enabled, status,
                 created_at, updated_at)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
            "#,
            user.id.as_uuid(),
            user.email,
            user.display_name,
            user.avatar_url,
            user.password_hash,
            user.token_secret,
            user.totp_secret.as_deref(),
            user.totp_enabled,
            user.status.to_db_str(),
            user.created_at,
            user.updated_at,
        )
        .execute(&self.pool)
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

        self.audit
            .log(
                None,
                "user",
                user.id.as_uuid(),
                "create",
                serde_json::json!({ "email": user.email }),
            )
            .await?;

        Ok(())
    }

    async fn update(&self, user: &User) -> Result<User, RepositoryError> {
        let row = sqlx::query!(
            r#"
            UPDATE users
            SET
                email        = $1,
                display_name = $2,
                avatar_url   = $3,
                password_hash = $4,
                token_secret  = $5,
                totp_secret   = $6,
                totp_enabled  = $7,
                status        = $8,
                updated_at    = now()
            WHERE id = $9
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
            user.email,
            user.display_name,
            user.avatar_url,
            user.password_hash,
            user.token_secret,
            user.totp_secret.as_deref(),
            user.totp_enabled,
            user.status.to_db_str(),
            user.id.as_uuid(),
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(RepositoryError::Database)?;

        match row {
            Some(r) => {
                self.audit
                    .log(
                        None,
                        "user",
                        user.id.as_uuid(),
                        "update",
                        serde_json::json!({ "email": user.email }),
                    )
                    .await?;
                Ok(User {
                    id: UserId::from_uuid(r.id),
                    email: r.email,
                    display_name: r.display_name,
                    avatar_url: r.avatar_url,
                    password_hash: r.password_hash,
                    token_secret: r.token_secret,
                    totp_secret: r.totp_secret,
                    totp_enabled: r.totp_enabled,
                    status: parse_user_status(&r.status)?,
                    created_at: r.created_at,
                    updated_at: r.updated_at,
                })
            }
            None => Err(RepositoryError::NotFound {
                id: user.id.to_string(),
            }),
        }
    }

    async fn find_permissions_for_user(
        &self,
        user_id: UserId,
        project_id: ProjectId,
    ) -> Result<Vec<Permission>, RepositoryError> {
        // New schema: user_project_roles has (user_id, project_id, role TEXT).
        // The old role_id FK and roles/permissions tables are still present but
        // user_project_roles no longer has a role_id column.
        // We return an empty vec — permission queries via RBAC role table are
        // no longer supported after the auth schema migration.
        // TODO: implement role-based permission lookup for new schema.
        let _ = (user_id, project_id);
        Ok(vec![])
    }
}

// ---------------------------------------------------------------------------
// Helper trait for converting UserStatus to DB string
// ---------------------------------------------------------------------------

trait UserStatusExt {
    fn to_db_str(&self) -> &'static str;
}

impl UserStatusExt for UserStatus {
    fn to_db_str(&self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Deactivated => "deactivated",
        }
    }
}

#[cfg(test)]
mod unit_tests {
    use super::*;

    #[test]
    fn parse_user_status_active() {
        assert!(matches!(
            parse_user_status("active").unwrap(),
            UserStatus::Active
        ));
    }

    #[test]
    fn parse_user_status_deactivated() {
        assert!(matches!(
            parse_user_status("deactivated").unwrap(),
            UserStatus::Deactivated
        ));
    }

    #[test]
    fn parse_user_status_unknown_returns_error() {
        assert!(parse_user_status("banned").is_err());
    }
}
