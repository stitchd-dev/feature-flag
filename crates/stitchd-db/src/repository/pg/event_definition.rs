//! Postgres implementation for the `event_definition` repository.

use std::sync::Arc;

use async_trait::async_trait;
use sqlx::PgPool;

use stitchd_core::{
    event::{EventDefinition, EventValueType},
    id::{EnvironmentId, EventDefinitionId},
};

use crate::{
    RepositoryError,
    repository::{AuditLogger, EventDefinitionRepository},
};

/// Postgres-backed implementation of [`EventDefinitionRepository`].
pub struct PgEventDefinitionRepository {
    pool: PgPool,
    audit: Arc<dyn AuditLogger>,
}

impl PgEventDefinitionRepository {
    /// Construct a new repository bound to `pool` and `audit`.
    pub fn new(pool: PgPool, audit: Arc<dyn AuditLogger>) -> Self {
        Self { pool, audit }
    }
}

#[async_trait]
impl EventDefinitionRepository for PgEventDefinitionRepository {
    async fn find_by_id(&self, id: EventDefinitionId) -> Result<EventDefinition, RepositoryError> {
        sqlx::query_as!(
            EventDefinition,
            r#"
            SELECT
                id             AS "id: EventDefinitionId",
                environment_id AS "environment_id: EnvironmentId",
                key,
                value_type     AS "value_type: EventValueType",
                created_at,
                updated_at,
                deleted_at,
                version
            FROM event_definitions
            WHERE id = $1 AND deleted_at IS NULL
            "#,
            id as EventDefinitionId,
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
    ) -> Result<EventDefinition, RepositoryError> {
        sqlx::query_as!(
            EventDefinition,
            r#"
            SELECT
                id             AS "id: EventDefinitionId",
                environment_id AS "environment_id: EnvironmentId",
                key,
                value_type     AS "value_type: EventValueType",
                created_at,
                updated_at,
                deleted_at,
                version
            FROM event_definitions
            WHERE key = $1 AND environment_id = $2 AND deleted_at IS NULL
            "#,
            key,
            environment_id as EnvironmentId,
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
    ) -> Result<Vec<EventDefinition>, RepositoryError> {
        sqlx::query_as!(
            EventDefinition,
            r#"
            SELECT
                id             AS "id: EventDefinitionId",
                environment_id AS "environment_id: EnvironmentId",
                key,
                value_type     AS "value_type: EventValueType",
                created_at,
                updated_at,
                deleted_at,
                version
            FROM event_definitions
            WHERE environment_id = $1 AND deleted_at IS NULL
            ORDER BY created_at
            "#,
            environment_id as EnvironmentId,
        )
        .fetch_all(&self.pool)
        .await
        .map_err(RepositoryError::Database)
    }

    async fn create(&self, def: &EventDefinition) -> Result<(), RepositoryError> {
        sqlx::query!(
            r#"
            INSERT INTO event_definitions
                (id, environment_id, key, value_type, created_at, updated_at, deleted_at, version)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
            "#,
            def.id as EventDefinitionId,
            def.environment_id as EnvironmentId,
            def.key,
            def.value_type as EventValueType,
            def.created_at,
            def.updated_at,
            def.deleted_at,
            def.version,
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
                "event_definition",
                def.id.as_uuid(),
                "create",
                serde_json::json!({
                    "key": def.key,
                    "environment_id": def.environment_id.to_string(),
                    "value_type": format!("{:?}", def.value_type),
                }),
            )
            .await?;

        Ok(())
    }

    async fn update(
        &self,
        def: &EventDefinition,
    ) -> Result<EventDefinition, RepositoryError> {
        let new_version = def.version + 1;
        let result = sqlx::query_as!(
            EventDefinition,
            r#"
            UPDATE event_definitions
            SET key = $1, value_type = $2, updated_at = NOW(), version = $3
            WHERE id = $4 AND version = $5 AND deleted_at IS NULL
            RETURNING
                id             AS "id: EventDefinitionId",
                environment_id AS "environment_id: EnvironmentId",
                key,
                value_type     AS "value_type: EventValueType",
                created_at,
                updated_at,
                deleted_at,
                version
            "#,
            def.key,
            def.value_type as EventValueType,
            new_version,
            def.id as EventDefinitionId,
            def.version,
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(RepositoryError::Database)?;

        match result {
            Some(updated) => {
                self.audit
                    .log(
                        None,
                        "event_definition",
                        def.id.as_uuid(),
                        "update",
                        serde_json::json!({
                            "key": def.key,
                            "value_type": format!("{:?}", def.value_type),
                            "version": new_version,
                        }),
                    )
                    .await?;
                Ok(updated)
            }
            None => {
                // Either the row doesn't exist or the version didn't match.
                // Distinguish by looking up the current version.
                let current = sqlx::query_scalar!(
                    "SELECT version FROM event_definitions WHERE id = $1 AND deleted_at IS NULL",
                    def.id as EventDefinitionId
                )
                .fetch_optional(&self.pool)
                .await
                .map_err(RepositoryError::Database)?;

                match current {
                    Some(current_version) => Err(RepositoryError::VersionConflict {
                        expected: def.version,
                        actual: current_version,
                    }),
                    None => Err(RepositoryError::NotFound {
                        id: def.id.to_string(),
                    }),
                }
            }
        }
    }

    async fn soft_delete(&self, id: EventDefinitionId) -> Result<(), RepositoryError> {
        let rows = sqlx::query!(
            r#"
            UPDATE event_definitions
            SET deleted_at = NOW(), updated_at = NOW()
            WHERE id = $1 AND deleted_at IS NULL
            "#,
            id as EventDefinitionId,
        )
        .execute(&self.pool)
        .await
        .map_err(RepositoryError::Database)?
        .rows_affected();

        if rows == 0 {
            return Err(RepositoryError::NotFound { id: id.to_string() });
        }

        self.audit
            .log(
                None,
                "event_definition",
                id.as_uuid(),
                "soft_delete",
                serde_json::json!({}),
            )
            .await?;

        Ok(())
    }
}
