//! Postgres implementation for the `segment` repository.

use std::sync::Arc;

use async_trait::async_trait;
use sqlx::PgPool;

use stitchd_core::{
    id::{EnvironmentId, SegmentId},
    segment::{Segment, SegmentType},
};

use crate::{
    RepositoryError,
    repository::{AuditLogger, SegmentRepository},
};

/// Postgres-backed implementation of [`SegmentRepository`].
pub struct PgSegmentRepository {
    pool: PgPool,
    audit: Arc<dyn AuditLogger>,
}

impl PgSegmentRepository {
    /// Construct a new repository bound to `pool` and `audit`.
    pub fn new(pool: PgPool, audit: Arc<dyn AuditLogger>) -> Self {
        Self { pool, audit }
    }
}

#[async_trait]
impl SegmentRepository for PgSegmentRepository {
    async fn find_by_id(&self, id: SegmentId) -> Result<Segment, RepositoryError> {
        sqlx::query_as!(
            Segment,
            r#"
            SELECT
                id             AS "id: SegmentId",
                environment_id AS "environment_id: EnvironmentId",
                key,
                segment_type   AS "segment_type: SegmentType",
                created_at,
                updated_at,
                deleted_at,
                version
            FROM segments
            WHERE id = $1 AND deleted_at IS NULL
            "#,
            id as SegmentId
        )
        .fetch_one(&self.pool)
        .await
        .map_err(|e| match e {
            sqlx::Error::RowNotFound => RepositoryError::NotFound { id: id.to_string() },
            other => RepositoryError::Database(other),
        })
    }

    async fn find_by_key(
        &self,
        key: &str,
        environment_id: EnvironmentId,
    ) -> Result<Segment, RepositoryError> {
        sqlx::query_as!(
            Segment,
            r#"
            SELECT
                id             AS "id: SegmentId",
                environment_id AS "environment_id: EnvironmentId",
                key,
                segment_type   AS "segment_type: SegmentType",
                created_at,
                updated_at,
                deleted_at,
                version
            FROM segments
            WHERE key = $1 AND environment_id = $2 AND deleted_at IS NULL
            "#,
            key,
            environment_id as EnvironmentId
        )
        .fetch_one(&self.pool)
        .await
        .map_err(|e| match e {
            sqlx::Error::RowNotFound => RepositoryError::NotFound {
                id: format!("{key}@{environment_id}"),
            },
            other => RepositoryError::Database(other),
        })
    }

    async fn list_by_environment(
        &self,
        environment_id: EnvironmentId,
    ) -> Result<Vec<Segment>, RepositoryError> {
        sqlx::query_as!(
            Segment,
            r#"
            SELECT
                id             AS "id: SegmentId",
                environment_id AS "environment_id: EnvironmentId",
                key,
                segment_type   AS "segment_type: SegmentType",
                created_at,
                updated_at,
                deleted_at,
                version
            FROM segments
            WHERE environment_id = $1 AND deleted_at IS NULL
            ORDER BY created_at
            "#,
            environment_id as EnvironmentId
        )
        .fetch_all(&self.pool)
        .await
        .map_err(RepositoryError::Database)
    }

    async fn create(&self, segment: &Segment) -> Result<(), RepositoryError> {
        sqlx::query!(
            r#"
            INSERT INTO segments
                (id, environment_id, key, segment_type,
                 created_at, updated_at, deleted_at, version)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
            "#,
            segment.id as SegmentId,
            segment.environment_id as EnvironmentId,
            segment.key,
            segment.segment_type as SegmentType,
            segment.created_at,
            segment.updated_at,
            segment.deleted_at,
            segment.version,
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
                "segment",
                segment.id.as_uuid(),
                "create",
                serde_json::json!({
                    "key": segment.key,
                    "environment_id": segment.environment_id.to_string(),
                }),
            )
            .await?;

        Ok(())
    }

    async fn update(&self, segment: &Segment) -> Result<Segment, RepositoryError> {
        let new_version = segment.version + 1;
        let result = sqlx::query_as!(
            Segment,
            r#"
            UPDATE segments
            SET key = $1, segment_type = $2, updated_at = NOW(), version = $3
            WHERE id = $4 AND version = $5 AND deleted_at IS NULL
            RETURNING
                id             AS "id: SegmentId",
                environment_id AS "environment_id: EnvironmentId",
                key,
                segment_type   AS "segment_type: SegmentType",
                created_at,
                updated_at,
                deleted_at,
                version
            "#,
            segment.key,
            segment.segment_type as SegmentType,
            new_version,
            segment.id as SegmentId,
            segment.version,
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(RepositoryError::Database)?;

        if let Some(updated) = result {
            self.audit
                .log(
                    None,
                    "segment",
                    segment.id.as_uuid(),
                    "update",
                    serde_json::json!({ "key": segment.key }),
                )
                .await?;
            Ok(updated)
        } else {
            let current = sqlx::query!(
                r#"SELECT version, deleted_at FROM segments WHERE id = $1"#,
                segment.id as SegmentId
            )
            .fetch_optional(&self.pool)
            .await
            .map_err(RepositoryError::Database)?;

            match current {
                None => Err(RepositoryError::NotFound {
                    id: segment.id.to_string(),
                }),
                Some(row) if row.deleted_at.is_some() => Err(RepositoryError::NotFound {
                    id: segment.id.to_string(),
                }),
                Some(row) => Err(RepositoryError::VersionConflict {
                    expected: segment.version,
                    actual: row.version,
                }),
            }
        }
    }

    async fn soft_delete(&self, id: SegmentId) -> Result<(), RepositoryError> {
        let result = sqlx::query!(
            r#"
            UPDATE segments
            SET deleted_at = NOW(), updated_at = NOW(), version = version + 1
            WHERE id = $1 AND deleted_at IS NULL
            "#,
            id as SegmentId
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
                "segment",
                id.as_uuid(),
                "soft_delete",
                serde_json::json!({}),
            )
            .await?;

        Ok(())
    }
}
