//! Postgres implementation for the `organisation` repository.

use std::sync::Arc;

use async_trait::async_trait;
use sqlx::PgPool;

use stitchd_core::{id::OrganisationId, tenant::Organisation};

use crate::{
    RepositoryError,
    repository::{AuditLogger, OrganisationRepository},
};

/// Postgres-backed implementation of [`OrganisationRepository`].
pub struct PgOrganisationRepository {
    pool: PgPool,
    audit: Arc<dyn AuditLogger>,
}

impl PgOrganisationRepository {
    /// Construct a new repository bound to `pool` and `audit`.
    pub fn new(pool: PgPool, audit: Arc<dyn AuditLogger>) -> Self {
        Self { pool, audit }
    }
}

#[async_trait]
impl OrganisationRepository for PgOrganisationRepository {
    async fn find_by_id(&self, id: OrganisationId) -> Result<Organisation, RepositoryError> {
        sqlx::query_as!(
            Organisation,
            r#"
            SELECT
                id         AS "id: OrganisationId",
                name,
                created_at,
                updated_at,
                deleted_at,
                version,
                is_system
            FROM organisations
            WHERE id = $1 AND deleted_at IS NULL
            "#,
            id as OrganisationId
        )
        .fetch_one(&self.pool)
        .await
        .map_err(|e| match e {
            sqlx::Error::RowNotFound => RepositoryError::NotFound { id: id.to_string() },
            other => RepositoryError::Database(other),
        })
    }

    async fn list_all(&self) -> Result<Vec<Organisation>, RepositoryError> {
        sqlx::query_as!(
            Organisation,
            r#"
            SELECT
                id         AS "id: OrganisationId",
                name,
                created_at,
                updated_at,
                deleted_at,
                version,
                is_system
            FROM organisations
            WHERE deleted_at IS NULL
            ORDER BY created_at
            "#
        )
        .fetch_all(&self.pool)
        .await
        .map_err(RepositoryError::Database)
    }

    async fn create(&self, org: &Organisation) -> Result<(), RepositoryError> {
        sqlx::query!(
            r#"
            INSERT INTO organisations (id, name, created_at, updated_at, deleted_at, version, is_system)
            VALUES ($1, $2, $3, $4, $5, $6, $7)
            "#,
            org.id as OrganisationId,
            org.name,
            org.created_at,
            org.updated_at,
            org.deleted_at,
            org.version,
            org.is_system,
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
                "organisation",
                org.id.as_uuid(),
                "create",
                serde_json::json!({ "name": org.name, "is_system": org.is_system }),
            )
            .await?;

        Ok(())
    }

    async fn update(&self, org: &Organisation) -> Result<Organisation, RepositoryError> {
        let new_version = org.version + 1;
        let result = sqlx::query_as!(
            Organisation,
            r#"
            UPDATE organisations
            SET name = $1, updated_at = NOW(), version = $2
            WHERE id = $3 AND version = $4 AND deleted_at IS NULL AND is_system = false
            RETURNING
                id         AS "id: OrganisationId",
                name,
                created_at,
                updated_at,
                deleted_at,
                version,
                is_system
            "#,
            org.name,
            new_version,
            org.id as OrganisationId,
            org.version,
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(RepositoryError::Database)?;

        if let Some(updated) = result {
            self.audit
                .log(
                    None,
                    "organisation",
                    org.id.as_uuid(),
                    "update",
                    serde_json::json!({ "name": org.name }),
                )
                .await?;
            Ok(updated)
        } else {
            let current = sqlx::query!(
                r#"SELECT version, deleted_at, is_system FROM organisations WHERE id = $1"#,
                org.id as OrganisationId
            )
            .fetch_optional(&self.pool)
            .await
            .map_err(RepositoryError::Database)?;

            match current {
                None => Err(RepositoryError::NotFound {
                    id: org.id.to_string(),
                }),
                Some(row) if row.deleted_at.is_some() => Err(RepositoryError::NotFound {
                    id: org.id.to_string(),
                }),
                Some(row) if row.is_system => Err(RepositoryError::InvalidState {
                    reason: "cannot modify the System organisation".to_string(),
                }),
                Some(row) => Err(RepositoryError::VersionConflict {
                    expected: org.version,
                    actual: row.version,
                }),
            }
        }
    }

    async fn soft_delete(&self, id: OrganisationId) -> Result<(), RepositoryError> {
        // Refuse to delete the System org at the DB layer.
        let row = sqlx::query!(
            r#"SELECT is_system FROM organisations WHERE id = $1 AND deleted_at IS NULL"#,
            id as OrganisationId,
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(RepositoryError::Database)?
        .ok_or_else(|| RepositoryError::NotFound { id: id.to_string() })?;

        if row.is_system {
            return Err(RepositoryError::InvalidState {
                reason: "cannot delete the System organisation".to_string(),
            });
        }

        let result = sqlx::query!(
            r#"
            UPDATE organisations
            SET deleted_at = NOW(), updated_at = NOW(), version = version + 1
            WHERE id = $1 AND deleted_at IS NULL
            "#,
            id as OrganisationId,
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
                "organisation",
                id.as_uuid(),
                "soft_delete",
                serde_json::json!({}),
            )
            .await?;

        Ok(())
    }
}
