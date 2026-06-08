//! Postgres implementation for the `sdk_key` repository.

use std::sync::Arc;

use async_trait::async_trait;
use sqlx::{PgPool, Row as _};

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

fn row_to_sdk_key(row: &sqlx::postgres::PgRow) -> SdkKey {
    SdkKey {
        id: SdkKeyId::from_uuid(row.get("id")),
        environment_id: EnvironmentId::from_uuid(row.get("environment_id")),
        key_hash: row.get("key_hash"),
        name: row.try_get("name").unwrap_or_default(),
        is_active: row.get("is_active"),
        created_at: row.get("created_at"),
        revoked_at: row.get("revoked_at"),
    }
}

#[async_trait]
impl SdkKeyRepository for PgSdkKeyRepository {
    async fn find_by_id(&self, id: SdkKeyId) -> Result<SdkKey, RepositoryError> {
        let row = sqlx::query(
            r"
            SELECT id, environment_id, key_hash, name, is_active, created_at, revoked_at
            FROM sdk_keys
            WHERE id = $1
            ",
        )
        .bind(id.as_uuid())
        .fetch_one(&self.pool)
        .await
        .map_err(|e| match e {
            sqlx::Error::RowNotFound => RepositoryError::NotFound { id: id.to_string() },
            other => RepositoryError::Database(other),
        })?;

        Ok(row_to_sdk_key(&row))
    }

    async fn list_by_environment(
        &self,
        environment_id: EnvironmentId,
    ) -> Result<Vec<SdkKey>, RepositoryError> {
        let rows = sqlx::query(
            r"
            SELECT id, environment_id, key_hash, name, is_active, created_at, revoked_at
            FROM sdk_keys
            WHERE environment_id = $1
            ORDER BY created_at
            ",
        )
        .bind(environment_id.as_uuid())
        .fetch_all(&self.pool)
        .await
        .map_err(RepositoryError::Database)?;

        Ok(rows.iter().map(row_to_sdk_key).collect())
    }

    async fn list_by_environment_keyset(
        &self,
        environment_id: EnvironmentId,
        after: Option<crate::KeysetCursor>,
        limit: u64,
    ) -> Result<(Vec<SdkKey>, Option<String>), RepositoryError> {
        // Keyset pagination ordered by (created_at, id): fetch limit+1 rows; the
        // surplus row signals a next page. The row-value comparison
        // `(created_at, id) > ($cursor_created_at, $cursor_id)` resumes strictly
        // after the prior page's last row. A NULL cursor (first page) admits all.
        // sdk_keys has no soft-delete column — revoked keys stay listed.
        #[allow(clippy::cast_possible_wrap)]
        let rows = sqlx::query(
            r"
            SELECT id, environment_id, key_hash, name, is_active, created_at, revoked_at
            FROM sdk_keys
            WHERE environment_id = $1
              AND ($2::timestamptz IS NULL OR (created_at, id) > ($2, $3))
            ORDER BY created_at, id
            LIMIT $4
            ",
        )
        .bind(environment_id.as_uuid())
        .bind(after.map(|c| c.created_at))
        .bind(after.map(|c| c.id))
        .bind((limit + 1) as i64)
        .fetch_all(&self.pool)
        .await
        .map_err(RepositoryError::Database)?;

        let mut keys: Vec<SdkKey> = rows.iter().map(row_to_sdk_key).collect();

        // limit+1 rows ⇒ more pages: drop the surplus and encode the new last
        // row's keyset as the next cursor.
        let next_cursor = if keys.len() as u64 > limit {
            keys.truncate(usize::try_from(limit).unwrap_or(usize::MAX));
            keys.last().map(|k| {
                crate::KeysetCursor {
                    created_at: k.created_at,
                    id: k.id.as_uuid(),
                }
                .encode()
            })
        } else {
            None
        };

        Ok((keys, next_cursor))
    }

    async fn create(&self, key: &SdkKey) -> Result<(), RepositoryError> {
        sqlx::query(
            r"
            INSERT INTO sdk_keys (id, environment_id, key_hash, name, is_active, created_at, revoked_at)
            VALUES ($1, $2, $3, $4, $5, $6, $7)
            ",
        )
        .bind(key.id.as_uuid())
        .bind(key.environment_id.as_uuid())
        .bind(&key.key_hash)
        .bind(&key.name)
        .bind(key.is_active)
        .bind(key.created_at)
        .bind(key.revoked_at)
        .execute(&self.pool)
        .await
        .map_err(|e| {
            if let sqlx::Error::Database(ref dbe) = e
                && let Some(constraint) = dbe.constraint()
            {
                return RepositoryError::UniqueViolation {
                    field: constraint.to_string(),
                };
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
                    "name": key.name,
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
        let rows = sqlx::query(
            r"
            SELECT id, environment_id, key_hash, name, is_active, created_at, revoked_at
            FROM sdk_keys
            WHERE environment_id = $1 AND is_active = TRUE
            ORDER BY created_at
            ",
        )
        .bind(environment_id.as_uuid())
        .fetch_all(&self.pool)
        .await
        .map_err(RepositoryError::Database)?;

        Ok(rows.iter().map(row_to_sdk_key).collect())
    }

    async fn find_active_by_hash(&self, key_hash: &str) -> Result<SdkKey, RepositoryError> {
        let row = sqlx::query(
            r"
            SELECT id, environment_id, key_hash, name, is_active, created_at, revoked_at
            FROM sdk_keys
            WHERE key_hash = $1 AND is_active = TRUE
            LIMIT 1
            ",
        )
        .bind(key_hash)
        .fetch_optional(&self.pool)
        .await
        .map_err(RepositoryError::Database)?
        .ok_or_else(|| RepositoryError::NotFound {
            id: "<sdk_key_by_hash>".to_string(),
        })?;

        Ok(row_to_sdk_key(&row))
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
