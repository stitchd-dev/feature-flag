//! Postgres implementation for the `environment` repository.

use std::sync::Arc;

use async_trait::async_trait;
use sqlx::PgPool;

use stitchd_core::{
    id::{EnvironmentId, ProjectId},
    tenant::Environment,
};

use crate::{
    RepositoryError,
    repository::{AuditLogger, EnvironmentRepository},
};

/// Postgres-backed implementation of [`EnvironmentRepository`].
pub struct PgEnvironmentRepository {
    pool: PgPool,
    audit: Arc<dyn AuditLogger>,
}

impl PgEnvironmentRepository {
    /// Construct a new repository bound to `pool` and `audit`.
    pub fn new(pool: PgPool, audit: Arc<dyn AuditLogger>) -> Self {
        Self { pool, audit }
    }
}

#[async_trait]
impl EnvironmentRepository for PgEnvironmentRepository {
    async fn find_by_id(&self, id: EnvironmentId) -> Result<Environment, RepositoryError> {
        sqlx::query_as!(
            Environment,
            r#"
            SELECT
                id         AS "id: EnvironmentId",
                project_id AS "project_id: ProjectId",
                name,
                created_at,
                updated_at,
                deleted_at,
                version
            FROM environments
            WHERE id = $1 AND deleted_at IS NULL
            "#,
            id as EnvironmentId
        )
        .fetch_one(&self.pool)
        .await
        .map_err(|e| match e {
            sqlx::Error::RowNotFound => RepositoryError::NotFound { id: id.to_string() },
            other => RepositoryError::Database(other),
        })
    }

    async fn list_by_project(
        &self,
        project_id: ProjectId,
    ) -> Result<Vec<Environment>, RepositoryError> {
        sqlx::query_as!(
            Environment,
            r#"
            SELECT
                id         AS "id: EnvironmentId",
                project_id AS "project_id: ProjectId",
                name,
                created_at,
                updated_at,
                deleted_at,
                version
            FROM environments
            WHERE project_id = $1 AND deleted_at IS NULL
            ORDER BY created_at
            "#,
            project_id as ProjectId
        )
        .fetch_all(&self.pool)
        .await
        .map_err(RepositoryError::Database)
    }

    async fn create(&self, env: &Environment) -> Result<(), RepositoryError> {
        sqlx::query!(
            r#"
            INSERT INTO environments
                (id, project_id, name, created_at, updated_at, deleted_at, version)
            VALUES ($1, $2, $3, $4, $5, $6, $7)
            "#,
            env.id as EnvironmentId,
            env.project_id as ProjectId,
            env.name,
            env.created_at,
            env.updated_at,
            env.deleted_at,
            env.version,
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
                "environment",
                env.id.as_uuid(),
                "create",
                serde_json::json!({
                    "name": env.name,
                    "project_id": env.project_id.to_string(),
                }),
            )
            .await?;

        Ok(())
    }

    async fn update(&self, env: &Environment) -> Result<Environment, RepositoryError> {
        let new_version = env.version + 1;
        let result = sqlx::query_as!(
            Environment,
            r#"
            UPDATE environments
            SET name = $1, updated_at = NOW(), version = $2
            WHERE id = $3 AND version = $4 AND deleted_at IS NULL
            RETURNING
                id         AS "id: EnvironmentId",
                project_id AS "project_id: ProjectId",
                name,
                created_at,
                updated_at,
                deleted_at,
                version
            "#,
            env.name,
            new_version,
            env.id as EnvironmentId,
            env.version,
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(RepositoryError::Database)?;

        if let Some(updated) = result {
            self.audit
                .log(
                    None,
                    "environment",
                    env.id.as_uuid(),
                    "update",
                    serde_json::json!({ "name": env.name }),
                )
                .await?;
            Ok(updated)
        } else {
            let current = sqlx::query!(
                r#"SELECT version, deleted_at FROM environments WHERE id = $1"#,
                env.id as EnvironmentId
            )
            .fetch_optional(&self.pool)
            .await
            .map_err(RepositoryError::Database)?;

            match current {
                None => Err(RepositoryError::NotFound {
                    id: env.id.to_string(),
                }),
                Some(row) if row.deleted_at.is_some() => Err(RepositoryError::NotFound {
                    id: env.id.to_string(),
                }),
                Some(row) => Err(RepositoryError::VersionConflict {
                    expected: env.version,
                    actual: row.version,
                }),
            }
        }
    }

    async fn soft_delete(&self, id: EnvironmentId) -> Result<(), RepositoryError> {
        let result = sqlx::query!(
            r#"
            UPDATE environments
            SET deleted_at = NOW(), updated_at = NOW(), version = version + 1
            WHERE id = $1 AND deleted_at IS NULL
            "#,
            id as EnvironmentId,
        )
        .execute(&self.pool)
        .await
        .map_err(RepositoryError::Database)?;

        if result.rows_affected() == 0 {
            return Err(RepositoryError::NotFound { id: id.to_string() });
        }

        self.audit
            .log(
                None,
                "environment",
                id.as_uuid(),
                "soft_delete",
                serde_json::json!({}),
            )
            .await?;

        Ok(())
    }
}
