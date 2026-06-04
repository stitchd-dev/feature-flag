//! Postgres implementation for the `experiment` repository.

use std::sync::Arc;

use async_trait::async_trait;
use sqlx::PgPool;

use stitchd_core::{
    experimentation::{Experiment, ExperimentIteration, ExperimentStatus, validate_transition},
    id::{
        EnvironmentId, ExclusionGroupId, ExperimentId, ExperimentIterationId, FlagId, MetricId,
        RuleId, UserId,
    },
    rollout::RolloutDistribution,
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

// ---------------------------------------------------------------------------
// Row → domain helpers
// ---------------------------------------------------------------------------

/// Convert a stored `pre_period_days` (signed `i32`) to the domain's `u32`.
/// The DB CHECK guarantees `>= 0`, so any negative value is corruption — we
/// surface it as `0` rather than panic.
#[allow(clippy::cast_sign_loss)]
const fn pre_period_to_u32(v: i32) -> u32 {
    if v < 0 { 0 } else { v as u32 }
}

/// Convert the domain's `u32` back to PG's signed `i32`. Saturating cast —
/// values above `i32::MAX` are clamped, which is harmless for a day-count.
#[allow(clippy::cast_possible_wrap, clippy::cast_possible_truncation)]
const fn pre_period_to_i32(v: u32) -> i32 {
    if v > i32::MAX as u32 {
        i32::MAX
    } else {
        v as i32
    }
}

fn parse_status(s: &str) -> ExperimentStatus {
    match s {
        "running" => ExperimentStatus::Running,
        "paused" => ExperimentStatus::Paused,
        "stopped" => ExperimentStatus::Stopped,
        _ => ExperimentStatus::Draft,
    }
}

#[async_trait]
impl ExperimentRepository for PgExperimentRepository {
    async fn find_by_id(&self, id: ExperimentId) -> Result<Experiment, RepositoryError> {
        let row = sqlx::query(
            r"
            SELECT
                e.id, e.env_id, e.flag_id, e.flag_rule_id, e.targets_default_rule,
                e.name, e.description, e.hypothesis, e.status,
                e.metric_ids, e.guardrail_metric_ids,
                e.traffic_allocation::float8 AS traffic_allocation,
                e.min_sample_size, e.pre_period_days, e.unit_context_types,
                e.scheduled_start_at, e.scheduled_end_at,
                e.version, e.created_at, e.updated_at, e.deleted_at,
                e.exclusion_group_id, e.group_bucket_lo, e.group_bucket_hi,
                e.sequential_testing_enabled, e.sequential_alpha,
                e.sequential_tau_squared, e.sequential_min_sample_size,
                f.key AS flag_key,
                COALESCE((
                    SELECT ARRAY_AGG(v.key ORDER BY v.id)
                    FROM variants v
                    WHERE v.flag_id = e.flag_id
                ), '{}') AS variant_keys
            FROM experiments e
            LEFT JOIN feature_flags f ON f.id = e.flag_id AND f.deleted_at IS NULL
            WHERE e.id = $1 AND e.deleted_at IS NULL
            ",
        )
        .bind(id.as_uuid())
        .fetch_optional(&self.pool)
        .await
        .map_err(RepositoryError::Database)?;

        let row = row.ok_or_else(|| RepositoryError::NotFound { id: id.to_string() })?;
        Ok(row_to_experiment(&row))
    }

    async fn list_by_environment(
        &self,
        env_id: EnvironmentId,
        status_filter: Option<ExperimentStatus>,
    ) -> Result<Vec<Experiment>, RepositoryError> {
        let rows = sqlx::query(
            r"
            SELECT
                e.id, e.env_id, e.flag_id, e.flag_rule_id, e.targets_default_rule,
                e.name, e.description, e.hypothesis, e.status,
                e.metric_ids, e.guardrail_metric_ids,
                e.traffic_allocation::float8 AS traffic_allocation,
                e.min_sample_size, e.pre_period_days, e.unit_context_types,
                e.scheduled_start_at, e.scheduled_end_at,
                e.version, e.created_at, e.updated_at, e.deleted_at,
                e.exclusion_group_id, e.group_bucket_lo, e.group_bucket_hi,
                e.sequential_testing_enabled, e.sequential_alpha,
                e.sequential_tau_squared, e.sequential_min_sample_size,
                f.key AS flag_key,
                COALESCE((
                    SELECT ARRAY_AGG(v.key ORDER BY v.id)
                    FROM variants v
                    WHERE v.flag_id = e.flag_id
                ), '{}') AS variant_keys
            FROM experiments e
            LEFT JOIN feature_flags f ON f.id = e.flag_id AND f.deleted_at IS NULL
            WHERE e.env_id = $1 AND e.deleted_at IS NULL
            ORDER BY e.created_at
            ",
        )
        .bind(env_id.as_uuid())
        .fetch_all(&self.pool)
        .await
        .map_err(RepositoryError::Database)?;

        let experiments: Vec<Experiment> = rows.iter().map(row_to_experiment).collect();

        if let Some(status) = status_filter {
            Ok(experiments
                .into_iter()
                .filter(|e| e.status == status)
                .collect())
        } else {
            Ok(experiments)
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
                e.id, e.env_id, e.flag_id, e.flag_rule_id, e.targets_default_rule,
                e.name, e.description, e.hypothesis, e.status,
                e.metric_ids, e.guardrail_metric_ids,
                e.traffic_allocation::float8 AS traffic_allocation,
                e.min_sample_size, e.pre_period_days, e.unit_context_types,
                e.scheduled_start_at, e.scheduled_end_at,
                e.version, e.created_at, e.updated_at, e.deleted_at,
                e.exclusion_group_id, e.group_bucket_lo, e.group_bucket_hi,
                e.sequential_testing_enabled, e.sequential_alpha,
                e.sequential_tau_squared, e.sequential_min_sample_size,
                f.key AS flag_key,
                COALESCE((
                    SELECT ARRAY_AGG(v.key ORDER BY v.id)
                    FROM variants v
                    WHERE v.flag_id = e.flag_id
                ), '{}') AS variant_keys,
                COUNT(*) OVER() AS total_count
            FROM experiments e
            LEFT JOIN feature_flags f ON f.id = e.flag_id AND f.deleted_at IS NULL
            WHERE e.env_id = $1 AND e.deleted_at IS NULL
            ORDER BY e.created_at
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

        let experiments: Vec<Experiment> = rows.iter().map(row_to_experiment).collect();

        Ok((experiments, total))
    }

    async fn create(&self, experiment: &Experiment) -> Result<(), RepositoryError> {
        let metric_uuids: Vec<uuid::Uuid> = experiment
            .metric_ids
            .iter()
            .map(MetricId::as_uuid)
            .collect();
        let guardrail_uuids: Vec<uuid::Uuid> = experiment
            .guardrail_metric_ids
            .iter()
            .map(MetricId::as_uuid)
            .collect();

        sqlx::query(
            r"
            INSERT INTO experiments
                (id, env_id, flag_id, flag_rule_id, targets_default_rule, name, description,
                 hypothesis, status, metric_ids, guardrail_metric_ids, traffic_allocation,
                 min_sample_size, pre_period_days, unit_context_types,
                 scheduled_start_at, scheduled_end_at,
                 sequential_testing_enabled, sequential_alpha,
                 sequential_tau_squared, sequential_min_sample_size,
                 version, created_at, updated_at, deleted_at)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9::text, $10, $11,
                    $12::float8::numeric, $13, $14, $15, $16, $17,
                    $18, $19, $20, $21, $22, $23, $24, $25)
            ",
        )
        .bind(experiment.id.as_uuid())
        .bind(experiment.environment_id.as_uuid())
        .bind(experiment.flag_id.as_uuid())
        .bind(experiment.flag_rule_id.map(|r| r.as_uuid()))
        .bind(experiment.targets_default_rule)
        .bind(&experiment.name)
        .bind(&experiment.description)
        .bind(&experiment.hypothesis)
        .bind(experiment_status_str(experiment.status))
        .bind(&metric_uuids)
        .bind(&guardrail_uuids)
        .bind(experiment.traffic_allocation)
        .bind(experiment.min_sample_size)
        .bind(pre_period_to_i32(experiment.pre_period_days))
        .bind(&experiment.unit_context_types)
        .bind(experiment.scheduled_start_at)
        .bind(experiment.scheduled_end_at)
        .bind(experiment.sequential_testing_enabled)
        .bind(experiment.sequential_alpha)
        .bind(experiment.sequential_tau_squared)
        .bind(experiment.sequential_min_sample_size)
        .bind(experiment.version)
        .bind(experiment.created_at)
        .bind(experiment.updated_at)
        .bind(experiment.deleted_at)
        .execute(&self.pool)
        .await
        .map_err(map_experiment_db_err)?;

        self.audit
            .log(
                None,
                "experiment",
                experiment.id.as_uuid(),
                "create",
                serde_json::json!({
                    "name": experiment.name,
                    "environment_id": experiment.environment_id.to_string(),
                    "flag_id": experiment.flag_id.to_string(),
                    "flag_rule_id": experiment.flag_rule_id.map(|r| r.to_string()),
                    "targets_default_rule": experiment.targets_default_rule,
                    "unit_context_types": experiment.unit_context_types,
                    "status": format!("{:?}", experiment.status),
                }),
            )
            .await?;

        Ok(())
    }

    #[allow(clippy::too_many_lines)]
    async fn update(&self, experiment: &Experiment) -> Result<Experiment, RepositoryError> {
        // Mutation guard: reject updates on active (running or paused) experiments.
        let current_status: Option<String> = sqlx::query_scalar(
            "SELECT status FROM experiments WHERE id = $1 AND deleted_at IS NULL",
        )
        .bind(experiment.id.as_uuid())
        .fetch_optional(&self.pool)
        .await
        .map_err(RepositoryError::Database)?;

        match current_status.as_deref() {
            None => {
                return Err(RepositoryError::NotFound {
                    id: experiment.id.to_string(),
                });
            }
            Some("running" | "paused") => {
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
        let guardrail_uuids: Vec<uuid::Uuid> = experiment
            .guardrail_metric_ids
            .iter()
            .map(MetricId::as_uuid)
            .collect();

        let row = sqlx::query(
            r"
            UPDATE experiments
            SET name = $1,
                description = $2,
                hypothesis = $3,
                status = $4::text,
                metric_ids = $5,
                guardrail_metric_ids = $6,
                traffic_allocation = $7::float8::numeric,
                min_sample_size = $8,
                pre_period_days = $9,
                unit_context_types = $10,
                scheduled_start_at = $11,
                scheduled_end_at = $12,
                sequential_testing_enabled = $13,
                sequential_alpha = $14,
                sequential_tau_squared = $15,
                sequential_min_sample_size = $16,
                updated_at = NOW(),
                version = $17
            WHERE id = $18 AND version = $19 AND deleted_at IS NULL
            RETURNING
                id, env_id, flag_id, flag_rule_id, targets_default_rule, name, description,
                hypothesis, status, metric_ids, guardrail_metric_ids,
                traffic_allocation::float8 AS traffic_allocation,
                min_sample_size, pre_period_days, unit_context_types,
                scheduled_start_at, scheduled_end_at,
                version, created_at, updated_at, deleted_at,
                exclusion_group_id, group_bucket_lo, group_bucket_hi,
                sequential_testing_enabled, sequential_alpha,
                sequential_tau_squared, sequential_min_sample_size
            ",
        )
        .bind(&experiment.name)
        .bind(&experiment.description)
        .bind(&experiment.hypothesis)
        .bind(experiment_status_str(experiment.status))
        .bind(&metric_uuids)
        .bind(&guardrail_uuids)
        .bind(experiment.traffic_allocation)
        .bind(experiment.min_sample_size)
        .bind(pre_period_to_i32(experiment.pre_period_days))
        .bind(&experiment.unit_context_types)
        .bind(experiment.scheduled_start_at)
        .bind(experiment.scheduled_end_at)
        .bind(experiment.sequential_testing_enabled)
        .bind(experiment.sequential_alpha)
        .bind(experiment.sequential_tau_squared)
        .bind(experiment.sequential_min_sample_size)
        .bind(new_version)
        .bind(experiment.id.as_uuid())
        .bind(experiment.version)
        .fetch_optional(&self.pool)
        .await
        .map_err(RepositoryError::Database)?;

        if let Some(row) = row {
            let updated = row_to_experiment(&row);
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
            return Ok(updated);
        }

        // Distinguish NotFound vs VersionConflict.
        let current = sqlx::query_scalar::<_, i64>(
            "SELECT version FROM experiments WHERE id = $1 AND deleted_at IS NULL",
        )
        .bind(experiment.id.as_uuid())
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

    async fn soft_delete(&self, id: ExperimentId) -> Result<(), RepositoryError> {
        // Fetch the current status to reject running experiments.
        let current: Option<String> = sqlx::query_scalar(
            "SELECT status FROM experiments WHERE id = $1 AND deleted_at IS NULL",
        )
        .bind(id.as_uuid())
        .fetch_optional(&self.pool)
        .await
        .map_err(RepositoryError::Database)?;

        let Some(status_str) = current else {
            return Err(RepositoryError::NotFound { id: id.to_string() });
        };

        if status_str == "running" {
            return Err(RepositoryError::InvalidState {
                reason: "cannot delete a running experiment; pause or stop it first".to_string(),
            });
        }

        sqlx::query(
            r"
            UPDATE experiments
            SET deleted_at = NOW(), updated_at = NOW()
            WHERE id = $1 AND deleted_at IS NULL
            ",
        )
        .bind(id.as_uuid())
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
        let rows = sqlx::query(
            r"
            SELECT
                id, experiment_id, flag_id, iteration_number, started_at, ended_at,
                metric_ids, guardrail_metric_ids,
                traffic_allocation::float8 AS traffic_allocation,
                min_sample_size, targets_default_rule, pre_period_days,
                unit_context_types, default_rule_distribution,
                exclusion_group_id, group_bucket_lo, group_bucket_hi,
                sequential_testing_enabled, sequential_alpha,
                sequential_tau_squared, sequential_min_sample_size
            FROM experiment_iterations
            WHERE experiment_id = $1
            ORDER BY iteration_number
            ",
        )
        .bind(experiment_id.as_uuid())
        .fetch_all(&self.pool)
        .await
        .map_err(RepositoryError::Database)?;

        rows.iter().map(row_to_iteration).collect()
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
        //     other non-deleted experiment on the same flag is already running
        //     or paused (excluding self).
        if to == ExperimentStatus::Running || to == ExperimentStatus::Paused {
            let conflict_count: i64 = sqlx::query_scalar(
                r"
                SELECT COUNT(*)::bigint
                FROM experiments
                WHERE flag_id = $1
                  AND id <> $2
                  AND status IN ('running', 'paused')
                  AND deleted_at IS NULL
                ",
            )
            .bind(current.flag_id.as_uuid())
            .bind(id.as_uuid())
            .fetch_one(&mut *tx)
            .await
            .map_err(RepositoryError::Database)?;

            if conflict_count > 0 {
                return Err(RepositoryError::UniqueViolation {
                    field: "flag_id".to_string(),
                });
            }
        }

        // 3b. Update experiment status and bump version (optimistic concurrency).
        let new_version = current.version + 1;
        let updated_row = sqlx::query(
            r"
            UPDATE experiments
            SET status    = $1::text,
                version   = $2,
                updated_at = NOW()
            WHERE id = $3 AND version = $4 AND deleted_at IS NULL
            RETURNING
                id, env_id, flag_id, flag_rule_id, targets_default_rule, name, description,
                hypothesis, status, metric_ids, guardrail_metric_ids,
                traffic_allocation::float8 AS traffic_allocation,
                min_sample_size, pre_period_days, unit_context_types,
                scheduled_start_at, scheduled_end_at,
                version, created_at, updated_at, deleted_at,
                exclusion_group_id, group_bucket_lo, group_bucket_hi,
                sequential_testing_enabled, sequential_alpha,
                sequential_tau_squared, sequential_min_sample_size
            ",
        )
        .bind(experiment_status_str(to))
        .bind(new_version)
        .bind(id.as_uuid())
        .bind(current.version)
        .fetch_optional(&mut *tx)
        .await
        .map_err(RepositoryError::Database)?
        .ok_or(RepositoryError::VersionConflict {
            expected: current.version,
            actual: new_version,
        })?;

        let updated = row_to_experiment(&updated_row);

        // 3c. If transitioning into Running: end the previous active iteration (if any),
        //     then insert a new iteration with the snapshot columns.
        if to == ExperimentStatus::Running {
            sqlx::query(
                r"
                UPDATE experiment_iterations
                SET ended_at = NOW()
                WHERE experiment_id = $1 AND ended_at IS NULL
                ",
            )
            .bind(id.as_uuid())
            .execute(&mut *tx)
            .await
            .map_err(RepositoryError::Database)?;

            let max_iter: Option<i32> = sqlx::query_scalar(
                r"
                SELECT MAX(iteration_number)
                FROM experiment_iterations
                WHERE experiment_id = $1
                ",
            )
            .bind(id.as_uuid())
            .fetch_one(&mut *tx)
            .await
            .map_err(RepositoryError::Database)?;

            let next_iteration = max_iter.unwrap_or(0) + 1;
            let iteration_id = ExperimentIterationId::new();
            let metric_uuids: Vec<uuid::Uuid> =
                current.metric_ids.iter().map(MetricId::as_uuid).collect();
            let guardrail_uuids: Vec<uuid::Uuid> = current
                .guardrail_metric_ids
                .iter()
                .map(MetricId::as_uuid)
                .collect();

            // Snapshot the flag's default_rule_distribution at iteration
            // start when the experiment is bound to the default rule.
            let dist_json: Option<serde_json::Value> = if current.targets_default_rule {
                let raw: Option<serde_json::Value> = sqlx::query_scalar(
                    "SELECT default_rule_distribution FROM feature_flags WHERE id = $1",
                )
                .bind(current.flag_id.as_uuid())
                .fetch_optional(&mut *tx)
                .await
                .map_err(RepositoryError::Database)?
                .flatten();
                raw
            } else {
                None
            };

            sqlx::query(
                r"
                INSERT INTO experiment_iterations
                    (id, experiment_id, flag_id, iteration_number, started_at, metric_ids,
                     guardrail_metric_ids, traffic_allocation, min_sample_size,
                     targets_default_rule, pre_period_days, unit_context_types,
                     default_rule_distribution,
                     exclusion_group_id, group_bucket_lo, group_bucket_hi,
                     sequential_testing_enabled, sequential_alpha,
                     sequential_tau_squared, sequential_min_sample_size)
                VALUES ($1, $2, $3, $4, NOW(), $5, $6, $7::float8::numeric, $8, $9, $10, $11, $12,
                        $13, $14, $15, $16, $17, $18, $19)
                ",
            )
            .bind(iteration_id.as_uuid())
            .bind(id.as_uuid())
            .bind(current.flag_id.as_uuid())
            .bind(next_iteration)
            .bind(&metric_uuids)
            .bind(&guardrail_uuids)
            .bind(current.traffic_allocation)
            .bind(current.min_sample_size)
            .bind(current.targets_default_rule)
            .bind(pre_period_to_i32(current.pre_period_days))
            .bind(&current.unit_context_types)
            .bind(dist_json)
            // Snapshot the exclusion-group membership at iteration start, mirroring
            // the other config fields — so retrospective per-iteration analysis
            // reflects the group/range that was actually in force during the run.
            .bind(current.exclusion_group_id.map(|g| g.as_uuid()))
            .bind(current.group_bucket_lo.map(i32::from))
            .bind(current.group_bucket_hi.map(i32::from))
            // Snapshot the sequential-testing config at iteration start, so the
            // analysis uses the α / τ² / min-sample that were in force at the
            // run, not whatever the experiment row holds at compute time.
            .bind(current.sequential_testing_enabled)
            .bind(current.sequential_alpha)
            .bind(current.sequential_tau_squared)
            .bind(current.sequential_min_sample_size)
            .execute(&mut *tx)
            .await
            .map_err(RepositoryError::Database)?;
        }

        // 3d. Flag access-control during an experiment is enforced by the
        //     whole-flag lock (`is_flag_locked`, derived from experiment status),
        //     not a per-rule column. The retired `feature_flag_rules.frozen`
        //     write path was removed in PROP-001 (domain_boundaries_20260530).

        // 3e. If from == Running and transitioning to Paused or Stopped: end active iteration.
        if from == ExperimentStatus::Running
            && (to == ExperimentStatus::Paused || to == ExperimentStatus::Stopped)
        {
            sqlx::query(
                r"
                UPDATE experiment_iterations
                SET ended_at = NOW()
                WHERE experiment_id = $1 AND ended_at IS NULL
                ",
            )
            .bind(id.as_uuid())
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
        // Resolve `variant_keys` via the variants subquery (mirroring
        // `find_by_id` / `list_by_environment`) so the stats-service compute
        // pass can source the FULL configured variant set for SRM zero-fill — a
        // starved (zero-assignment) arm is otherwise invisible. (See the
        // stats-service `srm_json_for` FIX.)
        let rows = sqlx::query(
            r"
            SELECT
                e.id, e.env_id, e.flag_id, e.flag_rule_id, e.targets_default_rule,
                e.name, e.description, e.hypothesis, e.status,
                e.metric_ids, e.guardrail_metric_ids,
                e.traffic_allocation::float8 AS traffic_allocation,
                e.min_sample_size, e.pre_period_days, e.unit_context_types,
                e.scheduled_start_at, e.scheduled_end_at,
                e.exclusion_group_id, e.group_bucket_lo, e.group_bucket_hi,
                e.sequential_testing_enabled, e.sequential_alpha,
                e.sequential_tau_squared, e.sequential_min_sample_size,
                e.version, e.created_at, e.updated_at, e.deleted_at,
                f.key AS flag_key,
                COALESCE((
                    SELECT ARRAY_AGG(v.key ORDER BY v.id)
                    FROM variants v
                    WHERE v.flag_id = e.flag_id
                ), '{}') AS variant_keys
            FROM experiments e
            LEFT JOIN feature_flags f ON f.id = e.flag_id AND f.deleted_at IS NULL
            WHERE e.status = 'running' AND e.deleted_at IS NULL
            ORDER BY e.created_at
            ",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(RepositoryError::Database)?;

        Ok(rows.iter().map(row_to_experiment).collect())
    }

    async fn find_active_experiment_for_flag(
        &self,
        flag_id: FlagId,
    ) -> Result<Option<ExperimentId>, RepositoryError> {
        use sqlx::Row as _;
        // Per-flag uniqueness (idx_experiments_one_active_per_flag) guarantees
        // at most one row here; we still LIMIT 1 defensively.
        let row = sqlx::query(
            r"
            SELECT id
            FROM experiments
            WHERE flag_id = $1
              AND status IN ('running', 'paused')
              AND deleted_at IS NULL
            LIMIT 1
            ",
        )
        .bind(flag_id.as_uuid())
        .fetch_optional(&self.pool)
        .await
        .map_err(RepositoryError::Database)?;

        Ok(row.map(|r| ExperimentId::from_uuid(r.get("id"))))
    }

    async fn find_iteration_by_id(
        &self,
        iteration_id: ExperimentIterationId,
    ) -> Result<ExperimentIteration, RepositoryError> {
        let row = sqlx::query(
            r"
            SELECT
                id, experiment_id, flag_id, iteration_number, started_at, ended_at,
                metric_ids, guardrail_metric_ids,
                traffic_allocation::float8 AS traffic_allocation,
                min_sample_size, targets_default_rule, pre_period_days,
                unit_context_types, default_rule_distribution,
                exclusion_group_id, group_bucket_lo, group_bucket_hi,
                sequential_testing_enabled, sequential_alpha,
                sequential_tau_squared, sequential_min_sample_size
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

        row_to_iteration(&row)
    }
}

// ---------------------------------------------------------------------------
// Free helpers
// ---------------------------------------------------------------------------

const fn experiment_status_str(s: ExperimentStatus) -> &'static str {
    match s {
        ExperimentStatus::Draft => "draft",
        ExperimentStatus::Running => "running",
        ExperimentStatus::Paused => "paused",
        ExperimentStatus::Stopped => "stopped",
    }
}

fn row_to_experiment(row: &sqlx::postgres::PgRow) -> Experiment {
    use sqlx::Row as _;
    let metric_uuids: Vec<uuid::Uuid> = row.get("metric_ids");
    let guardrail_uuids: Vec<uuid::Uuid> = row.get("guardrail_metric_ids");
    let status_str: &str = row.get("status");
    let flag_rule_uuid: Option<uuid::Uuid> = row.get("flag_rule_id");
    // flag_key / variant_keys are populated when the query JOINs on feature_flags/variants.
    let flag_key: Option<String> = row.try_get("flag_key").ok();
    let variant_keys: Vec<String> = row.try_get("variant_keys").unwrap_or_default();

    Experiment {
        id: ExperimentId::from_uuid(row.get("id")),
        environment_id: EnvironmentId::from_uuid(row.get("env_id")),
        flag_id: FlagId::from_uuid(row.get("flag_id")),
        flag_key,
        variant_keys,
        flag_rule_id: flag_rule_uuid.map(RuleId::from_uuid),
        targets_default_rule: row.get("targets_default_rule"),
        name: row.get("name"),
        description: row.get("description"),
        hypothesis: row.get("hypothesis"),
        status: parse_status(status_str),
        metric_ids: metric_uuids.into_iter().map(MetricId::from_uuid).collect(),
        guardrail_metric_ids: guardrail_uuids
            .into_iter()
            .map(MetricId::from_uuid)
            .collect(),
        traffic_allocation: row.get("traffic_allocation"),
        min_sample_size: row.get("min_sample_size"),
        pre_period_days: pre_period_to_u32(row.get::<i32, _>("pre_period_days")),
        unit_context_types: row.get::<Vec<String>, _>("unit_context_types"),
        scheduled_start_at: row.get("scheduled_start_at"),
        scheduled_end_at: row.get("scheduled_end_at"),
        version: row.get("version"),
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
        deleted_at: row.get("deleted_at"),
        exclusion_group_id: row
            .get::<Option<uuid::Uuid>, _>("exclusion_group_id")
            .map(ExclusionGroupId::from_uuid),
        group_bucket_lo: row
            .get::<Option<i32>, _>("group_bucket_lo")
            .map(bucket_to_u16),
        group_bucket_hi: row
            .get::<Option<i32>, _>("group_bucket_hi")
            .map(bucket_to_u16),
        sequential_testing_enabled: row.get("sequential_testing_enabled"),
        sequential_alpha: row.get("sequential_alpha"),
        sequential_tau_squared: row.get("sequential_tau_squared"),
        sequential_min_sample_size: row.get("sequential_min_sample_size"),
    }
}

/// Map an `INTEGER` group-bucket column value (0..=10000 basis points) to the
/// `u16` the domain model uses. Out-of-range / negative values (which the
/// `experiments_group_bucket_range` CHECK constraint forbids) are clamped into
/// `u16` defensively rather than panicking on a malformed row.
#[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
fn bucket_to_u16(n: i32) -> u16 {
    n.clamp(0, i32::from(u16::MAX)) as u16
}

fn row_to_iteration(row: &sqlx::postgres::PgRow) -> Result<ExperimentIteration, RepositoryError> {
    use sqlx::Row as _;
    let metric_uuids: Vec<uuid::Uuid> = row.get("metric_ids");
    let guardrail_uuids: Vec<uuid::Uuid> = row.get("guardrail_metric_ids");
    let dist_json: Option<serde_json::Value> = row.get("default_rule_distribution");
    let default_rule_distribution: Option<RolloutDistribution> = match dist_json {
        None => None,
        Some(v) => Some(serde_json::from_value(v).map_err(|e| {
            RepositoryError::Unexpected(anyhow::anyhow!(
                "default_rule_distribution JSONB malformed: {e}"
            ))
        })?),
    };

    Ok(ExperimentIteration {
        id: ExperimentIterationId::from_uuid(row.get("id")),
        experiment_id: ExperimentId::from_uuid(row.get("experiment_id")),
        flag_id: FlagId::from_uuid(row.get("flag_id")),
        iteration_number: row.get("iteration_number"),
        started_at: row.get("started_at"),
        ended_at: row.get("ended_at"),
        metric_ids: metric_uuids.into_iter().map(MetricId::from_uuid).collect(),
        guardrail_metric_ids: guardrail_uuids
            .into_iter()
            .map(MetricId::from_uuid)
            .collect(),
        traffic_allocation: row.get("traffic_allocation"),
        min_sample_size: row.get("min_sample_size"),
        targets_default_rule: row.get("targets_default_rule"),
        pre_period_days: pre_period_to_u32(row.get::<i32, _>("pre_period_days")),
        unit_context_types: row.get::<Vec<String>, _>("unit_context_types"),
        default_rule_distribution,
        exclusion_group_id: row
            .get::<Option<uuid::Uuid>, _>("exclusion_group_id")
            .map(ExclusionGroupId::from_uuid),
        group_bucket_lo: row
            .get::<Option<i32>, _>("group_bucket_lo")
            .map(bucket_to_u16),
        group_bucket_hi: row
            .get::<Option<i32>, _>("group_bucket_hi")
            .map(bucket_to_u16),
        sequential_testing_enabled: row.get("sequential_testing_enabled"),
        sequential_alpha: row.get("sequential_alpha"),
        sequential_tau_squared: row.get("sequential_tau_squared"),
        sequential_min_sample_size: row.get("sequential_min_sample_size"),
    })
}

fn map_experiment_db_err(e: sqlx::Error) -> RepositoryError {
    if let sqlx::Error::Database(ref dbe) = e {
        let code_cow = dbe.code();
        let code = code_cow.as_deref().unwrap_or("");
        let constraint = dbe.constraint().unwrap_or("unknown").to_string();
        return match code {
            "23505" => RepositoryError::UniqueViolation { field: constraint },
            "23503" => RepositoryError::ForeignKeyViolation { constraint },
            _ if dbe.constraint().is_some() => {
                RepositoryError::UniqueViolation { field: constraint }
            }
            _ => RepositoryError::Database(e),
        };
    }
    RepositoryError::Database(e)
}
