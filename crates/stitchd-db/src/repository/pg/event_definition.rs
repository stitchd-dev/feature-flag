//! Postgres implementation for the `event_definition` repository.
//!
//! Uses raw `sqlx::query` strings (not the `query!` / `query_as!` macros)
//! so adding new columns (Phase: `events_metrics_20260519`'s admin-CRUD
//! gap fix) doesn't require running `cargo sqlx prepare` and shipping a
//! new `.sqlx/` cache for every parallel worker.

use std::sync::Arc;

use async_trait::async_trait;
use sqlx::{PgPool, Row as _};

use stitchd_core::{
    event::{EventDefinition, EventValueType, MetricType},
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
    #[must_use]
    pub fn new(pool: PgPool, audit: Arc<dyn AuditLogger>) -> Self {
        Self { pool, audit }
    }
}

// ── Row → domain mapping ─────────────────────────────────────────────────────

fn row_to_event_definition(
    row: &sqlx::postgres::PgRow,
) -> Result<EventDefinition, RepositoryError> {
    let id: uuid::Uuid = row.try_get("id").map_err(RepositoryError::Database)?;
    let env_id: uuid::Uuid = row
        .try_get("environment_id")
        .map_err(RepositoryError::Database)?;
    let value_type_str: String = row
        .try_get("value_type")
        .map_err(RepositoryError::Database)?;
    let value_type = match value_type_str.as_str() {
        "bool" => EventValueType::Bool,
        "int" => EventValueType::Int,
        "double" => EventValueType::Double,
        other => {
            return Err(RepositoryError::Database(sqlx::Error::Decode(
                format!("unknown value_type: {other}").into(),
            )));
        }
    };
    let metric_type_str: String = row
        .try_get("metric_type")
        .map_err(RepositoryError::Database)?;
    let metric_type = match metric_type_str.as_str() {
        "count" => MetricType::Count,
        "conversion" => MetricType::Conversion,
        "revenue" => MetricType::Revenue,
        "duration" => MetricType::Duration,
        "numeric" => MetricType::Numeric,
        "custom" => MetricType::Custom,
        other => {
            return Err(RepositoryError::Database(sqlx::Error::Decode(
                format!("unknown metric_type: {other}").into(),
            )));
        }
    };
    let key: String = row.try_get("key").map_err(RepositoryError::Database)?;
    let name: Option<String> = row.try_get("name").map_err(RepositoryError::Database)?;
    Ok(EventDefinition {
        id: EventDefinitionId::from_uuid(id),
        environment_id: EnvironmentId::from_uuid(env_id),
        // Backfill: if `name` is null (legacy row), surface `key` so the
        // admin UI doesn't render empty cells.
        name: name.unwrap_or_else(|| key.clone()),
        description: row
            .try_get("description")
            .map_err(RepositoryError::Database)?,
        key,
        value_type,
        metric_type,
        schema: row.try_get("schema").map_err(RepositoryError::Database)?,
        created_at: row
            .try_get("created_at")
            .map_err(RepositoryError::Database)?,
        updated_at: row
            .try_get("updated_at")
            .map_err(RepositoryError::Database)?,
        deleted_at: row
            .try_get("deleted_at")
            .map_err(RepositoryError::Database)?,
        version: row.try_get("version").map_err(RepositoryError::Database)?,
    })
}

const fn value_type_str(v: EventValueType) -> &'static str {
    match v {
        EventValueType::Bool => "bool",
        EventValueType::Int => "int",
        EventValueType::Double => "double",
    }
}

const fn metric_type_str(m: MetricType) -> &'static str {
    match m {
        MetricType::Count => "count",
        MetricType::Conversion => "conversion",
        MetricType::Revenue => "revenue",
        MetricType::Duration => "duration",
        MetricType::Numeric => "numeric",
        MetricType::Custom => "custom",
    }
}

const SELECT_COLS: &str = "id, environment_id, key, name, description, value_type, metric_type, \
     schema, created_at, updated_at, deleted_at, version";

// ── EventDefinitionRepository impl ───────────────────────────────────────────

#[async_trait]
impl EventDefinitionRepository for PgEventDefinitionRepository {
    async fn find_by_id(&self, id: EventDefinitionId) -> Result<EventDefinition, RepositoryError> {
        let sql = format!(
            "SELECT {SELECT_COLS} FROM event_definitions \
             WHERE id = $1 AND deleted_at IS NULL"
        );
        let row = sqlx::query(&sql)
            .bind(id.as_uuid())
            .fetch_optional(&self.pool)
            .await
            .map_err(RepositoryError::Database)?;
        row.map_or_else(
            || Err(RepositoryError::NotFound { id: id.to_string() }),
            |r| row_to_event_definition(&r),
        )
    }

    async fn find_by_key(
        &self,
        key: &str,
        environment_id: EnvironmentId,
    ) -> Result<EventDefinition, RepositoryError> {
        let sql = format!(
            "SELECT {SELECT_COLS} FROM event_definitions \
             WHERE key = $1 AND environment_id = $2 AND deleted_at IS NULL"
        );
        let row = sqlx::query(&sql)
            .bind(key)
            .bind(environment_id.as_uuid())
            .fetch_optional(&self.pool)
            .await
            .map_err(RepositoryError::Database)?;
        row.map_or_else(
            || {
                Err(RepositoryError::NotFound {
                    id: format!("{key}@{environment_id}"),
                })
            },
            |r| row_to_event_definition(&r),
        )
    }

    async fn list_by_environment(
        &self,
        environment_id: EnvironmentId,
    ) -> Result<Vec<EventDefinition>, RepositoryError> {
        let sql = format!(
            "SELECT {SELECT_COLS} FROM event_definitions \
             WHERE environment_id = $1 AND deleted_at IS NULL \
             ORDER BY created_at"
        );
        let rows = sqlx::query(&sql)
            .bind(environment_id.as_uuid())
            .fetch_all(&self.pool)
            .await
            .map_err(RepositoryError::Database)?;
        let mut out = Vec::with_capacity(rows.len());
        for row in rows {
            out.push(row_to_event_definition(&row)?);
        }
        Ok(out)
    }

    async fn list_by_environment_paginated(
        &self,
        environment_id: EnvironmentId,
        offset: u64,
        limit: u64,
        include_archived: bool,
    ) -> Result<(Vec<EventDefinition>, u64), RepositoryError> {
        let where_clause = if include_archived {
            "WHERE environment_id = $1"
        } else {
            "WHERE environment_id = $1 AND deleted_at IS NULL"
        };
        let sql = format!(
            "SELECT {SELECT_COLS}, COUNT(*) OVER() AS total_count \
             FROM event_definitions {where_clause} \
             ORDER BY created_at \
             OFFSET $2 LIMIT $3"
        );
        let rows = sqlx::query(&sql)
            .bind(environment_id.as_uuid())
            .bind(i64::try_from(offset).unwrap_or(i64::MAX))
            .bind(i64::try_from(limit).unwrap_or(i64::MAX))
            .fetch_all(&self.pool)
            .await
            .map_err(RepositoryError::Database)?;

        if rows.is_empty() {
            return Ok((Vec::new(), 0));
        }
        let total: i64 = rows[0]
            .try_get("total_count")
            .map_err(RepositoryError::Database)?;
        let mut out = Vec::with_capacity(rows.len());
        for row in rows {
            out.push(row_to_event_definition(&row)?);
        }
        Ok((out, u64::try_from(total).unwrap_or(0)))
    }

    async fn create(&self, def: &EventDefinition) -> Result<(), RepositoryError> {
        sqlx::query(
            "INSERT INTO event_definitions \
             (id, environment_id, key, name, description, value_type, metric_type, \
              schema, created_at, updated_at, deleted_at, version) \
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12)",
        )
        .bind(def.id.as_uuid())
        .bind(def.environment_id.as_uuid())
        .bind(&def.key)
        .bind(&def.name)
        .bind(def.description.as_deref())
        .bind(value_type_str(def.value_type))
        .bind(metric_type_str(def.metric_type))
        .bind(def.schema.as_ref())
        .bind(def.created_at)
        .bind(def.updated_at)
        .bind(def.deleted_at)
        .bind(def.version)
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
                "event_definition",
                def.id.as_uuid(),
                "create",
                serde_json::json!({
                    "key": def.key,
                    "name": def.name,
                    "environment_id": def.environment_id.to_string(),
                    "value_type": value_type_str(def.value_type),
                    "metric_type": metric_type_str(def.metric_type),
                }),
            )
            .await?;

        Ok(())
    }

    async fn update(&self, def: &EventDefinition) -> Result<EventDefinition, RepositoryError> {
        let new_version = def.version + 1;
        let sql = format!(
            "UPDATE event_definitions \
             SET name = $1, description = $2, value_type = $3, metric_type = $4, \
                 schema = $5, updated_at = NOW(), version = $6 \
             WHERE id = $7 AND version = $8 AND deleted_at IS NULL \
             RETURNING {SELECT_COLS}"
        );
        let row = sqlx::query(&sql)
            .bind(&def.name)
            .bind(def.description.as_deref())
            .bind(value_type_str(def.value_type))
            .bind(metric_type_str(def.metric_type))
            .bind(def.schema.as_ref())
            .bind(new_version)
            .bind(def.id.as_uuid())
            .bind(def.version)
            .fetch_optional(&self.pool)
            .await
            .map_err(RepositoryError::Database)?;

        if let Some(r) = row {
            let updated = row_to_event_definition(&r)?;
            self.audit
                .log(
                    None,
                    "event_definition",
                    def.id.as_uuid(),
                    "update",
                    serde_json::json!({
                        "key": def.key,
                        "name": def.name,
                        "value_type": value_type_str(def.value_type),
                        "metric_type": metric_type_str(def.metric_type),
                        "version": new_version,
                    }),
                )
                .await?;
            return Ok(updated);
        }

        // Distinguish NotFound vs VersionConflict
        let current: Option<i64> = sqlx::query_scalar(
            "SELECT version FROM event_definitions \
             WHERE id = $1 AND deleted_at IS NULL",
        )
        .bind(def.id.as_uuid())
        .fetch_optional(&self.pool)
        .await
        .map_err(RepositoryError::Database)?;

        current.map_or_else(
            || {
                Err(RepositoryError::NotFound {
                    id: def.id.to_string(),
                })
            },
            |actual| {
                Err(RepositoryError::VersionConflict {
                    expected: def.version,
                    actual,
                })
            },
        )
    }

    async fn soft_delete(&self, id: EventDefinitionId) -> Result<(), RepositoryError> {
        let rows = sqlx::query(
            "UPDATE event_definitions \
             SET deleted_at = NOW(), updated_at = NOW() \
             WHERE id = $1 AND deleted_at IS NULL",
        )
        .bind(id.as_uuid())
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
