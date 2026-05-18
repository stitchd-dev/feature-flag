use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sqlx::PgPool;
use std::str::FromStr;

use stitchd_core::{
    context::{ContextParamRecord, ContextTypeRecord, InferredType},
    id::EnvironmentId,
};

use crate::{RepositoryError, repository::ContextRegistryRepository};

/// Postgres-backed implementation of [`ContextRegistryRepository`].
pub struct PgContextRegistryRepository {
    pool: PgPool,
}

impl PgContextRegistryRepository {
    /// Construct a new repository bound to `pool`.
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[derive(sqlx::FromRow)]
struct ContextTypeRow {
    env_id: EnvironmentId,
    context_type: String,
    first_seen_at: DateTime<Utc>,
    last_seen_at: DateTime<Utc>,
}

#[derive(sqlx::FromRow)]
struct ContextParamRow {
    env_id: EnvironmentId,
    context_type: String,
    param_key: String,
    inferred_type: String,
    is_private: bool,
    first_seen_at: DateTime<Utc>,
    last_seen_at: DateTime<Utc>,
}

#[async_trait]
impl ContextRegistryRepository for PgContextRegistryRepository {
    async fn upsert_context_type(
        &self,
        env_id: EnvironmentId,
        context_type: &str,
    ) -> Result<(), RepositoryError> {
        sqlx::query(
            r#"
            INSERT INTO context_type_registry (env_id, context_type, first_seen_at, last_seen_at)
            VALUES ($1, $2, NOW(), NOW())
            ON CONFLICT (env_id, context_type)
            DO UPDATE SET last_seen_at = NOW()
            "#,
        )
        .bind(env_id.as_uuid())
        .bind(context_type)
        .execute(&self.pool)
        .await
        .map_err(RepositoryError::Database)?;
        Ok(())
    }

    async fn upsert_param(
        &self,
        env_id: EnvironmentId,
        context_type: &str,
        param_key: &str,
        inferred_type: InferredType,
        is_private: bool,
    ) -> Result<(), RepositoryError> {
        sqlx::query(
            r#"
            INSERT INTO context_param_registry
                (env_id, context_type, param_key, inferred_type, is_private,
                 first_seen_at, last_seen_at)
            VALUES ($1, $2, $3, $4, $5, NOW(), NOW())
            ON CONFLICT (env_id, context_type, param_key)
            DO UPDATE SET inferred_type = EXCLUDED.inferred_type,
                          is_private    = EXCLUDED.is_private,
                          last_seen_at  = NOW()
            "#,
        )
        .bind(env_id.as_uuid())
        .bind(context_type)
        .bind(param_key)
        .bind(inferred_type.as_str())
        .bind(is_private)
        .execute(&self.pool)
        .await
        .map_err(RepositoryError::Database)?;
        Ok(())
    }

    async fn list_types(
        &self,
        env_id: EnvironmentId,
    ) -> Result<Vec<ContextTypeRecord>, RepositoryError> {
        let rows: Vec<ContextTypeRow> = sqlx::query_as(
            r#"
            SELECT env_id, context_type, first_seen_at, last_seen_at
            FROM context_type_registry
            WHERE env_id = $1
              AND last_seen_at >= NOW() - INTERVAL '90 days'
            ORDER BY last_seen_at DESC
            "#,
        )
        .bind(env_id.as_uuid())
        .fetch_all(&self.pool)
        .await
        .map_err(RepositoryError::Database)?;

        Ok(rows
            .into_iter()
            .map(|r| ContextTypeRecord {
                env_id: r.env_id,
                context_type: r.context_type,
                first_seen_at: r.first_seen_at,
                last_seen_at: r.last_seen_at,
            })
            .collect())
    }

    async fn list_params(
        &self,
        env_id: EnvironmentId,
        context_type: &str,
    ) -> Result<Vec<ContextParamRecord>, RepositoryError> {
        let rows: Vec<ContextParamRow> = sqlx::query_as(
            r#"
            SELECT env_id, context_type, param_key, inferred_type, is_private,
                   first_seen_at, last_seen_at
            FROM context_param_registry
            WHERE env_id = $1
              AND context_type = $2
              AND last_seen_at >= NOW() - INTERVAL '90 days'
            ORDER BY param_key ASC
            "#,
        )
        .bind(env_id.as_uuid())
        .bind(context_type)
        .fetch_all(&self.pool)
        .await
        .map_err(RepositoryError::Database)?;

        Ok(rows
            .into_iter()
            .map(|r| ContextParamRecord {
                env_id: r.env_id,
                context_type: r.context_type,
                param_key: r.param_key,
                inferred_type: InferredType::from_str(&r.inferred_type)
                    .unwrap_or(InferredType::Unknown),
                is_private: r.is_private,
                first_seen_at: r.first_seen_at,
                last_seen_at: r.last_seen_at,
            })
            .collect())
    }

    async fn purge_stale(&self, older_than: DateTime<Utc>) -> Result<(), RepositoryError> {
        sqlx::query(r#"DELETE FROM context_param_registry WHERE last_seen_at < $1"#)
            .bind(older_than)
            .execute(&self.pool)
            .await
            .map_err(RepositoryError::Database)?;

        sqlx::query(r#"DELETE FROM context_type_registry WHERE last_seen_at < $1"#)
            .bind(older_than)
            .execute(&self.pool)
            .await
            .map_err(RepositoryError::Database)?;

        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use stitchd_core::context::InferredType;

    #[sqlx::test(migrations = "./migrations")]
    async fn upsert_context_type_creates_row(pool: PgPool) {
        let repo = PgContextRegistryRepository::new(pool.clone());
        let env_id = EnvironmentId::new();

        repo.upsert_context_type(env_id, "user").await.unwrap();

        let count: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM context_type_registry WHERE context_type = 'user'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(count.0, 1);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn upsert_context_type_is_idempotent(pool: PgPool) {
        let repo = PgContextRegistryRepository::new(pool.clone());
        let env_id = EnvironmentId::new();

        repo.upsert_context_type(env_id, "user").await.unwrap();
        repo.upsert_context_type(env_id, "user").await.unwrap();

        let count: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM context_type_registry WHERE context_type = 'user'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(count.0, 1, "upsert must not create duplicate rows");
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn upsert_param_creates_row(pool: PgPool) {
        let repo = PgContextRegistryRepository::new(pool.clone());
        let env_id = EnvironmentId::new();

        repo.upsert_param(env_id, "user", "email", InferredType::Str, true)
            .await
            .unwrap();

        let count: (i64,) =
            sqlx::query_as("SELECT COUNT(*) FROM context_param_registry WHERE param_key = 'email'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(count.0, 1);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn upsert_param_updates_type_and_privacy(pool: PgPool) {
        let repo = PgContextRegistryRepository::new(pool.clone());
        let env_id = EnvironmentId::new();

        repo.upsert_param(env_id, "user", "age", InferredType::Str, false)
            .await
            .unwrap();
        repo.upsert_param(env_id, "user", "age", InferredType::Int, false)
            .await
            .unwrap();

        let row: (String, bool) = sqlx::query_as(
            "SELECT inferred_type, is_private FROM context_param_registry WHERE param_key = 'age'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(row.0, "int");
        assert!(!row.1);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn list_types_returns_within_90_days(pool: PgPool) {
        let repo = PgContextRegistryRepository::new(pool.clone());
        let env_id = EnvironmentId::new();

        // Recent entry
        repo.upsert_context_type(env_id, "user").await.unwrap();

        // Stale entry (91 days old)
        sqlx::query(
            "INSERT INTO context_type_registry (env_id, context_type, first_seen_at, last_seen_at)
             VALUES ($1, 'stale', NOW() - INTERVAL '91 days', NOW() - INTERVAL '91 days')",
        )
        .bind(env_id.as_uuid())
        .execute(&pool)
        .await
        .unwrap();

        let types = repo.list_types(env_id).await.unwrap();
        assert_eq!(types.len(), 1);
        assert_eq!(types[0].context_type, "user");
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn list_params_returns_within_90_days(pool: PgPool) {
        let repo = PgContextRegistryRepository::new(pool.clone());
        let env_id = EnvironmentId::new();

        repo.upsert_param(env_id, "user", "plan", InferredType::Str, false)
            .await
            .unwrap();

        // Stale param
        sqlx::query(
            "INSERT INTO context_param_registry
             (env_id, context_type, param_key, inferred_type, is_private, first_seen_at, last_seen_at)
             VALUES ($1, 'user', 'stale_key', 'str', false, NOW() - INTERVAL '91 days', NOW() - INTERVAL '91 days')",
        )
        .bind(env_id.as_uuid())
        .execute(&pool)
        .await
        .unwrap();

        let params = repo.list_params(env_id, "user").await.unwrap();
        assert_eq!(params.len(), 1);
        assert_eq!(params[0].param_key, "plan");
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn purge_stale_deletes_old_entries(pool: PgPool) {
        let repo = PgContextRegistryRepository::new(pool.clone());
        let env_id = EnvironmentId::new();

        let cutoff = chrono::Utc::now() - chrono::Duration::days(90);

        // Insert an entry that is older than cutoff
        sqlx::query(
            "INSERT INTO context_type_registry (env_id, context_type, first_seen_at, last_seen_at)
             VALUES ($1, 'old_type', NOW() - INTERVAL '91 days', NOW() - INTERVAL '91 days')",
        )
        .bind(env_id.as_uuid())
        .execute(&pool)
        .await
        .unwrap();

        // Insert a recent entry
        repo.upsert_context_type(env_id, "new_type").await.unwrap();

        repo.purge_stale(cutoff).await.unwrap();

        let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM context_type_registry")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count.0, 1, "only the recent entry should remain");
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn list_types_ordered_by_last_seen_desc(pool: PgPool) {
        let repo = PgContextRegistryRepository::new(pool.clone());
        let env_id = EnvironmentId::new();

        sqlx::query(
            "INSERT INTO context_type_registry (env_id, context_type, first_seen_at, last_seen_at)
             VALUES ($1, 'older', NOW() - INTERVAL '10 days', NOW() - INTERVAL '10 days'),
                    ($1, 'newer', NOW() - INTERVAL '1 day',  NOW() - INTERVAL '1 day')",
        )
        .bind(env_id.as_uuid())
        .execute(&pool)
        .await
        .unwrap();

        let types = repo.list_types(env_id).await.unwrap();
        assert_eq!(types.len(), 2);
        assert_eq!(types[0].context_type, "newer");
        assert_eq!(types[1].context_type, "older");
    }
}
