//! Postgres implementation for the `metric` repository.
//!
//! Uses raw `sqlx::query` strings (not the `query!` / `query_as!` macros) —
//! the `metric_definitions` table is new in this track and the `.sqlx/`
//! offline cache would need regeneration for every parallel worker
//! otherwise. See `conductor/patterns.md` "`sqlx::query_as` for new tables".

use std::sync::Arc;

use async_trait::async_trait;
use sqlx::{PgPool, Row as _};

use stitchd_core::{
    id::{EnvironmentId, MetricId},
    metric::{GoalDirection, MetricDefinition, MetricKind},
};

use crate::{
    RepositoryError,
    repository::{AuditLogger, MetricRepository},
};

/// Postgres-backed implementation of [`MetricRepository`].
pub struct PgMetricRepository {
    pool: PgPool,
    audit: Arc<dyn AuditLogger>,
}

impl PgMetricRepository {
    /// Construct a new repository bound to `pool` and `audit`.
    #[must_use]
    pub fn new(pool: PgPool, audit: Arc<dyn AuditLogger>) -> Self {
        Self { pool, audit }
    }
}

// ── Row → domain mapping ─────────────────────────────────────────────────────

fn row_to_metric(row: &sqlx::postgres::PgRow) -> Result<MetricDefinition, RepositoryError> {
    let kind_str: String = row.try_get("kind").map_err(RepositoryError::Database)?;
    let config_json: serde_json::Value =
        row.try_get("config").map_err(RepositoryError::Database)?;

    // Reconstruct MetricKind from kind-string + raw config JSON. The
    // wire format on the API uses serde tag = "kind" with the config
    // flattened — at this layer we have them split across columns, so
    // we materialise the same shape and deserialise.
    let mut tagged = serde_json::Map::with_capacity(2);
    tagged.insert("kind".into(), serde_json::Value::String(kind_str.clone()));
    if let serde_json::Value::Object(map) = config_json {
        for (k, v) in map {
            tagged.insert(k, v);
        }
    } else {
        return Err(RepositoryError::Database(sqlx::Error::Decode(
            format!("metric.config is not a JSON object: {config_json}").into(),
        )));
    }
    let kind: MetricKind =
        serde_json::from_value(serde_json::Value::Object(tagged)).map_err(|e| {
            RepositoryError::Database(sqlx::Error::Decode(
                format!("failed to decode MetricKind (kind={kind_str}): {e}").into(),
            ))
        })?;

    let goal_str: String = row
        .try_get("goal_direction")
        .map_err(RepositoryError::Database)?;
    let goal_direction: GoalDirection = match goal_str.as_str() {
        "increase" => GoalDirection::Increase,
        "decrease" => GoalDirection::Decrease,
        "neutral" => GoalDirection::Neutral,
        other => {
            return Err(RepositoryError::Database(sqlx::Error::Decode(
                format!("unknown goal_direction: {other}").into(),
            )));
        }
    };

    Ok(MetricDefinition {
        id: MetricId::from_uuid(row.try_get("id").map_err(RepositoryError::Database)?),
        environment_id: EnvironmentId::from_uuid(
            row.try_get("environment_id")
                .map_err(RepositoryError::Database)?,
        ),
        key: row.try_get("key").map_err(RepositoryError::Database)?,
        name: row.try_get("name").map_err(RepositoryError::Database)?,
        description: row.try_get("description").map_err(RepositoryError::Database)?,
        kind,
        goal_direction,
        version: row.try_get("version").map_err(RepositoryError::Database)?,
        created_at: row.try_get("created_at").map_err(RepositoryError::Database)?,
        updated_at: row.try_get("updated_at").map_err(RepositoryError::Database)?,
        deleted_at: row.try_get("deleted_at").map_err(RepositoryError::Database)?,
    })
}

/// Split `MetricKind` serialisation into `(kind_string, config_value)`
/// for the `(kind TEXT, config JSONB)` column split.
fn split_kind(kind: &MetricKind) -> Result<(String, serde_json::Value), RepositoryError> {
    let mut tagged = serde_json::to_value(kind).map_err(|e| {
        RepositoryError::Database(sqlx::Error::Encode(
            format!("failed to encode MetricKind: {e}").into(),
        ))
    })?;
    let kind_str = tagged
        .get("kind")
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            RepositoryError::Database(sqlx::Error::Encode(
                "MetricKind serialisation missing `kind` tag".into(),
            ))
        })?
        .to_string();
    if let serde_json::Value::Object(map) = &mut tagged {
        map.remove("kind");
    }
    Ok((kind_str, tagged))
}

const fn goal_direction_str(g: GoalDirection) -> &'static str {
    match g {
        GoalDirection::Increase => "increase",
        GoalDirection::Decrease => "decrease",
        GoalDirection::Neutral => "neutral",
    }
}

// ── MetricRepository impl ────────────────────────────────────────────────────

#[async_trait]
impl MetricRepository for PgMetricRepository {
    async fn find_by_id(&self, id: MetricId) -> Result<MetricDefinition, RepositoryError> {
        let row = sqlx::query(
            r"
            SELECT id, environment_id, key, name, description,
                   kind, config, goal_direction,
                   version, created_at, updated_at, deleted_at
            FROM metric_definitions
            WHERE id = $1 AND deleted_at IS NULL
            ",
        )
        .bind(id.as_uuid())
        .fetch_optional(&self.pool)
        .await
        .map_err(RepositoryError::Database)?;

        row.map_or_else(
            || Err(RepositoryError::NotFound { id: id.to_string() }),
            |r| row_to_metric(&r),
        )
    }

    async fn find_by_key(
        &self,
        key: &str,
        environment_id: EnvironmentId,
    ) -> Result<MetricDefinition, RepositoryError> {
        let row = sqlx::query(
            r"
            SELECT id, environment_id, key, name, description,
                   kind, config, goal_direction,
                   version, created_at, updated_at, deleted_at
            FROM metric_definitions
            WHERE key = $1 AND environment_id = $2 AND deleted_at IS NULL
            ",
        )
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
            |r| row_to_metric(&r),
        )
    }

    async fn find_batch_by_ids(
        &self,
        ids: &[MetricId],
    ) -> Result<Vec<MetricDefinition>, RepositoryError> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let uuids: Vec<uuid::Uuid> = ids.iter().map(MetricId::as_uuid).collect();
        let rows = sqlx::query(
            r"
            SELECT id, environment_id, key, name, description,
                   kind, config, goal_direction,
                   version, created_at, updated_at, deleted_at
            FROM metric_definitions
            WHERE id = ANY($1) AND deleted_at IS NULL
            ",
        )
        .bind(&uuids)
        .fetch_all(&self.pool)
        .await
        .map_err(RepositoryError::Database)?;

        let mut metrics = Vec::with_capacity(rows.len());
        for row in rows {
            metrics.push(row_to_metric(&row)?);
        }
        Ok(metrics)
    }

    async fn list_by_environment(
        &self,
        environment_id: EnvironmentId,
    ) -> Result<Vec<MetricDefinition>, RepositoryError> {
        let rows = sqlx::query(
            r"
            SELECT id, environment_id, key, name, description,
                   kind, config, goal_direction,
                   version, created_at, updated_at, deleted_at
            FROM metric_definitions
            WHERE environment_id = $1 AND deleted_at IS NULL
            ORDER BY created_at
            ",
        )
        .bind(environment_id.as_uuid())
        .fetch_all(&self.pool)
        .await
        .map_err(RepositoryError::Database)?;

        let mut metrics = Vec::with_capacity(rows.len());
        for row in rows {
            metrics.push(row_to_metric(&row)?);
        }
        Ok(metrics)
    }

    async fn list_by_environment_paginated(
        &self,
        environment_id: EnvironmentId,
        offset: u64,
        limit: u64,
    ) -> Result<(Vec<MetricDefinition>, u64), RepositoryError> {
        // Use COUNT(*) OVER() to return total alongside each row, matching
        // the pattern from `db_optim_20260516`.
        let rows = sqlx::query(
            r"
            SELECT id, environment_id, key, name, description,
                   kind, config, goal_direction,
                   version, created_at, updated_at, deleted_at,
                   COUNT(*) OVER() AS total_count
            FROM metric_definitions
            WHERE environment_id = $1 AND deleted_at IS NULL
            ORDER BY created_at
            OFFSET $2 LIMIT $3
            ",
        )
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
        let mut metrics = Vec::with_capacity(rows.len());
        for row in rows {
            metrics.push(row_to_metric(&row)?);
        }
        Ok((metrics, u64::try_from(total).unwrap_or(0)))
    }

    async fn create(&self, metric: &MetricDefinition) -> Result<(), RepositoryError> {
        let (kind_str, config) = split_kind(&metric.kind)?;

        sqlx::query(
            r"
            INSERT INTO metric_definitions
                (id, environment_id, key, name, description, kind, config,
                 goal_direction, version, created_at, updated_at, deleted_at)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)
            ",
        )
        .bind(metric.id.as_uuid())
        .bind(metric.environment_id.as_uuid())
        .bind(&metric.key)
        .bind(&metric.name)
        .bind(metric.description.as_deref())
        .bind(&kind_str)
        .bind(&config)
        .bind(goal_direction_str(metric.goal_direction))
        .bind(metric.version)
        .bind(metric.created_at)
        .bind(metric.updated_at)
        .bind(metric.deleted_at)
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
                "metric_definition",
                metric.id.as_uuid(),
                "create",
                serde_json::json!({
                    "key": metric.key,
                    "environment_id": metric.environment_id.to_string(),
                    "kind": kind_str,
                    "name": metric.name,
                }),
            )
            .await?;

        Ok(())
    }

    async fn update(
        &self,
        metric: &MetricDefinition,
    ) -> Result<MetricDefinition, RepositoryError> {
        let (kind_str, config) = split_kind(&metric.kind)?;
        let new_version = metric.version + 1;

        let row = sqlx::query(
            r"
            UPDATE metric_definitions
            SET key = $1, name = $2, description = $3,
                kind = $4, config = $5, goal_direction = $6,
                version = $7, updated_at = NOW()
            WHERE id = $8 AND version = $9 AND deleted_at IS NULL
            RETURNING id, environment_id, key, name, description,
                      kind, config, goal_direction,
                      version, created_at, updated_at, deleted_at
            ",
        )
        .bind(&metric.key)
        .bind(&metric.name)
        .bind(metric.description.as_deref())
        .bind(&kind_str)
        .bind(&config)
        .bind(goal_direction_str(metric.goal_direction))
        .bind(new_version)
        .bind(metric.id.as_uuid())
        .bind(metric.version)
        .fetch_optional(&self.pool)
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

        if let Some(r) = row {
            let updated = row_to_metric(&r)?;
            self.audit
                .log(
                    None,
                    "metric_definition",
                    metric.id.as_uuid(),
                    "update",
                    serde_json::json!({
                        "key": metric.key,
                        "kind": kind_str,
                        "version": new_version,
                    }),
                )
                .await?;
            return Ok(updated);
        }

        // Either the row doesn't exist (NotFound) or the version didn't
        // match (VersionConflict). Distinguish by looking up the current
        // version.
        let current: Option<i64> = sqlx::query_scalar(
            "SELECT version FROM metric_definitions \
             WHERE id = $1 AND deleted_at IS NULL",
        )
        .bind(metric.id.as_uuid())
        .fetch_optional(&self.pool)
        .await
        .map_err(RepositoryError::Database)?;

        current.map_or_else(
            || {
                Err(RepositoryError::NotFound {
                    id: metric.id.to_string(),
                })
            },
            |actual| {
                Err(RepositoryError::VersionConflict {
                    expected: metric.version,
                    actual,
                })
            },
        )
    }

    async fn soft_delete(&self, id: MetricId) -> Result<(), RepositoryError> {
        let rows = sqlx::query(
            r"
            UPDATE metric_definitions
            SET deleted_at = NOW(), updated_at = NOW()
            WHERE id = $1 AND deleted_at IS NULL
            ",
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
                "metric_definition",
                id.as_uuid(),
                "soft_delete",
                serde_json::json!({}),
            )
            .await?;

        Ok(())
    }
}
