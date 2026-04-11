//! Postgres implementation for the `project` repository.

use std::sync::Arc;

use async_trait::async_trait;
use sqlx::PgPool;

use stitchd_core::{
    id::{OrganisationId, ProjectId},
    tenant::Project,
};

use crate::{
    RepositoryError,
    repository::{AuditLogger, ProjectRepository},
};

/// Postgres-backed implementation of [`ProjectRepository`].
pub struct PgProjectRepository {
    pool: PgPool,
    audit: Arc<dyn AuditLogger>,
}

impl PgProjectRepository {
    /// Construct a new repository bound to `pool` and `audit`.
    pub fn new(pool: PgPool, audit: Arc<dyn AuditLogger>) -> Self {
        Self { pool, audit }
    }
}

#[async_trait]
impl ProjectRepository for PgProjectRepository {
    async fn find_by_id(&self, id: ProjectId) -> Result<Project, RepositoryError> {
        sqlx::query_as!(
            Project,
            r#"
            SELECT
                id              AS "id: ProjectId",
                organisation_id AS "organisation_id: OrganisationId",
                name,
                created_at,
                updated_at,
                deleted_at,
                version
            FROM projects
            WHERE id = $1 AND deleted_at IS NULL
            "#,
            id as ProjectId
        )
        .fetch_one(&self.pool)
        .await
        .map_err(|e| match e {
            sqlx::Error::RowNotFound => RepositoryError::NotFound { id: id.to_string() },
            other => RepositoryError::Database(other),
        })
    }

    async fn list_by_organisation(
        &self,
        organisation_id: OrganisationId,
    ) -> Result<Vec<Project>, RepositoryError> {
        sqlx::query_as!(
            Project,
            r#"
            SELECT
                id              AS "id: ProjectId",
                organisation_id AS "organisation_id: OrganisationId",
                name,
                created_at,
                updated_at,
                deleted_at,
                version
            FROM projects
            WHERE organisation_id = $1 AND deleted_at IS NULL
            ORDER BY created_at
            "#,
            organisation_id as OrganisationId
        )
        .fetch_all(&self.pool)
        .await
        .map_err(RepositoryError::Database)
    }

    async fn create(&self, project: &Project) -> Result<(), RepositoryError> {
        sqlx::query!(
            r#"
            INSERT INTO projects
                (id, organisation_id, name, created_at, updated_at, deleted_at, version)
            VALUES ($1, $2, $3, $4, $5, $6, $7)
            "#,
            project.id as ProjectId,
            project.organisation_id as OrganisationId,
            project.name,
            project.created_at,
            project.updated_at,
            project.deleted_at,
            project.version,
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
                "project",
                project.id.as_uuid(),
                "create",
                serde_json::json!({
                    "name": project.name,
                    "organisation_id": project.organisation_id.to_string(),
                }),
            )
            .await?;

        Ok(())
    }

    async fn update(&self, project: &Project) -> Result<Project, RepositoryError> {
        let new_version = project.version + 1;
        let result = sqlx::query_as!(
            Project,
            r#"
            UPDATE projects
            SET name = $1, updated_at = NOW(), version = $2
            WHERE id = $3 AND version = $4 AND deleted_at IS NULL
            RETURNING
                id              AS "id: ProjectId",
                organisation_id AS "organisation_id: OrganisationId",
                name,
                created_at,
                updated_at,
                deleted_at,
                version
            "#,
            project.name,
            new_version,
            project.id as ProjectId,
            project.version,
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(RepositoryError::Database)?;

        if let Some(updated) = result {
            self.audit
                .log(
                    None,
                    "project",
                    project.id.as_uuid(),
                    "update",
                    serde_json::json!({ "name": project.name }),
                )
                .await?;
            Ok(updated)
        } else {
            let current = sqlx::query!(
                r#"SELECT version, deleted_at FROM projects WHERE id = $1"#,
                project.id as ProjectId
            )
            .fetch_optional(&self.pool)
            .await
            .map_err(RepositoryError::Database)?;

            match current {
                None => Err(RepositoryError::NotFound {
                    id: project.id.to_string(),
                }),
                Some(row) if row.deleted_at.is_some() => Err(RepositoryError::NotFound {
                    id: project.id.to_string(),
                }),
                Some(row) => Err(RepositoryError::VersionConflict {
                    expected: project.version,
                    actual: row.version,
                }),
            }
        }
    }

    async fn soft_delete(&self, id: ProjectId) -> Result<(), RepositoryError> {
        let result = sqlx::query!(
            r#"
            UPDATE projects
            SET deleted_at = NOW(), updated_at = NOW(), version = version + 1
            WHERE id = $1 AND deleted_at IS NULL
            "#,
            id as ProjectId,
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
                "project",
                id.as_uuid(),
                "soft_delete",
                serde_json::json!({}),
            )
            .await?;

        Ok(())
    }
}
