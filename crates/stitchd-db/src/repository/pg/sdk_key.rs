//! Postgres implementation for the `sdk_key` repository.

use std::sync::Arc;

use async_trait::async_trait;
use sqlx::PgPool;

use stitchd_core::{
    id::{EnvironmentId, SdkKeyId},
    tenant::SdkKey,
};

use crate::{
    RepositoryError,
    repository::{AuditLogger, SdkKeyRepository},
};

/// Postgres-backed implementation of [`SdkKeyRepository`].
pub struct PgSdkKeyRepository {
    pool: PgPool,
    audit: Arc<dyn AuditLogger>,
}

impl PgSdkKeyRepository {
    /// Construct a new repository bound to `pool` and `audit`.
    pub fn new(pool: PgPool, audit: Arc<dyn AuditLogger>) -> Self {
        Self { pool, audit }
    }
}

#[async_trait]
impl SdkKeyRepository for PgSdkKeyRepository {
    async fn find_by_id(&self, id: SdkKeyId) -> Result<SdkKey, RepositoryError> {
        sqlx::query_as!(
            SdkKey,
            r#"
            SELECT
                id             AS "id: SdkKeyId",
                environment_id AS "environment_id: EnvironmentId",
                key_hash,
                is_active,
                created_at,
                revoked_at
            FROM sdk_keys
            WHERE id = $1
            "#,
            id as SdkKeyId
        )
        .fetch_one(&self.pool)
        .await
        .map_err(|e| match e {
            sqlx::Error::RowNotFound => RepositoryError::NotFound { id: id.to_string() },
            other => RepositoryError::Database(other),
        })
    }

    async fn list_by_environment(
        &self,
        environment_id: EnvironmentId,
    ) -> Result<Vec<SdkKey>, RepositoryError> {
        sqlx::query_as!(
            SdkKey,
            r#"
            SELECT
                id             AS "id: SdkKeyId",
                environment_id AS "environment_id: EnvironmentId",
                key_hash,
                is_active,
                created_at,
                revoked_at
            FROM sdk_keys
            WHERE environment_id = $1
            ORDER BY created_at
            "#,
            environment_id as EnvironmentId
        )
        .fetch_all(&self.pool)
        .await
        .map_err(RepositoryError::Database)
    }

    async fn create(&self, key: &SdkKey) -> Result<(), RepositoryError> {
        sqlx::query!(
            r#"
            INSERT INTO sdk_keys (id, environment_id, key_hash, is_active, created_at, revoked_at)
            VALUES ($1, $2, $3, $4, $5, $6)
            "#,
            key.id as SdkKeyId,
            key.environment_id as EnvironmentId,
            key.key_hash,
            key.is_active,
            key.created_at,
            key.revoked_at,
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
                "sdk_key",
                key.id.as_uuid(),
                "create",
                serde_json::json!({
                    "environment_id": key.environment_id.to_string(),
                    "is_active": key.is_active,
                }),
            )
            .await?;

        Ok(())
    }

    async fn find_active_by_environment(
        &self,
        environment_id: EnvironmentId,
    ) -> Result<Vec<SdkKey>, RepositoryError> {
        // sqlx::query (non-macro) to avoid breaking offline mode for new queries.
        use sqlx::Row as _;
        let rows = sqlx::query(
            r"
            SELECT id, environment_id, key_hash, is_active, created_at, revoked_at
            FROM sdk_keys
            WHERE environment_id = $1 AND is_active = TRUE
            ORDER BY created_at
            ",
        )
        .bind(environment_id.as_uuid())
        .fetch_all(&self.pool)
        .await
        .map_err(RepositoryError::Database)?;

        rows.into_iter()
            .map(|row| {
                Ok(SdkKey {
                    id: SdkKeyId::from_uuid(row.get("id")),
                    environment_id: EnvironmentId::from_uuid(row.get("environment_id")),
                    key_hash: row.get("key_hash"),
                    is_active: row.get("is_active"),
                    created_at: row.get("created_at"),
                    revoked_at: row.get("revoked_at"),
                })
            })
            .collect()
    }

    async fn revoke(&self, id: SdkKeyId) -> Result<(), RepositoryError> {
        // Fetch the key's environment so we can count remaining active keys.
        let key = sqlx::query!(
            r#"
            SELECT environment_id AS "environment_id: EnvironmentId", is_active
            FROM sdk_keys
            WHERE id = $1
            "#,
            id as SdkKeyId
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(RepositoryError::Database)?
        .ok_or_else(|| RepositoryError::NotFound { id: id.to_string() })?;

        if !key.is_active {
            // Already revoked — nothing to do.
            return Ok(());
        }

        // Count how many active keys the environment currently has.
        let active_count: i64 = sqlx::query_scalar!(
            r#"
            SELECT COUNT(*) AS "count!"
            FROM sdk_keys
            WHERE environment_id = $1 AND is_active = TRUE
            "#,
            key.environment_id as EnvironmentId
        )
        .fetch_one(&self.pool)
        .await
        .map_err(RepositoryError::Database)?;

        if active_count <= 1 {
            return Err(RepositoryError::UniqueViolation {
                field: "is_active".to_string(),
            });
        }

        sqlx::query!(
            r#"
            UPDATE sdk_keys
            SET is_active = FALSE, revoked_at = NOW()
            WHERE id = $1
            "#,
            id as SdkKeyId,
        )
        .execute(&self.pool)
        .await
        .map_err(RepositoryError::Database)?;

        self.audit
            .log(
                None,
                "sdk_key",
                id.as_uuid(),
                "revoke",
                serde_json::json!({ "is_active": false }),
            )
            .await?;

        Ok(())
    }
}
