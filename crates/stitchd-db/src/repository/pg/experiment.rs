//! Postgres implementation for the `experiment` repository.

use std::sync::Arc;

use async_trait::async_trait;
use sqlx::PgPool;

use stitchd_core::{
    experimentation::{Experiment, ExperimentIteration, ExperimentStatus, validate_transition},
    id::{EnvironmentId, ExperimentId, ExperimentIterationId, MetricId, RuleId, UserId},
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
                metric_ids          AS "metric_ids: Vec<MetricId>",
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
                metric_ids          AS "metric_ids: Vec<MetricId>",
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

    async fn list_by_environment_paginated(
        &self,
        env_id: EnvironmentId,
        offset: u64,
        limit: u64,
    ) -> Result<(Vec<Experiment>, u64), RepositoryError> {
        use sqlx::Row as _;

        let rows = sqlx::query(
            r"
            SELECT
                id, env_id, flag_rule_id, name, description, hypothesis, status,
                metric_ids, traffic_allocation::float8 AS traffic_allocation, min_sample_size,
                scheduled_start_at, scheduled_end_at, version, created_at, updated_at, deleted_at,
                COUNT(*) OVER() AS total_count
            FROM experiments
            WHERE env_id = $1 AND deleted_at IS NULL
            ORDER BY created_at
            LIMIT $2 OFFSET $3
            ",
        )
        .bind(env_id.as_uuid())
        .bind({
            #[allow(clippy::cast_possible_wrap)]
            let v = limit as i64;
            v
        })
        .bind({
            #[allow(clippy::cast_possible_wrap)]
            let v = offset as i64;
            v
        })
        .fetch_all(&self.pool)
        .await
        .map_err(RepositoryError::Database)?;

        let total = rows.first().map_or(0, |r| {
            let n: i64 = r.get("total_count");
            #[allow(clippy::cast_sign_loss)]
            let result = n.max(0) as u64;
            result
        });

        let experiments = rows
            .iter()
            .map(|r| {
                use stitchd_core::id::RuleId;
                let status_str: &str = r.get("status");
                let status = match status_str {
                    "running" => ExperimentStatus::Running,
                    "paused" => ExperimentStatus::Paused,
                    "stopped" => ExperimentStatus::Stopped,
                    _ => ExperimentStatus::Draft,
                };
                let metric_uuids: Vec<uuid::Uuid> = r.get("metric_ids");
                let metric_ids: Vec<MetricId> =
                    metric_uuids.into_iter().map(MetricId::from_uuid).collect();
                Ok(Experiment {
                    id: ExperimentId::from_uuid(r.get("id")),
                    environment_id: EnvironmentId::from_uuid(r.get("env_id")),
                    flag_rule_id: RuleId::from_uuid(r.get("flag_rule_id")),
                    name: r.get("name"),
                    description: r.get("description"),
                    hypothesis: r.get("hypothesis"),
                    status,
                    metric_ids,
                    traffic_allocation: {
                        let v: f64 = r.get("traffic_allocation");
                        v
                    },
                    min_sample_size: r.get("min_sample_size"),
                    scheduled_start_at: r.get("scheduled_start_at"),
                    scheduled_end_at: r.get("scheduled_end_at"),
                    version: r.get("version"),
                    created_at: r.get("created_at"),
                    updated_at: r.get("updated_at"),
                    deleted_at: r.get("deleted_at"),
                })
            })
            .collect::<Result<Vec<_>, RepositoryError>>()?;

        Ok((experiments, total))
    }

    async fn create(&self, experiment: &Experiment) -> Result<(), RepositoryError> {
        // sqlx can't infer `Vec<MetricId>` for a `UUID[]` bind through the
        // query!() macro, so we project to `Vec<Uuid>` at the bind site.
        let metric_uuids: Vec<uuid::Uuid> = experiment
            .metric_ids
            .iter()
            .map(MetricId::as_uuid)
            .collect();
        sqlx::query!(
            r#"
            INSERT INTO experiments
                (id, env_id, flag_rule_id, name, description, hypothesis, status,
                 metric_ids, traffic_allocation, min_sample_size,
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
            &metric_uuids,
            experiment.traffic_allocation,
            experiment.min_sample_size,
            experiment.scheduled_start_at,
            experiment.scheduled_end_at,
            experiment.version,
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

    #[allow(clippy::too_many_lines)]
    async fn update(&self, experiment: &Experiment) -> Result<Experiment, RepositoryError> {
        // Mutation guard: reject updates on active (running or paused) experiments.
        let current_status: Option<ExperimentStatus> = sqlx::query_scalar!(
            r#"SELECT status AS "status: ExperimentStatus" FROM experiments WHERE id = $1 AND deleted_at IS NULL"#,
            experiment.id as ExperimentId,
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(RepositoryError::Database)?;

        match current_status {
            None => {
                return Err(RepositoryError::NotFound {
                    id: experiment.id.to_string(),
                });
            }
            Some(ExperimentStatus::Running | ExperimentStatus::Paused) => {
                return Err(RepositoryError::InvalidState {
                    reason: "cannot update an experiment that is running or paused".to_string(),
                });
            }
            _ => {}
        }

        let new_version = experiment.version + 1;
        let metric_uuids: Vec<uuid::Uuid> = experiment
            .metric_ids
            .iter()
            .map(MetricId::as_uuid)
            .collect();
        let result = sqlx::query_as!(
            Experiment,
            r#"
            UPDATE experiments
            SET name = $1,
                description = $2,
                hypothesis = $3,
                status = $4::text,
                metric_ids = $5,
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
                metric_ids          AS "metric_ids: Vec<MetricId>",
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
            &metric_uuids,
            experiment.traffic_allocation as f64,
            experiment.min_sample_size,
            experiment.scheduled_start_at,
            experiment.scheduled_end_at,
            new_version,
            experiment.id as ExperimentId,
            experiment.version,
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
                        actual: current_version,
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
                metric_ids          AS "metric_ids: Vec<MetricId>",
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

    #[allow(clippy::too_many_lines)]
    async fn apply_transition(
        &self,
        id: ExperimentId,
        to: ExperimentStatus,
        actor_id: Option<UserId>,
    ) -> Result<Experiment, RepositoryError> {
        // 1. Fetch current experiment to determine `from` status and current data.
        let current = self.find_by_id(id).await?;
        let from = current.status;

        // 2. Validate transition.
        validate_transition(from, to).map_err(|e| RepositoryError::InvalidState {
            reason: e.to_string(),
        })?;

        // 3. Execute everything in a single transaction.
        let mut tx = self.pool.begin().await.map_err(RepositoryError::Database)?;

        // 3a. Uniqueness check: if transitioning to running or paused, ensure no
        //     other non-deleted experiment on the same flag_rule_id is already
        //     running or paused (excluding self).
        if to == ExperimentStatus::Running || to == ExperimentStatus::Paused {
            let conflict_count: i64 = sqlx::query_scalar!(
                r#"
                SELECT COUNT(*) AS "count!"
                FROM experiments
                WHERE flag_rule_id = $1
                  AND id <> $2
                  AND status IN ('running', 'paused')
                  AND deleted_at IS NULL
                "#,
                current.flag_rule_id as RuleId,
                id as ExperimentId,
            )
            .fetch_one(&mut *tx)
            .await
            .map_err(RepositoryError::Database)?;

            if conflict_count > 0 {
                return Err(RepositoryError::UniqueViolation {
                    field: "flag_rule_id".to_string(),
                });
            }
        }

        // 3b. Update experiment status and bump version (optimistic concurrency).
        let new_version = current.version + 1;
        let updated = sqlx::query_as!(
            Experiment,
            r#"
            UPDATE experiments
            SET status    = $1::text,
                version   = $2,
                updated_at = NOW()
            WHERE id = $3 AND version = $4 AND deleted_at IS NULL
            RETURNING
                id                  AS "id: ExperimentId",
                env_id              AS "environment_id: EnvironmentId",
                flag_rule_id        AS "flag_rule_id: RuleId",
                name,
                description,
                hypothesis,
                status              AS "status: ExperimentStatus",
                metric_ids          AS "metric_ids: Vec<MetricId>",
                traffic_allocation  AS "traffic_allocation: f64",
                min_sample_size     AS "min_sample_size: i64",
                scheduled_start_at,
                scheduled_end_at,
                version             AS "version: i64",
                created_at,
                updated_at,
                deleted_at
            "#,
            to as ExperimentStatus,
            new_version,
            id as ExperimentId,
            current.version,
        )
        .fetch_optional(&mut *tx)
        .await
        .map_err(RepositoryError::Database)?
        .ok_or_else(|| RepositoryError::VersionConflict {
            expected: current.version,
            actual: new_version, // could be stale, but we can't re-query inside the tx here
        })?;

        // 3c. If transitioning into Running: end the previous active iteration (if any),
        //     then insert a new iteration.
        if to == ExperimentStatus::Running {
            // End any currently active iteration (handles paused→running restart).
            sqlx::query!(
                r#"
                UPDATE experiment_iterations
                SET ended_at = NOW()
                WHERE experiment_id = $1 AND ended_at IS NULL
                "#,
                id as ExperimentId,
            )
            .execute(&mut *tx)
            .await
            .map_err(RepositoryError::Database)?;

            // Calculate next iteration_number.
            let max_iter: Option<i32> = sqlx::query_scalar!(
                r#"
                SELECT MAX(iteration_number)
                FROM experiment_iterations
                WHERE experiment_id = $1
                "#,
                id as ExperimentId,
            )
            .fetch_one(&mut *tx)
            .await
            .map_err(RepositoryError::Database)?;

            let next_iteration = max_iter.unwrap_or(0) + 1;
            let iteration_id = ExperimentIterationId::new();
            let metric_uuids: Vec<uuid::Uuid> =
                current.metric_ids.iter().map(MetricId::as_uuid).collect();

            sqlx::query!(
                r#"
                INSERT INTO experiment_iterations
                    (id, experiment_id, iteration_number, started_at, metric_ids,
                     traffic_allocation, min_sample_size)
                VALUES ($1, $2, $3, NOW(), $4, $5::float8::numeric, $6)
                "#,
                iteration_id as ExperimentIterationId,
                id as ExperimentId,
                next_iteration,
                &metric_uuids,
                current.traffic_allocation as f64,
                current.min_sample_size,
            )
            .execute(&mut *tx)
            .await
            .map_err(RepositoryError::Database)?;
        }

        // 3d. If transitioning into Running: freeze the flag rule.
        if to == ExperimentStatus::Running {
            sqlx::query!(
                "UPDATE feature_flag_rules SET frozen = TRUE WHERE id = $1",
                current.flag_rule_id as RuleId,
            )
            .execute(&mut *tx)
            .await
            .map_err(RepositoryError::Database)?;
        }

        // 3e. If transitioning into Paused or Stopped: unfreeze the flag rule.
        if to == ExperimentStatus::Paused || to == ExperimentStatus::Stopped {
            sqlx::query!(
                "UPDATE feature_flag_rules SET frozen = FALSE WHERE id = $1",
                current.flag_rule_id as RuleId,
            )
            .execute(&mut *tx)
            .await
            .map_err(RepositoryError::Database)?;
        }

        // 3f. If from == Running and transitioning to Paused or Stopped: end active iteration.
        if from == ExperimentStatus::Running
            && (to == ExperimentStatus::Paused || to == ExperimentStatus::Stopped)
        {
            sqlx::query!(
                r#"
                UPDATE experiment_iterations
                SET ended_at = NOW()
                WHERE experiment_id = $1 AND ended_at IS NULL
                "#,
                id as ExperimentId,
            )
            .execute(&mut *tx)
            .await
            .map_err(RepositoryError::Database)?;
        }

        tx.commit().await.map_err(RepositoryError::Database)?;

        // 4. Audit log the transition.
        self.audit
            .log(
                actor_id,
                "experiment",
                id.as_uuid(),
                "transition",
                serde_json::json!({
                    "from": format!("{from:?}"),
                    "to": format!("{to:?}"),
                    "version": new_version,
                }),
            )
            .await?;

        Ok(updated)
    }

    async fn list_all_running(&self) -> Result<Vec<Experiment>, RepositoryError> {
        use sqlx::Row as _;
        let rows = sqlx::query(
            r"
            SELECT
                id, env_id, flag_rule_id, name, description, hypothesis, status,
                metric_ids, traffic_allocation::float8 AS traffic_allocation, min_sample_size,
                scheduled_start_at, scheduled_end_at, version, created_at, updated_at, deleted_at
            FROM experiments
            WHERE status = 'running' AND deleted_at IS NULL
            ORDER BY created_at
            ",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(RepositoryError::Database)?;

        rows.iter()
            .map(|r| {
                let status_str: &str = r.get("status");
                let status = match status_str {
                    "running" => ExperimentStatus::Running,
                    "paused" => ExperimentStatus::Paused,
                    "stopped" => ExperimentStatus::Stopped,
                    _ => ExperimentStatus::Draft,
                };
                let metric_uuids: Vec<uuid::Uuid> = r.get("metric_ids");
                let metric_ids: Vec<MetricId> =
                    metric_uuids.into_iter().map(MetricId::from_uuid).collect();
                Ok(Experiment {
                    id: ExperimentId::from_uuid(r.get("id")),
                    environment_id: EnvironmentId::from_uuid(r.get("env_id")),
                    flag_rule_id: RuleId::from_uuid(r.get("flag_rule_id")),
                    name: r.get("name"),
                    description: r.get("description"),
                    hypothesis: r.get("hypothesis"),
                    status,
                    metric_ids,
                    traffic_allocation: {
                        let v: f64 = r.get("traffic_allocation");
                        v
                    },
                    min_sample_size: r.get("min_sample_size"),
                    scheduled_start_at: r.get("scheduled_start_at"),
                    scheduled_end_at: r.get("scheduled_end_at"),
                    version: r.get("version"),
                    created_at: r.get("created_at"),
                    updated_at: r.get("updated_at"),
                    deleted_at: r.get("deleted_at"),
                })
            })
            .collect::<Result<Vec<_>, RepositoryError>>()
    }

    async fn find_iteration_by_id(
        &self,
        iteration_id: ExperimentIterationId,
    ) -> Result<ExperimentIteration, RepositoryError> {
        use sqlx::Row as _;
        let row = sqlx::query(
            r"
            SELECT
                id, experiment_id, iteration_number, started_at, ended_at,
                metric_ids, traffic_allocation::float8 AS traffic_allocation, min_sample_size
            FROM experiment_iterations
            WHERE id = $1
            ",
        )
        .bind(iteration_id.as_uuid())
        .fetch_optional(&self.pool)
        .await
        .map_err(RepositoryError::Database)?;

        let row = row.ok_or_else(|| RepositoryError::NotFound {
            id: iteration_id.to_string(),
        })?;

        let metric_uuids: Vec<uuid::Uuid> = row.get("metric_ids");
        let metric_ids: Vec<MetricId> = metric_uuids.into_iter().map(MetricId::from_uuid).collect();
        Ok(ExperimentIteration {
            id: ExperimentIterationId::from_uuid(row.get("id")),
            experiment_id: ExperimentId::from_uuid(row.get("experiment_id")),
            iteration_number: row.get("iteration_number"),
            started_at: row.get("started_at"),
            ended_at: row.get("ended_at"),
            metric_ids,
            traffic_allocation: {
                let v: f64 = row.get("traffic_allocation");
                v
            },
            min_sample_size: row.get("min_sample_size"),
        })
    }
}
