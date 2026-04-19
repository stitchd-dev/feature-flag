//! Postgres implementation for the `experiment` repository.

use std::sync::Arc;

use async_trait::async_trait;
use sqlx::PgPool;

use stitchd_core::{
    experimentation::{Experiment, ExperimentIteration, ExperimentStatus},
    id::{EnvironmentId, ExperimentId, ExperimentIterationId, RuleId},
};

use crate::{
    RepositoryError,
    repository::{AuditLogger, ExperimentRepository},
};

/// Postgres-backed implementation of [`ExperimentRepository`].
pub struct PgExperimentRepository {
    pool: PgPool,
    audit: Arc<dyn AuditLogger>,
}

impl PgExperimentRepository {
    /// Construct a new repository bound to `pool` and `audit`.
    pub fn new(pool: PgPool, audit: Arc<dyn AuditLogger>) -> Self {
        Self { pool, audit }
    }
}

#[async_trait]
impl ExperimentRepository for PgExperimentRepository {
    async fn find_by_id(&self, id: ExperimentId) -> Result<Experiment, RepositoryError> {
        sqlx::query_as!(
            Experiment,
            r#"
            SELECT
                id                  AS "id: ExperimentId",
                env_id              AS "environment_id: EnvironmentId",
                flag_rule_id        AS "flag_rule_id: RuleId",
                name,
                description,
                hypothesis,
                status              AS "status: ExperimentStatus",
                metric_keys,
                traffic_allocation  AS "traffic_allocation: f64",
                min_sample_size     AS "min_sample_size: i64",
                scheduled_start_at,
                scheduled_end_at,
                version             AS "version: i64",
                created_at,
                updated_at,
                deleted_at
            FROM experiments
            WHERE id = $1 AND deleted_at IS NULL
            "#,
            id as ExperimentId,
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
        env_id: EnvironmentId,
        status_filter: Option<ExperimentStatus>,
    ) -> Result<Vec<Experiment>, RepositoryError> {
        // We use a raw query with an optional status filter.
        // Since sqlx::query_as! doesn't support dynamic WHERE clauses,
        // we fetch all and filter in Rust when a filter is given.
        let rows = sqlx::query_as!(
            Experiment,
            r#"
            SELECT
                id                  AS "id: ExperimentId",
                env_id              AS "environment_id: EnvironmentId",
                flag_rule_id        AS "flag_rule_id: RuleId",
                name,
                description,
                hypothesis,
                status              AS "status: ExperimentStatus",
                metric_keys,
                traffic_allocation  AS "traffic_allocation: f64",
                min_sample_size     AS "min_sample_size: i64",
                scheduled_start_at,
                scheduled_end_at,
                version             AS "version: i64",
                created_at,
                updated_at,
                deleted_at
            FROM experiments
            WHERE env_id = $1 AND deleted_at IS NULL
            ORDER BY created_at
            "#,
            env_id as EnvironmentId,
        )
        .fetch_all(&self.pool)
        .await
        .map_err(RepositoryError::Database)?;

        if let Some(status) = status_filter {
            Ok(rows.into_iter().filter(|e| e.status == status).collect())
        } else {
            Ok(rows)
        }
    }

    async fn create(&self, experiment: &Experiment) -> Result<(), RepositoryError> {
        sqlx::query!(
            r#"
            INSERT INTO experiments
                (id, env_id, flag_rule_id, name, description, hypothesis, status,
                 metric_keys, traffic_allocation, min_sample_size,
                 scheduled_start_at, scheduled_end_at, version, created_at, updated_at, deleted_at)
            VALUES ($1, $2, $3, $4, $5, $6, $7::text, $8, $9::float8::numeric, $10, $11, $12, $13, $14, $15, $16)
            "#,
            experiment.id as ExperimentId,
            experiment.environment_id as EnvironmentId,
            experiment.flag_rule_id as RuleId,
            experiment.name,
            experiment.description,
            experiment.hypothesis,
            experiment.status as ExperimentStatus,
            &experiment.metric_keys,
            experiment.traffic_allocation,
            experiment.min_sample_size.map(|v| v as i32),
            experiment.scheduled_start_at,
            experiment.scheduled_end_at,
            experiment.version as i32,
            experiment.created_at,
            experiment.updated_at,
            experiment.deleted_at,
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
                "experiment",
                experiment.id.as_uuid(),
                "create",
                serde_json::json!({
                    "name": experiment.name,
                    "environment_id": experiment.environment_id.to_string(),
                    "flag_rule_id": experiment.flag_rule_id.to_string(),
                    "status": format!("{:?}", experiment.status),
                }),
            )
            .await?;

        Ok(())
    }

    async fn update(&self, experiment: &Experiment) -> Result<Experiment, RepositoryError> {
        let new_version = experiment.version + 1;
        let result = sqlx::query_as!(
            Experiment,
            r#"
            UPDATE experiments
            SET name = $1,
                description = $2,
                hypothesis = $3,
                status = $4::text,
                metric_keys = $5,
                traffic_allocation = $6::float8::numeric,
                min_sample_size = $7,
                scheduled_start_at = $8,
                scheduled_end_at = $9,
                updated_at = NOW(),
                version = $10
            WHERE id = $11 AND version = $12 AND deleted_at IS NULL
            RETURNING
                id                  AS "id: ExperimentId",
                env_id              AS "environment_id: EnvironmentId",
                flag_rule_id        AS "flag_rule_id: RuleId",
                name,
                description,
                hypothesis,
                status              AS "status: ExperimentStatus",
                metric_keys,
                traffic_allocation  AS "traffic_allocation: f64",
                min_sample_size     AS "min_sample_size: i64",
                scheduled_start_at,
                scheduled_end_at,
                version             AS "version: i64",
                created_at,
                updated_at,
                deleted_at
            "#,
            experiment.name,
            experiment.description,
            experiment.hypothesis,
            experiment.status as ExperimentStatus,
            &experiment.metric_keys,
            experiment.traffic_allocation as f64,
            experiment.min_sample_size.map(|v| v as i32),
            experiment.scheduled_start_at,
            experiment.scheduled_end_at,
            new_version as i32,
            experiment.id as ExperimentId,
            experiment.version as i32,
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(RepositoryError::Database)?;

        if let Some(updated) = result {
            self.audit
                .log(
                    None,
                    "experiment",
                    experiment.id.as_uuid(),
                    "update",
                    serde_json::json!({
                        "name": experiment.name,
                        "status": format!("{:?}", experiment.status),
                        "version": new_version,
                    }),
                )
                .await?;
            Ok(updated)
        } else {
            // Distinguish NotFound vs VersionConflict.
            let current = sqlx::query_scalar!(
                "SELECT version FROM experiments WHERE id = $1 AND deleted_at IS NULL",
                experiment.id as ExperimentId
            )
            .fetch_optional(&self.pool)
            .await
            .map_err(RepositoryError::Database)?;

            current.map_or_else(
                || {
                    Err(RepositoryError::NotFound {
                        id: experiment.id.to_string(),
                    })
                },
                |current_version| {
                    Err(RepositoryError::VersionConflict {
                        expected: experiment.version,
                        actual: i64::from(current_version),
                    })
                },
            )
        }
    }

    async fn soft_delete(&self, id: ExperimentId) -> Result<(), RepositoryError> {
        // Fetch the current status to reject running experiments.
        let row = sqlx::query!(
            r#"
            SELECT status AS "status: ExperimentStatus"
            FROM experiments
            WHERE id = $1 AND deleted_at IS NULL
            "#,
            id as ExperimentId,
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(RepositoryError::Database)?;

        let Some(row) = row else {
            return Err(RepositoryError::NotFound { id: id.to_string() });
        };

        if row.status == ExperimentStatus::Running {
            return Err(RepositoryError::InvalidState {
                reason: "cannot delete a running experiment; pause or stop it first".to_string(),
            });
        }

        sqlx::query!(
            r#"
            UPDATE experiments
            SET deleted_at = NOW(), updated_at = NOW()
            WHERE id = $1 AND deleted_at IS NULL
            "#,
            id as ExperimentId,
        )
        .execute(&self.pool)
        .await
        .map_err(RepositoryError::Database)?;

        self.audit
            .log(
                None,
                "experiment",
                id.as_uuid(),
                "soft_delete",
                serde_json::json!({}),
            )
            .await?;

        Ok(())
    }

    async fn list_iterations(
        &self,
        experiment_id: ExperimentId,
    ) -> Result<Vec<ExperimentIteration>, RepositoryError> {
        sqlx::query_as!(
            ExperimentIteration,
            r#"
            SELECT
                id                  AS "id: ExperimentIterationId",
                experiment_id       AS "experiment_id: ExperimentId",
                iteration_number,
                started_at,
                ended_at,
                metric_keys,
                traffic_allocation  AS "traffic_allocation: f64",
                min_sample_size     AS "min_sample_size: i64"
            FROM experiment_iterations
            WHERE experiment_id = $1
            ORDER BY iteration_number
            "#,
            experiment_id as ExperimentId,
        )
        .fetch_all(&self.pool)
        .await
        .map_err(RepositoryError::Database)
    }
}
