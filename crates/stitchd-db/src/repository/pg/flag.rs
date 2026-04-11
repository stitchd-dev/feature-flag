//! Postgres implementations for the `flag` and `variant` repositories.

use std::sync::Arc;

use async_trait::async_trait;
use sqlx::PgPool;

use stitchd_core::{
    flag::{FlagRecord, FlagValueType, Variant, VariantValue},
    id::{FlagId, FlagKey, ProjectId, VariantId},
};

use crate::{
    repository::{AuditLogger, FlagRepository, VariantRepository},
    RepositoryError,
};

// ---------------------------------------------------------------------------
// Type-conversion helpers
// ---------------------------------------------------------------------------

fn parse_flag_value_type(s: &str) -> Result<FlagValueType, RepositoryError> {
    match s {
        "bool" => Ok(FlagValueType::Bool),
        "int" => Ok(FlagValueType::Int),
        "double" => Ok(FlagValueType::Double),
        "str" => Ok(FlagValueType::Str),
        "json" => Ok(FlagValueType::Json),
        other => Err(RepositoryError::Unexpected(anyhow::anyhow!(
            "unknown flag value_type: {other}"
        ))),
    }
}

const fn flag_value_type_to_str(fvt: FlagValueType) -> &'static str {
    match fvt {
        FlagValueType::Bool => "bool",
        FlagValueType::Int => "int",
        FlagValueType::Double => "double",
        FlagValueType::Str => "str",
        FlagValueType::Json => "json",
    }
}

/// Assemble a [`FlagRecord`] from raw DB columns.
// Nine parameters mirrors the nine columns in feature_flags SELECT — no useful grouping.
#[allow(clippy::too_many_arguments)]
fn assemble_flag(
    id: uuid::Uuid,
    project_id: uuid::Uuid,
    key: String,
    value_type: &str,
    enabled: bool,
    created_at: chrono::DateTime<chrono::Utc>,
    updated_at: chrono::DateTime<chrono::Utc>,
    deleted_at: Option<chrono::DateTime<chrono::Utc>>,
    version: i64,
) -> Result<FlagRecord, RepositoryError> {
    let value_type = parse_flag_value_type(value_type)?;
    let key = FlagKey::new(key).map_err(|e| {
        RepositoryError::Unexpected(anyhow::anyhow!("invalid flag key stored in DB: {e}"))
    })?;
    Ok(FlagRecord {
        id: FlagId::from_uuid(id),
        project_id: ProjectId::from_uuid(project_id),
        key,
        value_type,
        enabled,
        created_at,
        updated_at,
        deleted_at,
        version,
    })
}

/// Parse a [`Variant`] from its DB columns.
fn assemble_variant(
    id: uuid::Uuid,
    key: String,
    value: serde_json::Value,
) -> Result<Variant, RepositoryError> {
    let value: VariantValue = serde_json::from_value(value).map_err(|e| {
        RepositoryError::Unexpected(anyhow::anyhow!("cannot deserialise variant value: {e}"))
    })?;
    Ok(Variant {
        id: VariantId::from_uuid(id),
        key,
        value,
    })
}

// ---------------------------------------------------------------------------
// PgFlagRepository
// ---------------------------------------------------------------------------

/// Postgres-backed implementation of [`FlagRepository`].
pub struct PgFlagRepository {
    pool: PgPool,
    audit: Arc<dyn AuditLogger>,
}

impl PgFlagRepository {
    /// Construct a new repository bound to `pool` and `audit`.
    pub fn new(pool: PgPool, audit: Arc<dyn AuditLogger>) -> Self {
        Self { pool, audit }
    }
}

#[async_trait]
impl FlagRepository for PgFlagRepository {
    async fn find_by_id(&self, id: FlagId) -> Result<FlagRecord, RepositoryError> {
        let row = sqlx::query!(
            r#"
            SELECT id, project_id, key, value_type, enabled,
                   created_at, updated_at, deleted_at, version
            FROM feature_flags
            WHERE id = $1 AND deleted_at IS NULL
            "#,
            id.as_uuid()
        )
        .fetch_one(&self.pool)
        .await
        .map_err(|e| match e {
            sqlx::Error::RowNotFound => RepositoryError::NotFound { id: id.to_string() },
            other => RepositoryError::Database(other),
        })?;

        assemble_flag(
            row.id, row.project_id, row.key, &row.value_type,
            row.enabled, row.created_at, row.updated_at, row.deleted_at, row.version,
        )
    }

    async fn find_by_key(
        &self,
        key: &FlagKey,
        project_id: ProjectId,
    ) -> Result<FlagRecord, RepositoryError> {
        let row = sqlx::query!(
            r#"
            SELECT id, project_id, key, value_type, enabled,
                   created_at, updated_at, deleted_at, version
            FROM feature_flags
            WHERE key = $1 AND project_id = $2 AND deleted_at IS NULL
            "#,
            key.as_str(),
            project_id.as_uuid()
        )
        .fetch_one(&self.pool)
        .await
        .map_err(|e| match e {
            sqlx::Error::RowNotFound => RepositoryError::NotFound {
                id: format!("{key}@{project_id}"),
            },
            other => RepositoryError::Database(other),
        })?;

        assemble_flag(
            row.id, row.project_id, row.key, &row.value_type,
            row.enabled, row.created_at, row.updated_at, row.deleted_at, row.version,
        )
    }

    async fn list_by_project(
        &self,
        project_id: ProjectId,
    ) -> Result<Vec<FlagRecord>, RepositoryError> {
        let rows = sqlx::query!(
            r#"
            SELECT id, project_id, key, value_type, enabled,
                   created_at, updated_at, deleted_at, version
            FROM feature_flags
            WHERE project_id = $1 AND deleted_at IS NULL
            ORDER BY created_at
            "#,
            project_id.as_uuid()
        )
        .fetch_all(&self.pool)
        .await
        .map_err(RepositoryError::Database)?;

        rows.into_iter()
            .map(|row| {
                assemble_flag(
                    row.id, row.project_id, row.key, &row.value_type,
                    row.enabled, row.created_at, row.updated_at, row.deleted_at, row.version,
                )
            })
            .collect()
    }

    async fn create(&self, flag: &FlagRecord) -> Result<(), RepositoryError> {
        let value_type = flag_value_type_to_str(flag.value_type);
        sqlx::query!(
            r#"
            INSERT INTO feature_flags
                (id, project_id, key, value_type, enabled,
                 created_at, updated_at, deleted_at, version)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
            "#,
            flag.id.as_uuid(),
            flag.project_id.as_uuid(),
            flag.key.as_str(),
            value_type,
            flag.enabled,
            flag.created_at,
            flag.updated_at,
            flag.deleted_at,
            flag.version,
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
                "flag",
                flag.id.as_uuid(),
                "create",
                serde_json::json!({
                    "key": flag.key.as_str(),
                    "project_id": flag.project_id.to_string(),
                    "value_type": value_type,
                }),
            )
            .await?;

        Ok(())
    }

    async fn update(&self, flag: &FlagRecord) -> Result<FlagRecord, RepositoryError> {
        let new_version = flag.version + 1;
        let value_type = flag_value_type_to_str(flag.value_type);
        let result = sqlx::query!(
            r#"
            UPDATE feature_flags
            SET key = $1, value_type = $2, enabled = $3,
                updated_at = NOW(), version = $4
            WHERE id = $5 AND version = $6 AND deleted_at IS NULL
            RETURNING id, project_id, key, value_type, enabled,
                      created_at, updated_at, deleted_at, version
            "#,
            flag.key.as_str(),
            value_type,
            flag.enabled,
            new_version,
            flag.id.as_uuid(),
            flag.version,
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(RepositoryError::Database)?;

        if let Some(row) = result {
            let updated = assemble_flag(
                row.id, row.project_id, row.key, &row.value_type,
                row.enabled, row.created_at, row.updated_at, row.deleted_at, row.version,
            )?;
            self.audit
                .log(
                    None,
                    "flag",
                    flag.id.as_uuid(),
                    "update",
                    serde_json::json!({ "key": flag.key.as_str(), "enabled": flag.enabled }),
                )
                .await?;
            return Ok(updated);
        }

        let current = sqlx::query!(
            "SELECT version, deleted_at FROM feature_flags WHERE id = $1",
            flag.id.as_uuid()
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(RepositoryError::Database)?;

        match current {
            None => Err(RepositoryError::NotFound {
                id: flag.id.to_string(),
            }),
            Some(row) if row.deleted_at.is_some() => Err(RepositoryError::NotFound {
                id: flag.id.to_string(),
            }),
            Some(row) => Err(RepositoryError::VersionConflict {
                expected: flag.version,
                actual: row.version,
            }),
        }
    }

    async fn soft_delete(&self, id: FlagId) -> Result<(), RepositoryError> {
        let result = sqlx::query!(
            r#"
            UPDATE feature_flags
            SET deleted_at = NOW(), updated_at = NOW(), version = version + 1
            WHERE id = $1 AND deleted_at IS NULL
            "#,
            id.as_uuid()
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
                "flag",
                id.as_uuid(),
                "soft_delete",
                serde_json::json!({}),
            )
            .await?;

        Ok(())
    }
}

// ---------------------------------------------------------------------------
// PgVariantRepository
// ---------------------------------------------------------------------------

/// Postgres-backed implementation of [`VariantRepository`].
pub struct PgVariantRepository {
    pool: PgPool,
    audit: Arc<dyn AuditLogger>,
}

impl PgVariantRepository {
    /// Construct a new repository bound to `pool` and `audit`.
    pub fn new(pool: PgPool, audit: Arc<dyn AuditLogger>) -> Self {
        Self { pool, audit }
    }
}

#[async_trait]
impl VariantRepository for PgVariantRepository {
    async fn find_by_flag(&self, flag_id: FlagId) -> Result<Vec<Variant>, RepositoryError> {
        let rows = sqlx::query!(
            r#"
            SELECT id, key, value
            FROM variants
            WHERE flag_id = $1
            ORDER BY key
            "#,
            flag_id.as_uuid()
        )
        .fetch_all(&self.pool)
        .await
        .map_err(RepositoryError::Database)?;

        rows.into_iter()
            .map(|row| assemble_variant(row.id, row.key, row.value))
            .collect()
    }

    async fn create(&self, flag_id: FlagId, variant: &Variant) -> Result<(), RepositoryError> {
        let value = serde_json::to_value(&variant.value).map_err(|e| {
            RepositoryError::Unexpected(anyhow::anyhow!("cannot serialise variant value: {e}"))
        })?;

        sqlx::query!(
            r#"
            INSERT INTO variants (id, flag_id, key, value)
            VALUES ($1, $2, $3, $4)
            "#,
            variant.id.as_uuid(),
            flag_id.as_uuid(),
            variant.key,
            value,
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
                "variant",
                variant.id.as_uuid(),
                "create",
                serde_json::json!({
                    "key": variant.key,
                    "flag_id": flag_id.to_string(),
                }),
            )
            .await?;

        Ok(())
    }

    async fn update(&self, variant: &Variant) -> Result<Variant, RepositoryError> {
        let value = serde_json::to_value(&variant.value).map_err(|e| {
            RepositoryError::Unexpected(anyhow::anyhow!("cannot serialise variant value: {e}"))
        })?;

        let result = sqlx::query!(
            r#"
            UPDATE variants
            SET key = $1, value = $2
            WHERE id = $3
            RETURNING id, key, value
            "#,
            variant.key,
            value,
            variant.id.as_uuid()
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(RepositoryError::Database)?
        .ok_or_else(|| RepositoryError::NotFound {
            id: variant.id.to_string(),
        })?;

        let updated = assemble_variant(result.id, result.key, result.value)?;

        self.audit
            .log(
                None,
                "variant",
                variant.id.as_uuid(),
                "update",
                serde_json::json!({ "key": variant.key }),
            )
            .await?;

        Ok(updated)
    }

    async fn delete(&self, id: VariantId) -> Result<(), RepositoryError> {
        let result = sqlx::query!(
            "DELETE FROM variants WHERE id = $1",
            id.as_uuid()
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
                "variant",
                id.as_uuid(),
                "delete",
                serde_json::json!({}),
            )
            .await?;

        Ok(())
    }
}
