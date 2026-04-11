//! Postgres implementation for the `audit` repository.

use async_trait::async_trait;
use sqlx::PgPool;
use uuid::Uuid;

use stitchd_core::id::UserId;

use crate::{RepositoryError, repository::AuditLogger};

/// Postgres-backed implementation of [`AuditLogger`].
pub struct PgAuditLogger {
    pool: PgPool,
}

impl PgAuditLogger {
    /// Construct a new logger bound to `pool`.
    #[must_use]
    pub const fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl AuditLogger for PgAuditLogger {
    async fn log(
        &self,
        actor_id: Option<UserId>,
        resource_type: &str,
        resource_id: Uuid,
        action: &str,
        diff: serde_json::Value,
    ) -> Result<(), RepositoryError> {
        sqlx::query!(
            r#"
            INSERT INTO audit_log (actor_id, resource_type, resource_id, action, diff)
            VALUES ($1, $2, $3, $4, $5)
            "#,
            actor_id as Option<UserId>,
            resource_type,
            resource_id,
            action,
            diff
        )
        .execute(&self.pool)
        .await
        .map_err(RepositoryError::Database)?;

        Ok(())
    }
}
