//! Postgres implementation for the `user` repository.

use std::sync::Arc;

use async_trait::async_trait;
use sqlx::PgPool;

use stitchd_core::{
    id::{OrganisationId, ProjectId, UserId},
    user::{Action, Permission, ResourceType, User},
};

use crate::{
    RepositoryError,
    repository::{AuditLogger, UserRepository},
};

/// Postgres-backed implementation of [`UserRepository`].
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

#[async_trait]
impl UserRepository for PgUserRepository {
    async fn find_by_id(&self, id: UserId) -> Result<User, RepositoryError> {
        sqlx::query_as!(
            User,
            r#"
            SELECT
                id              AS "id: UserId",
                email,
                password_hash,
                organisation_id AS "organisation_id: OrganisationId",
                created_at,
                updated_at,
                deleted_at,
                version
            FROM users
            WHERE id = $1
            "#,
            id as UserId
        )
        .fetch_one(&self.pool)
        .await
        .map_err(|e| match e {
            sqlx::Error::RowNotFound => RepositoryError::NotFound { id: id.to_string() },
            other => RepositoryError::Database(other),
        })
    }

    async fn find_by_email(
        &self,
        email: &str,
        organisation_id: OrganisationId,
    ) -> Result<User, RepositoryError> {
        sqlx::query_as!(
            User,
            r#"
            SELECT
                id              AS "id: UserId",
                email,
                password_hash,
                organisation_id AS "organisation_id: OrganisationId",
                created_at,
                updated_at,
                deleted_at,
                version
            FROM users
            WHERE email = $1 AND organisation_id = $2 AND deleted_at IS NULL
            "#,
            email,
            organisation_id as OrganisationId
        )
        .fetch_one(&self.pool)
        .await
        .map_err(|e| match e {
            sqlx::Error::RowNotFound => RepositoryError::NotFound {
                id: format!("{email}@{organisation_id}"),
            },
            other => RepositoryError::Database(other),
        })
    }

    async fn list_by_organisation(
        &self,
        organisation_id: OrganisationId,
    ) -> Result<Vec<User>, RepositoryError> {
        sqlx::query_as!(
            User,
            r#"
            SELECT
                id              AS "id: UserId",
                email,
                password_hash,
                organisation_id AS "organisation_id: OrganisationId",
                created_at,
                updated_at,
                deleted_at,
                version
            FROM users
            WHERE organisation_id = $1 AND deleted_at IS NULL
            ORDER BY created_at
            "#,
            organisation_id as OrganisationId
        )
        .fetch_all(&self.pool)
        .await
        .map_err(RepositoryError::Database)
    }

    async fn create(&self, user: &User) -> Result<(), RepositoryError> {
        sqlx::query!(
            r#"
            INSERT INTO users
                (id, organisation_id, email, password_hash,
                 created_at, updated_at, deleted_at, version)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
            "#,
            user.id as UserId,
            user.organisation_id as OrganisationId,
            user.email,
            user.password_hash,
            user.created_at,
            user.updated_at,
            user.deleted_at,
            user.version,
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
                serde_json::json!({
                    "email": user.email,
                    "organisation_id": user.organisation_id.to_string(),
                }),
            )
            .await?;

        Ok(())
    }

    async fn update(&self, user: &User) -> Result<User, RepositoryError> {
        let new_version = user.version + 1;
        let result = sqlx::query_as!(
            User,
            r#"
            UPDATE users
            SET email = $1, updated_at = NOW(), version = $2
            WHERE id = $3 AND version = $4 AND deleted_at IS NULL
            RETURNING
                id              AS "id: UserId",
                email,
                password_hash,
                organisation_id AS "organisation_id: OrganisationId",
                created_at,
                updated_at,
                deleted_at,
                version
            "#,
            user.email,
            new_version,
            user.id as UserId,
            user.version,
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(RepositoryError::Database)?;

        if let Some(updated) = result {
            self.audit
                .log(
                    None,
                    "user",
                    user.id.as_uuid(),
                    "update",
                    serde_json::json!({ "email": user.email }),
                )
                .await?;
            Ok(updated)
        } else {
            let current = sqlx::query!(
                r#"SELECT version, deleted_at FROM users WHERE id = $1"#,
                user.id as UserId
            )
            .fetch_optional(&self.pool)
            .await
            .map_err(RepositoryError::Database)?;

            match current {
                None => Err(RepositoryError::NotFound {
                    id: user.id.to_string(),
                }),
                Some(row) if row.deleted_at.is_some() => Err(RepositoryError::NotFound {
                    id: user.id.to_string(),
                }),
                Some(row) => Err(RepositoryError::VersionConflict {
                    expected: user.version,
                    actual: row.version,
                }),
            }
        }
    }

    async fn find_permissions_for_user(
        &self,
        user_id: UserId,
        project_id: ProjectId,
    ) -> Result<Vec<Permission>, RepositoryError> {
        // JOIN: user_project_roles → roles → permissions
        // Filter out soft-deleted roles.
        let rows = sqlx::query!(
            r#"
            SELECT
                p.resource_type,
                p.resource_pattern,
                p.action
            FROM user_project_roles upr
            JOIN roles r ON r.id = upr.role_id AND r.deleted_at IS NULL
            JOIN permissions p ON p.role_id = r.id
            WHERE upr.user_id = $1
              AND upr.project_id = $2
            "#,
            user_id as UserId,
            project_id as ProjectId
        )
        .fetch_all(&self.pool)
        .await
        .map_err(RepositoryError::Database)?;

        rows.into_iter()
            .map(|row| {
                let resource_type = parse_resource_type(&row.resource_type)?;
                let action = parse_action(&row.action)?;
                Ok(Permission {
                    resource_type,
                    resource_pattern: row.resource_pattern,
                    action,
                })
            })
            .collect()
    }
}

fn parse_resource_type(s: &str) -> Result<ResourceType, RepositoryError> {
    match s {
        "environment" => Ok(ResourceType::Environment),
        "flag" => Ok(ResourceType::Flag),
        "segment" => Ok(ResourceType::Segment),
        other => Err(RepositoryError::Unexpected(anyhow::anyhow!(
            "unknown resource_type: {other}"
        ))),
    }
}

fn parse_action(s: &str) -> Result<Action, RepositoryError> {
    match s {
        "read" => Ok(Action::Read),
        "write" => Ok(Action::Write),
        "publish" => Ok(Action::Publish),
        "admin" => Ok(Action::Admin),
        other => Err(RepositoryError::Unexpected(anyhow::anyhow!(
            "unknown action: {other}"
        ))),
    }
}
