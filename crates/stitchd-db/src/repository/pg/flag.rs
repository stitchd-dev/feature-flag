//! Postgres implementations for the `flag` and `variant` repositories.

use std::sync::Arc;

use async_trait::async_trait;
use sqlx::{PgPool, Row};

use stitchd_core::{
    flag::{FlagHashingConfig, FlagRecord, FlagRule, FlagValueType, Variant, VariantValue},
    id::{EnvironmentId, FlagId, FlagKey, ProjectId, VariantId},
};

use crate::{
    RepositoryError,
    repository::{AuditLogger, FlagRepository, VariantRepository},
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
#[allow(clippy::too_many_arguments)]
fn assemble_flag(
    id: uuid::Uuid,
    project_id: uuid::Uuid,
    key: String,
    value_type: &str,
    enabled: bool,
    default_variant_id: Option<uuid::Uuid>,
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
        default_variant_id: default_variant_id.map(VariantId::from_uuid),
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
        let row = sqlx::query(
            r"
            SELECT id, project_id, key, value_type, enabled, default_variant_id,
                   created_at, updated_at, deleted_at, version
            FROM feature_flags
            WHERE id = $1 AND deleted_at IS NULL
            ",
        )
        .bind(id.as_uuid())
        .fetch_one(&self.pool)
        .await
        .map_err(|e| match e {
            sqlx::Error::RowNotFound => RepositoryError::NotFound { id: id.to_string() },
            other => RepositoryError::Database(other),
        })?;

        assemble_flag(
            row.get("id"),
            row.get("project_id"),
            row.get("key"),
            row.get::<String, _>("value_type").as_str(),
            row.get("enabled"),
            row.get("default_variant_id"),
            row.get("created_at"),
            row.get("updated_at"),
            row.get("deleted_at"),
            row.get("version"),
        )
    }

    async fn find_by_key(
        &self,
        key: &FlagKey,
        project_id: ProjectId,
    ) -> Result<FlagRecord, RepositoryError> {
        let row = sqlx::query(
            r"
            SELECT id, project_id, key, value_type, enabled, default_variant_id,
                   created_at, updated_at, deleted_at, version
            FROM feature_flags
            WHERE key = $1 AND project_id = $2 AND deleted_at IS NULL
            ",
        )
        .bind(key.as_str())
        .bind(project_id.as_uuid())
        .fetch_one(&self.pool)
        .await
        .map_err(|e| match e {
            sqlx::Error::RowNotFound => RepositoryError::NotFound {
                id: format!("{key}@{project_id}"),
            },
            other => RepositoryError::Database(other),
        })?;

        assemble_flag(
            row.get("id"),
            row.get("project_id"),
            row.get("key"),
            row.get::<String, _>("value_type").as_str(),
            row.get("enabled"),
            row.get("default_variant_id"),
            row.get("created_at"),
            row.get("updated_at"),
            row.get("deleted_at"),
            row.get("version"),
        )
    }

    async fn list_by_project(
        &self,
        project_id: ProjectId,
    ) -> Result<Vec<FlagRecord>, RepositoryError> {
        let rows = sqlx::query(
            r"
            SELECT id, project_id, key, value_type, enabled, default_variant_id,
                   created_at, updated_at, deleted_at, version
            FROM feature_flags
            WHERE project_id = $1 AND deleted_at IS NULL
            ORDER BY created_at
            ",
        )
        .bind(project_id.as_uuid())
        .fetch_all(&self.pool)
        .await
        .map_err(RepositoryError::Database)?;

        rows.into_iter()
            .map(|row| {
                assemble_flag(
                    row.get("id"),
                    row.get("project_id"),
                    row.get("key"),
                    row.get::<String, _>("value_type").as_str(),
                    row.get("enabled"),
                    row.get("default_variant_id"),
                    row.get("created_at"),
                    row.get("updated_at"),
                    row.get("deleted_at"),
                    row.get("version"),
                )
            })
            .collect()
    }

    async fn list_by_environment(
        &self,
        environment_id: EnvironmentId,
    ) -> Result<Vec<FlagRecord>, RepositoryError> {
        let rows = sqlx::query(
            r"
            SELECT ff.id, ff.project_id, ff.key, ff.value_type, ff.enabled, ff.default_variant_id,
                   ff.created_at, ff.updated_at, ff.deleted_at, ff.version
            FROM feature_flags ff
            JOIN environments env ON ff.project_id = env.project_id
            WHERE env.id = $1 AND ff.deleted_at IS NULL AND env.deleted_at IS NULL
            ORDER BY ff.created_at
            ",
        )
        .bind(environment_id.as_uuid())
        .fetch_all(&self.pool)
        .await
        .map_err(RepositoryError::Database)?;

        rows.into_iter()
            .map(|row| {
                assemble_flag(
                    row.get("id"),
                    row.get("project_id"),
                    row.get("key"),
                    row.get::<String, _>("value_type").as_str(),
                    row.get("enabled"),
                    row.get("default_variant_id"),
                    row.get("created_at"),
                    row.get("updated_at"),
                    row.get("deleted_at"),
                    row.get("version"),
                )
            })
            .collect()
    }

    async fn create(&self, flag: &FlagRecord) -> Result<(), RepositoryError> {
        let value_type = flag_value_type_to_str(flag.value_type);
        sqlx::query(
            r"
            INSERT INTO feature_flags
                (id, project_id, key, value_type, enabled, default_variant_id,
                 created_at, updated_at, deleted_at, version)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
            ",
        )
        .bind(flag.id.as_uuid())
        .bind(flag.project_id.as_uuid())
        .bind(flag.key.as_str())
        .bind(value_type)
        .bind(flag.enabled)
        .bind(flag.default_variant_id.map(|id| id.as_uuid()))
        .bind(flag.created_at)
        .bind(flag.updated_at)
        .bind(flag.deleted_at)
        .bind(flag.version)
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
        let result = sqlx::query(
            r"
            UPDATE feature_flags
            SET key = $1, value_type = $2, enabled = $3, default_variant_id = $4,
                updated_at = NOW(), version = $5
            WHERE id = $6 AND version = $7 AND deleted_at IS NULL
            RETURNING id, project_id, key, value_type, enabled, default_variant_id,
                      created_at, updated_at, deleted_at, version
            ",
        )
        .bind(flag.key.as_str())
        .bind(value_type)
        .bind(flag.enabled)
        .bind(flag.default_variant_id.map(|id| id.as_uuid()))
        .bind(new_version)
        .bind(flag.id.as_uuid())
        .bind(flag.version)
        .fetch_optional(&self.pool)
        .await
        .map_err(RepositoryError::Database)?;

        if let Some(row) = result {
            let updated = assemble_flag(
                row.get("id"),
                row.get("project_id"),
                row.get("key"),
                row.get::<String, _>("value_type").as_str(),
                row.get("enabled"),
                row.get("default_variant_id"),
                row.get("created_at"),
                row.get("updated_at"),
                row.get("deleted_at"),
                row.get("version"),
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

        let current = sqlx::query("SELECT version, deleted_at FROM feature_flags WHERE id = $1")
            .bind(flag.id.as_uuid())
            .fetch_optional(&self.pool)
            .await
            .map_err(RepositoryError::Database)?;

        match current {
            None => Err(RepositoryError::NotFound {
                id: flag.id.to_string(),
            }),
            Some(row)
                if row
                    .get::<Option<chrono::DateTime<chrono::Utc>>, _>("deleted_at")
                    .is_some() =>
            {
                Err(RepositoryError::NotFound {
                    id: flag.id.to_string(),
                })
            }
            Some(row) => Err(RepositoryError::VersionConflict {
                expected: flag.version,
                actual: row.get("version"),
            }),
        }
    }

    async fn soft_delete(&self, id: FlagId) -> Result<(), RepositoryError> {
        let result = sqlx::query(
            r"
            UPDATE feature_flags
            SET deleted_at = NOW(), updated_at = NOW(), version = version + 1
            WHERE id = $1 AND deleted_at IS NULL
            ",
        )
        .bind(id.as_uuid())
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

    async fn find_hashing_config(
        &self,
        flag_id: FlagId,
    ) -> Result<Vec<FlagHashingConfig>, RepositoryError> {
        let rows = sqlx::query(
            r#"
            SELECT parameter_key, parameter_type, "order"
            FROM flag_hashing_config
            WHERE flag_id = $1
            ORDER BY "order" ASC
            "#,
        )
        .bind(flag_id.as_uuid())
        .fetch_all(&self.pool)
        .await
        .map_err(RepositoryError::Database)?;

        Ok(rows
            .into_iter()
            .map(|row| FlagHashingConfig {
                flag_id,
                parameter_key: row.get("parameter_key"),
                parameter_type: row.get("parameter_type"),
                order: row.get("order"),
            })
            .collect())
    }

    async fn upsert_hashing_config(
        &self,
        flag_id: FlagId,
        config: &[FlagHashingConfig],
    ) -> Result<(), RepositoryError> {
        let mut tx = self.pool.begin().await.map_err(RepositoryError::Database)?;

        sqlx::query("DELETE FROM flag_hashing_config WHERE flag_id = $1")
            .bind(flag_id.as_uuid())
            .execute(&mut *tx)
            .await
            .map_err(RepositoryError::Database)?;

        for item in config {
            sqlx::query(
                r#"
                INSERT INTO flag_hashing_config (flag_id, parameter_key, parameter_type, "order")
                VALUES ($1, $2, $3, $4)
                "#,
            )
            .bind(flag_id.as_uuid())
            .bind(&item.parameter_key)
            .bind(&item.parameter_type)
            .bind(item.order)
            .execute(&mut *tx)
            .await
            .map_err(RepositoryError::Database)?;
        }

        tx.commit().await.map_err(RepositoryError::Database)?;

        self.audit
            .log(
                None,
                "flag",
                flag_id.as_uuid(),
                "update_hashing_config",
                serde_json::json!({ "count": config.len() }),
            )
            .await?;

        Ok(())
    }

    async fn find_rules(&self, flag_id: FlagId) -> Result<Vec<FlagRule>, RepositoryError> {
        let rows = sqlx::query(
            r"
            SELECT rule_index, rule_def
            FROM feature_flag_rules
            WHERE flag_id = $1
            ORDER BY rule_index ASC
            ",
        )
        .bind(flag_id.as_uuid())
        .fetch_all(&self.pool)
        .await
        .map_err(RepositoryError::Database)?;

        let mut rules = Vec::with_capacity(rows.len());
        for row in rows {
            let rule_def: serde_json::Value = row.get("rule_def");
            let rule: stitchd_core::rule_engine::types::Rule = serde_json::from_value(rule_def)
                .map_err(|e| {
                    RepositoryError::Unexpected(anyhow::anyhow!("failed to deserialize rule: {e}"))
                })?;
            rules.push(FlagRule {
                flag_id,
                rule_index: row.get("rule_index"),
                rule,
            });
        }

        Ok(rules)
    }

    async fn upsert_rules(
        &self,
        flag_id: FlagId,
        rules: &[FlagRule],
    ) -> Result<(), RepositoryError> {
        let mut tx = self.pool.begin().await.map_err(RepositoryError::Database)?;

        sqlx::query("DELETE FROM feature_flag_rules WHERE flag_id = $1")
            .bind(flag_id.as_uuid())
            .execute(&mut *tx)
            .await
            .map_err(RepositoryError::Database)?;

        for item in rules {
            let rule_def = serde_json::to_value(&item.rule).map_err(|e| {
                RepositoryError::Unexpected(anyhow::anyhow!("failed to serialize rule: {e}"))
            })?;
            sqlx::query(
                r"
                INSERT INTO feature_flag_rules (flag_id, rule_index, rule_def)
                VALUES ($1, $2, $3)
                ",
            )
            .bind(flag_id.as_uuid())
            .bind(item.rule_index)
            .bind(rule_def)
            .execute(&mut *tx)
            .await
            .map_err(RepositoryError::Database)?;
        }

        tx.commit().await.map_err(RepositoryError::Database)?;

        self.audit
            .log(
                None,
                "flag",
                flag_id.as_uuid(),
                "update_rules",
                serde_json::json!({ "count": rules.len() }),
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
            RepositoryError::Unexpected(anyhow::anyhow!("cannot deserialise variant value: {e}"))
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
            RepositoryError::Unexpected(anyhow::anyhow!("cannot deserialise variant value: {e}"))
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
        let result = sqlx::query!("DELETE FROM variants WHERE id = $1", id.as_uuid())
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
