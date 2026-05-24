//! Postgres implementations for the `flag` and `variant` repositories.

use std::sync::Arc;

use async_trait::async_trait;
use sqlx::{PgPool, Row};

use stitchd_core::{
    flag::{FlagHashingConfig, FlagRecord, FlagRule, FlagValueType, Variant, VariantValue},
    id::{EnvironmentId, FlagId, FlagKey, ProjectId, VariantId},
    rollout::RolloutDistribution,
};

use crate::{
    RepositoryError,
    repository::{AuditLogger, FlagRepository, VariantRepository},
};

// ---------------------------------------------------------------------------
// DB error mapping helper
// ---------------------------------------------------------------------------

/// Map a sqlx database error to a typed [`RepositoryError`], distinguishing
/// unique violations (23505) from foreign-key violations (23503).
fn map_db_err(e: sqlx::Error) -> RepositoryError {
    if let sqlx::Error::Database(ref dbe) = e
        && let Some(constraint) = dbe.constraint()
    {
        let code = dbe
            .code()
            .map(std::borrow::Cow::into_owned)
            .unwrap_or_default();
        return match code.as_str() {
            "23505" => RepositoryError::UniqueViolation {
                field: constraint.to_string(),
            },
            "23503" => RepositoryError::ForeignKeyViolation {
                constraint: constraint.to_string(),
            },
            _ => RepositoryError::Database(e),
        };
    }
    RepositoryError::Database(e)
}

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
    name: String,
    description: String,
    value_type: &str,
    enabled: bool,
    default_variant_id: Option<uuid::Uuid>,
    default_rule_distribution: Option<serde_json::Value>,
    created_at: chrono::DateTime<chrono::Utc>,
    updated_at: chrono::DateTime<chrono::Utc>,
    deleted_at: Option<chrono::DateTime<chrono::Utc>>,
    version: i64,
) -> Result<FlagRecord, RepositoryError> {
    let value_type = parse_flag_value_type(value_type)?;
    let key = FlagKey::new(key).map_err(|e| {
        RepositoryError::Unexpected(anyhow::anyhow!("invalid flag key stored in DB: {e}"))
    })?;
    let default_rule_distribution: Option<RolloutDistribution> = match default_rule_distribution {
        None => None,
        Some(v) => Some(serde_json::from_value(v).map_err(|e| {
            RepositoryError::Unexpected(anyhow::anyhow!(
                "default_rule_distribution JSONB malformed: {e}"
            ))
        })?),
    };
    Ok(FlagRecord {
        id: FlagId::from_uuid(id),
        project_id: ProjectId::from_uuid(project_id),
        key,
        name,
        description,
        value_type,
        enabled,
        default_variant_id: default_variant_id.map(VariantId::from_uuid),
        default_rule_distribution,
        created_at,
        updated_at,
        deleted_at,
        version,
    })
}

/// Assemble a [`FlagRecord`] from a sqlx `PgRow`. Convenience wrapper around
/// [`assemble_flag`] that keeps caller bodies small (helps `clippy::too_many_lines`
/// across the `impl FlagRepository for PgFlagRepository` block, which `async_trait`
/// expands into a single macro-generated function).
fn assemble_flag_from_row(row: &sqlx::postgres::PgRow) -> Result<FlagRecord, RepositoryError> {
    assemble_flag(
        row.get("id"),
        row.get("project_id"),
        row.get("key"),
        row.get("name"),
        row.get("description"),
        row.get::<String, _>("value_type").as_str(),
        row.get("enabled"),
        row.get("default_variant_id"),
        row.get::<Option<serde_json::Value>, _>("default_rule_distribution"),
        row.get("created_at"),
        row.get("updated_at"),
        row.get("deleted_at"),
        row.get("version"),
    )
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
            SELECT id, project_id, key, name, description, value_type, enabled,
                   default_variant_id, default_rule_distribution,
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

        assemble_flag_from_row(&row)
    }

    async fn find_by_key(
        &self,
        key: &FlagKey,
        project_id: ProjectId,
    ) -> Result<FlagRecord, RepositoryError> {
        let row = sqlx::query(
            r"
            SELECT id, project_id, key, name, description, value_type, enabled,
                   default_variant_id, default_rule_distribution,
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

        assemble_flag_from_row(&row)
    }

    async fn list_by_project(
        &self,
        project_id: ProjectId,
    ) -> Result<Vec<FlagRecord>, RepositoryError> {
        let rows = sqlx::query(
            r"
            SELECT id, project_id, key, name, description, value_type, enabled,
                   default_variant_id, default_rule_distribution,
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
            .map(|row| assemble_flag_from_row(&row))
            .collect()
    }

    async fn list_by_project_paginated(
        &self,
        project_id: ProjectId,
        offset: u64,
        limit: u64,
    ) -> Result<(Vec<FlagRecord>, u64), RepositoryError> {
        // COUNT(*) OVER() gives the total row count alongside each result row,
        // so we avoid a separate COUNT query.
        #[allow(clippy::cast_possible_wrap)]
        let rows = sqlx::query(
            r"
            SELECT id, project_id, key, name, description, value_type, enabled,
                   default_variant_id, default_rule_distribution,
                   created_at, updated_at, deleted_at, version,
                   COUNT(*) OVER() AS total_count
            FROM feature_flags
            WHERE project_id = $1 AND deleted_at IS NULL
            ORDER BY created_at
            LIMIT $2 OFFSET $3
            ",
        )
        .bind(project_id.as_uuid())
        .bind(limit as i64)
        .bind(offset as i64)
        .fetch_all(&self.pool)
        .await
        .map_err(RepositoryError::Database)?;

        let total = rows.first().map_or(0, |r| {
            let n: i64 = r.get("total_count");
            #[allow(clippy::cast_sign_loss)]
            let result = n.max(0) as u64;
            result
        });

        let flags = rows
            .into_iter()
            .map(|row| assemble_flag_from_row(&row))
            .collect::<Result<Vec<_>, _>>()?;

        Ok((flags, total))
    }

    async fn list_by_project_all(
        &self,
        project_id: ProjectId,
    ) -> Result<Vec<FlagRecord>, RepositoryError> {
        let rows = sqlx::query(
            r"
            SELECT id, project_id, key, name, description, value_type, enabled,
                   default_variant_id, default_rule_distribution,
                   created_at, updated_at, deleted_at, version
            FROM feature_flags
            WHERE project_id = $1
            ORDER BY created_at
            ",
        )
        .bind(project_id.as_uuid())
        .fetch_all(&self.pool)
        .await
        .map_err(RepositoryError::Database)?;

        rows.into_iter()
            .map(|row| assemble_flag_from_row(&row))
            .collect()
    }

    async fn list_by_environment(
        &self,
        environment_id: EnvironmentId,
    ) -> Result<Vec<FlagRecord>, RepositoryError> {
        let rows = sqlx::query(
            r"
            SELECT ff.id, ff.project_id, ff.key, ff.name, ff.description, ff.value_type,
                   ff.enabled, ff.default_variant_id, ff.default_rule_distribution,
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
            .map(|row| assemble_flag_from_row(&row))
            .collect()
    }

    async fn list_by_environment_all(
        &self,
        environment_id: EnvironmentId,
    ) -> Result<Vec<FlagRecord>, RepositoryError> {
        let rows = sqlx::query(
            r"
            SELECT ff.id, ff.project_id, ff.key, ff.name, ff.description, ff.value_type,
                   ff.enabled, ff.default_variant_id, ff.default_rule_distribution,
                   ff.created_at, ff.updated_at, ff.deleted_at, ff.version
            FROM feature_flags ff
            JOIN environments env ON ff.project_id = env.project_id
            WHERE env.id = $1 AND env.deleted_at IS NULL
            ORDER BY ff.created_at
            ",
        )
        .bind(environment_id.as_uuid())
        .fetch_all(&self.pool)
        .await
        .map_err(RepositoryError::Database)?;

        rows.into_iter()
            .map(|row| assemble_flag_from_row(&row))
            .collect()
    }

    async fn create(&self, flag: &FlagRecord) -> Result<(), RepositoryError> {
        let value_type = flag_value_type_to_str(flag.value_type);
        sqlx::query(
            r"
            INSERT INTO feature_flags
                (id, project_id, key, name, description, value_type, enabled, default_variant_id,
                 default_rule_distribution, created_at, updated_at, deleted_at, version)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)
            ",
        )
        .bind(flag.id.as_uuid())
        .bind(flag.project_id.as_uuid())
        .bind(flag.key.as_str())
        .bind(&flag.name)
        .bind(&flag.description)
        .bind(value_type)
        .bind(flag.enabled)
        .bind(flag.default_variant_id.map(|id| id.as_uuid()))
        .bind(
            flag.default_rule_distribution
                .as_ref()
                .map(|d| serde_json::to_value(d).expect("RolloutDistribution -> JSON")),
        )
        .bind(flag.created_at)
        .bind(flag.updated_at)
        .bind(flag.deleted_at)
        .bind(flag.version)
        .execute(&self.pool)
        .await
        .map_err(map_db_err)?;

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

    // SQL + result-mapping + version-conflict probe + audit-log call exceed the
    // 80-line clippy threshold; the function is still a single linear step
    // sequence and splitting it hurts readability more than it helps.
    #[allow(clippy::too_many_lines)]
    async fn update(&self, flag: &FlagRecord) -> Result<FlagRecord, RepositoryError> {
        let new_version = flag.version + 1;
        let value_type = flag_value_type_to_str(flag.value_type);
        let result = sqlx::query(
            r"
            UPDATE feature_flags
            SET key = $1, name = $2, description = $3, value_type = $4, enabled = $5,
                default_variant_id = $6, default_rule_distribution = $7,
                updated_at = NOW(), version = $8
            WHERE id = $9 AND version = $10 AND deleted_at IS NULL
            RETURNING id, project_id, key, name, description, value_type, enabled,
                      default_variant_id, default_rule_distribution,
                      created_at, updated_at, deleted_at, version
            ",
        )
        .bind(flag.key.as_str())
        .bind(&flag.name)
        .bind(&flag.description)
        .bind(value_type)
        .bind(flag.enabled)
        .bind(flag.default_variant_id.map(|id| id.as_uuid()))
        .bind(
            flag.default_rule_distribution
                .as_ref()
                .map(|d| serde_json::to_value(d).expect("RolloutDistribution -> JSON")),
        )
        .bind(new_version)
        .bind(flag.id.as_uuid())
        .bind(flag.version)
        .fetch_optional(&self.pool)
        .await
        .map_err(RepositoryError::Database)?;

        if let Some(row) = result {
            let updated = assemble_flag_from_row(&row)?;
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

    async fn find_by_key_any(
        &self,
        key: &FlagKey,
        project_id: ProjectId,
    ) -> Result<FlagRecord, RepositoryError> {
        let row = sqlx::query(
            r"
            SELECT id, project_id, key, name, description, value_type, enabled,
                   default_variant_id, default_rule_distribution,
                   created_at, updated_at, deleted_at, version
            FROM feature_flags
            WHERE key = $1 AND project_id = $2
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

        assemble_flag_from_row(&row)
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

    async fn soft_restore(&self, id: FlagId) -> Result<(), RepositoryError> {
        let result = sqlx::query(
            r"
            UPDATE feature_flags
            SET deleted_at = NULL, updated_at = NOW(), version = version + 1
            WHERE id = $1 AND deleted_at IS NOT NULL
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
                "soft_restore",
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
            SELECT id, rule_index, rule_def
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
            let mut rule: stitchd_core::rule_engine::types::Rule = serde_json::from_value(rule_def)
                .map_err(|e| {
                    RepositoryError::Unexpected(anyhow::anyhow!("failed to deserialize rule: {e}"))
                })?;
            // Authoritative rule UUID is the `feature_flag_rules.id` column —
            // overwrite any stale value carried in the serialised `rule_def`
            // JSON so callers (admin UI, experiment bindings) always see the
            // real row PK. Experiments FK on this UUID, so we must surface it
            // exactly as stored.
            let db_rule_id: uuid::Uuid = row.get("id");
            rule.id = stitchd_core::id::RuleId::from_uuid(db_rule_id);
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
        .map_err(map_db_err)?;

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

    async fn replace_all_for_flag(
        &self,
        flag_id: FlagId,
        variants: &[Variant],
    ) -> Result<(), RepositoryError> {
        let mut tx = self.pool.begin().await.map_err(RepositoryError::Database)?;

        sqlx::query("DELETE FROM variants WHERE flag_id = $1")
            .bind(flag_id.as_uuid())
            .execute(&mut *tx)
            .await
            .map_err(RepositoryError::Database)?;

        for v in variants {
            let value = serde_json::to_value(&v.value).map_err(|e| {
                RepositoryError::Unexpected(anyhow::anyhow!("cannot serialise variant value: {e}"))
            })?;
            sqlx::query("INSERT INTO variants (id, flag_id, key, value) VALUES ($1, $2, $3, $4)")
                .bind(v.id.as_uuid())
                .bind(flag_id.as_uuid())
                .bind(&v.key)
                .bind(value)
                .execute(&mut *tx)
                .await
                .map_err(map_db_err)?;
        }

        tx.commit().await.map_err(RepositoryError::Database)?;

        self.audit
            .log(
                None,
                "variant",
                flag_id.as_uuid(),
                "replace_all",
                serde_json::json!({ "count": variants.len() }),
            )
            .await?;

        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Unit tests for pure helpers
// ---------------------------------------------------------------------------

#[cfg(test)]
mod unit_tests {
    use super::*;
    use stitchd_core::flag::VariantValue;

    #[test]
    fn parse_flag_value_type_all_variants() {
        assert!(matches!(
            parse_flag_value_type("bool").unwrap(),
            FlagValueType::Bool
        ));
        assert!(matches!(
            parse_flag_value_type("int").unwrap(),
            FlagValueType::Int
        ));
        assert!(matches!(
            parse_flag_value_type("double").unwrap(),
            FlagValueType::Double
        ));
        assert!(matches!(
            parse_flag_value_type("str").unwrap(),
            FlagValueType::Str
        ));
        assert!(matches!(
            parse_flag_value_type("json").unwrap(),
            FlagValueType::Json
        ));
    }

    #[test]
    fn parse_flag_value_type_unknown_returns_error() {
        let err = parse_flag_value_type("unknown").unwrap_err();
        assert!(err.to_string().contains("unknown flag value_type: unknown"));
    }

    #[test]
    fn flag_value_type_to_str_all_variants() {
        assert_eq!(flag_value_type_to_str(FlagValueType::Bool), "bool");
        assert_eq!(flag_value_type_to_str(FlagValueType::Int), "int");
        assert_eq!(flag_value_type_to_str(FlagValueType::Double), "double");
        assert_eq!(flag_value_type_to_str(FlagValueType::Str), "str");
        assert_eq!(flag_value_type_to_str(FlagValueType::Json), "json");
    }

    #[test]
    fn flag_value_type_roundtrips() {
        for vt in [
            FlagValueType::Bool,
            FlagValueType::Int,
            FlagValueType::Double,
            FlagValueType::Str,
            FlagValueType::Json,
        ] {
            let s = flag_value_type_to_str(vt);
            let parsed = parse_flag_value_type(s).unwrap();
            assert_eq!(parsed, vt, "roundtrip failed for {s}");
        }
    }

    #[test]
    fn assemble_variant_bool_value() {
        let id = uuid::Uuid::new_v4();
        // VariantValue uses #[serde(untagged)]: BoolValue(true) serialises as plain `true`
        let value = serde_json::json!(true);
        let variant = assemble_variant(id, "on".to_string(), value).unwrap();
        assert_eq!(variant.key, "on");
        assert!(matches!(variant.value, VariantValue::BoolValue(true)));
    }

    #[test]
    fn assemble_variant_str_value() {
        let id = uuid::Uuid::new_v4();
        let value = serde_json::json!("hello");
        let variant = assemble_variant(id, "v1".to_string(), value).unwrap();
        assert!(matches!(variant.value, VariantValue::StrValue(ref s) if s == "hello"));
    }

    #[test]
    fn assemble_flag_valid() {
        let id = uuid::Uuid::new_v4();
        let project_id = uuid::Uuid::new_v4();
        let now = chrono::Utc::now();
        let flag = assemble_flag(
            id,
            project_id,
            "my-flag".to_string(),
            "My Flag".to_string(),
            "A test flag".to_string(),
            "bool",
            true,
            None,
            None,
            now,
            now,
            None,
            1,
        )
        .unwrap();
        assert_eq!(flag.key.as_str(), "my-flag");
        assert_eq!(flag.name, "My Flag");
        assert!(flag.enabled);
    }

    #[test]
    fn assemble_flag_invalid_value_type_returns_error() {
        let id = uuid::Uuid::new_v4();
        let project_id = uuid::Uuid::new_v4();
        let now = chrono::Utc::now();
        let result = assemble_flag(
            id,
            project_id,
            "my-flag".to_string(),
            String::new(),
            String::new(),
            "badtype",
            true,
            None,
            None,
            now,
            now,
            None,
            1,
        );
        assert!(result.is_err());
    }

    #[test]
    fn assemble_flag_invalid_key_returns_error() {
        let id = uuid::Uuid::new_v4();
        let project_id = uuid::Uuid::new_v4();
        let now = chrono::Utc::now();
        // FlagKey validation rejects empty string
        let result = assemble_flag(
            id,
            project_id,
            String::new(), // empty key should fail
            String::new(),
            String::new(),
            "bool",
            true,
            None,
            None,
            now,
            now,
            None,
            1,
        );
        assert!(result.is_err());
    }
}
