//! Repository trait and Postgres implementation for `experiment_results`.
//!
//! Each row represents the computed statistical analysis for one metric
//! within one iteration of an experiment.

use async_trait::async_trait;
use sqlx::PgPool;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Row types
// ---------------------------------------------------------------------------

/// A fully-hydrated row from the `experiment_results` table.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ExperimentResultRow {
    /// Primary key.
    pub id: Uuid,
    /// FK → `experiments.id`.
    pub experiment_id: Uuid,
    /// FK → `experiment_iterations.id`.
    pub iteration_id: Uuid,
    /// Metric key (e.g. `"checkout_completed"`).
    pub metric_key: String,
    /// Metric type discriminant: `"count"`, `"numeric"`, `"percentile"`, or `"funnel"`.
    pub metric_type: String,
    /// Per-variant sample statistics (JSONB).
    pub variant_stats: serde_json::Value,
    /// Frequentist analysis result, if computed (JSONB).
    pub frequentist_result: Option<serde_json::Value>,
    /// Bayesian analysis result, if computed (JSONB).
    pub bayesian_result: Option<serde_json::Value>,
    /// Human-readable recommendation string.
    pub recommendation: String,
    /// When this result was computed.
    pub computed_at: chrono::DateTime<chrono::Utc>,
    /// When this row was first inserted.
    pub created_at: chrono::DateTime<chrono::Utc>,
}

/// Input type for [`ExperimentResultsRepository::upsert`].
#[derive(Debug, Clone)]
pub struct UpsertResultRow {
    /// FK → `experiments.id`.
    pub experiment_id: Uuid,
    /// FK → `experiment_iterations.id`.
    pub iteration_id: Uuid,
    /// Metric key.
    pub metric_key: String,
    /// Metric type discriminant.
    pub metric_type: String,
    /// Per-variant sample statistics (JSONB).
    pub variant_stats: serde_json::Value,
    /// Frequentist analysis result (JSONB), optional.
    pub frequentist_result: Option<serde_json::Value>,
    /// Bayesian analysis result (JSONB), optional.
    pub bayesian_result: Option<serde_json::Value>,
    /// Human-readable recommendation.
    pub recommendation: String,
    /// Timestamp the analysis was computed.
    pub computed_at: chrono::DateTime<chrono::Utc>,
}

// ---------------------------------------------------------------------------
// Trait
// ---------------------------------------------------------------------------

/// Data-access interface for the `experiment_results` table.
#[async_trait]
pub trait ExperimentResultsRepository: Send + Sync {
    /// Upsert one result row.
    ///
    /// On conflict on `(experiment_id, iteration_id, metric_key)` all mutable
    /// columns are updated in-place.
    async fn upsert(&self, row: &UpsertResultRow) -> Result<ExperimentResultRow, sqlx::Error>;

    /// Fetch all results for the **latest** iteration of an experiment.
    ///
    /// "Latest" is defined as the iteration with the highest `iteration_number`.
    async fn fetch_latest(
        &self,
        experiment_id: Uuid,
    ) -> Result<Vec<ExperimentResultRow>, sqlx::Error>;

    /// Fetch all results for a specific iteration.
    async fn fetch_by_iteration(
        &self,
        experiment_id: Uuid,
        iteration_id: Uuid,
    ) -> Result<Vec<ExperimentResultRow>, sqlx::Error>;

    /// Return `true` if the results are stale for the given iteration.
    ///
    /// Stale means either:
    /// - no result rows exist yet for this `(experiment_id, iteration_id)`, or
    /// - the minimum `computed_at` across all result rows is earlier than
    ///   `experiment_iterations.started_at`.
    async fn is_stale(&self, experiment_id: Uuid, iteration_id: Uuid) -> Result<bool, sqlx::Error>;
}

// ---------------------------------------------------------------------------
// Postgres implementation
// ---------------------------------------------------------------------------

/// Postgres-backed implementation of [`ExperimentResultsRepository`].
pub struct PgExperimentResultsRepository {
    pool: PgPool,
}

impl PgExperimentResultsRepository {
    /// Construct a new repository bound to `pool`.
    #[must_use]
    pub const fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl ExperimentResultsRepository for PgExperimentResultsRepository {
    async fn upsert(&self, row: &UpsertResultRow) -> Result<ExperimentResultRow, sqlx::Error> {
        sqlx::query_as::<_, ExperimentResultRow>(
            r"
            INSERT INTO experiment_results
                (experiment_id, iteration_id, metric_key, metric_type,
                 variant_stats, frequentist_result, bayesian_result,
                 recommendation, computed_at)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
            ON CONFLICT (experiment_id, iteration_id, metric_key) DO UPDATE
                SET metric_type          = EXCLUDED.metric_type,
                    variant_stats        = EXCLUDED.variant_stats,
                    frequentist_result   = EXCLUDED.frequentist_result,
                    bayesian_result      = EXCLUDED.bayesian_result,
                    recommendation       = EXCLUDED.recommendation,
                    computed_at          = EXCLUDED.computed_at
            RETURNING
                id,
                experiment_id,
                iteration_id,
                metric_key,
                metric_type,
                variant_stats,
                frequentist_result,
                bayesian_result,
                recommendation,
                computed_at,
                created_at
            ",
        )
        .bind(row.experiment_id)
        .bind(row.iteration_id)
        .bind(&row.metric_key)
        .bind(&row.metric_type)
        .bind(&row.variant_stats)
        .bind(&row.frequentist_result)
        .bind(&row.bayesian_result)
        .bind(&row.recommendation)
        .bind(row.computed_at)
        .fetch_one(&self.pool)
        .await
    }

    async fn fetch_latest(
        &self,
        experiment_id: Uuid,
    ) -> Result<Vec<ExperimentResultRow>, sqlx::Error> {
        sqlx::query_as::<_, ExperimentResultRow>(
            r"
            SELECT
                er.id,
                er.experiment_id,
                er.iteration_id,
                er.metric_key,
                er.metric_type,
                er.variant_stats,
                er.frequentist_result,
                er.bayesian_result,
                er.recommendation,
                er.computed_at,
                er.created_at
            FROM experiment_results er
            INNER JOIN experiment_iterations ei ON ei.id = er.iteration_id
            WHERE er.experiment_id = $1
              AND ei.iteration_number = (
                  SELECT MAX(iteration_number)
                  FROM experiment_iterations
                  WHERE experiment_id = $1
              )
            ORDER BY er.metric_key
            ",
        )
        .bind(experiment_id)
        .fetch_all(&self.pool)
        .await
    }

    async fn fetch_by_iteration(
        &self,
        experiment_id: Uuid,
        iteration_id: Uuid,
    ) -> Result<Vec<ExperimentResultRow>, sqlx::Error> {
        sqlx::query_as::<_, ExperimentResultRow>(
            r"
            SELECT
                id,
                experiment_id,
                iteration_id,
                metric_key,
                metric_type,
                variant_stats,
                frequentist_result,
                bayesian_result,
                recommendation,
                computed_at,
                created_at
            FROM experiment_results
            WHERE experiment_id = $1 AND iteration_id = $2
            ORDER BY metric_key
            ",
        )
        .bind(experiment_id)
        .bind(iteration_id)
        .fetch_all(&self.pool)
        .await
    }

    async fn is_stale(&self, experiment_id: Uuid, iteration_id: Uuid) -> Result<bool, sqlx::Error> {
        // If MIN(computed_at) IS NULL → no rows exist → stale.
        // If MIN(computed_at) < started_at → at least one result is older than the
        // iteration start → stale.
        let row: Option<(
            Option<chrono::DateTime<chrono::Utc>>,
            chrono::DateTime<chrono::Utc>,
        )> = sqlx::query_as(
            r"
                SELECT
                    MIN(er.computed_at) AS min_computed_at,
                    ei.started_at
                FROM experiment_iterations ei
                LEFT JOIN experiment_results er
                    ON er.experiment_id = ei.experiment_id
                   AND er.iteration_id  = ei.id
                WHERE ei.experiment_id = $1
                  AND ei.id            = $2
                GROUP BY ei.started_at
                ",
        )
        .bind(experiment_id)
        .bind(iteration_id)
        .fetch_optional(&self.pool)
        .await?;

        let Some((min_computed_at, started_at)) = row else {
            // Iteration not found — treat as stale.
            return Ok(true);
        };

        Ok(min_computed_at.is_none_or(|computed_at| computed_at < started_at))
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    use crate::{
        EnvironmentRepository, ExperimentRepository, FlagRepository, OrganisationRepository,
        ProjectRepository,
        repository::pg::{
            PgAuditLogger, PgEnvironmentRepository, PgExperimentRepository, PgFlagRepository,
            PgOrganisationRepository, PgProjectRepository,
        },
    };
    use chrono::Utc;
    use stitchd_core::{
        experimentation::{Experiment, ExperimentStatus},
        flag::{FlagRecord, FlagRule, FlagValueType},
        id::{
            EnvironmentId, ExperimentId, FlagId, FlagKey, OrganisationId, ProjectId, RuleId,
            VariantId,
        },
        rule_engine::types::{ConditionExpr, Rule, RuleOutput},
        tenant::{Environment, Organisation, Project},
    };

    // -----------------------------------------------------------------------
    // Helpers
    // -----------------------------------------------------------------------

    async fn setup_experiment_deps(pool: sqlx::PgPool) -> (EnvironmentId, RuleId) {
        let audit = Arc::new(PgAuditLogger::new(pool.clone()));
        let org_repo = PgOrganisationRepository::new(pool.clone(), audit.clone());
        let proj_repo = PgProjectRepository::new(pool.clone(), audit.clone());
        let env_repo = PgEnvironmentRepository::new(pool.clone(), audit.clone());
        let flag_repo = PgFlagRepository::new(pool.clone(), audit.clone());

        let org = Organisation {
            id: OrganisationId::new(),
            name: "TestOrg".into(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            deleted_at: None,
            version: 1,
        };
        org_repo.create(&org).await.unwrap();

        let project = Project {
            id: ProjectId::new(),
            organisation_id: org.id,
            name: "TestProj".into(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            deleted_at: None,
            version: 1,
        };
        proj_repo.create(&project).await.unwrap();

        let env = Environment {
            id: EnvironmentId::new(),
            project_id: project.id,
            name: "TestEnv".into(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            deleted_at: None,
            version: 1,
        };
        env_repo.create(&env).await.unwrap();

        let flag = FlagRecord {
            id: FlagId::new(),
            project_id: project.id,
            key: FlagKey::new("res-flag").unwrap(),
            value_type: FlagValueType::Bool,
            enabled: true,
            default_variant_id: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            deleted_at: None,
            version: 1,
        };
        flag_repo.create(&flag).await.unwrap();

        let rules = vec![FlagRule {
            flag_id: flag.id,
            rule_index: 0,
            rule: Rule {
                id: RuleId::new(),
                condition: ConditionExpr::And(vec![]),
                output: RuleOutput::Variant(VariantId::new()),
            },
        }];
        flag_repo.upsert_rules(flag.id, &rules).await.unwrap();

        let rule_uuid: uuid::Uuid = sqlx::query_scalar!(
            "SELECT id FROM feature_flag_rules WHERE flag_id = $1 LIMIT 1",
            flag.id.as_uuid()
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        let flag_rule_id = RuleId::from_uuid(rule_uuid);

        (env.id, flag_rule_id)
    }

    fn make_experiment(env_id: EnvironmentId, flag_rule_id: RuleId) -> Experiment {
        Experiment {
            id: ExperimentId::new(),
            environment_id: env_id,
            flag_rule_id,
            name: "Results Test Experiment".into(),
            description: None,
            hypothesis: None,
            metric_keys: vec!["checkout".into(), "revenue".into()],
            traffic_allocation: 100.0,
            min_sample_size: None,
            scheduled_start_at: None,
            scheduled_end_at: None,
            status: ExperimentStatus::Draft,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            deleted_at: None,
            version: 1,
        }
    }

    fn make_upsert_row(
        experiment_id: Uuid,
        iteration_id: Uuid,
        metric_key: &str,
    ) -> UpsertResultRow {
        UpsertResultRow {
            experiment_id,
            iteration_id,
            metric_key: metric_key.to_string(),
            metric_type: "count".to_string(),
            variant_stats: serde_json::json!({ "control": 100, "treatment": 120 }),
            frequentist_result: Some(serde_json::json!({ "p_value": 0.03 })),
            bayesian_result: None,
            recommendation: "ship_treatment".to_string(),
            computed_at: Utc::now(),
        }
    }

    // -----------------------------------------------------------------------
    // Tests
    // -----------------------------------------------------------------------

    /// Upsert a single result and read it back via `fetch_by_iteration`.
    #[sqlx::test(migrations = "./migrations")]
    async fn test_upsert_and_fetch_by_iteration(pool: sqlx::PgPool) {
        let (env_id, rule_id) = setup_experiment_deps(pool.clone()).await;
        let audit = Arc::new(PgAuditLogger::new(pool.clone()));
        let exp_repo = PgExperimentRepository::new(pool.clone(), audit);
        let results_repo = PgExperimentResultsRepository::new(pool.clone());

        let exp = make_experiment(env_id, rule_id);
        exp_repo.create(&exp).await.unwrap();

        // Transition to running so an iteration is created.
        exp_repo
            .apply_transition(exp.id, ExperimentStatus::Running, None)
            .await
            .unwrap();

        let iterations = exp_repo.list_iterations(exp.id).await.unwrap();
        assert_eq!(iterations.len(), 1);
        let iter = &iterations[0];

        let input = make_upsert_row(exp.id.as_uuid(), iter.id.as_uuid(), "checkout");
        let returned = results_repo
            .upsert(&input)
            .await
            .expect("upsert should succeed");

        assert_eq!(returned.experiment_id, exp.id.as_uuid());
        assert_eq!(returned.iteration_id, iter.id.as_uuid());
        assert_eq!(returned.metric_key, "checkout");
        assert_eq!(returned.metric_type, "count");
        assert_eq!(returned.recommendation, "ship_treatment");
        assert!(returned.bayesian_result.is_none());
        assert!(returned.frequentist_result.is_some());

        let rows = results_repo
            .fetch_by_iteration(exp.id.as_uuid(), iter.id.as_uuid())
            .await
            .expect("fetch_by_iteration should succeed");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].metric_key, "checkout");
    }

    /// Upsert the same key twice; the second call should update the row.
    #[sqlx::test(migrations = "./migrations")]
    async fn test_upsert_is_idempotent(pool: sqlx::PgPool) {
        let (env_id, rule_id) = setup_experiment_deps(pool.clone()).await;
        let audit = Arc::new(PgAuditLogger::new(pool.clone()));
        let exp_repo = PgExperimentRepository::new(pool.clone(), audit);
        let results_repo = PgExperimentResultsRepository::new(pool.clone());

        let exp = make_experiment(env_id, rule_id);
        exp_repo.create(&exp).await.unwrap();
        exp_repo
            .apply_transition(exp.id, ExperimentStatus::Running, None)
            .await
            .unwrap();

        let iterations = exp_repo.list_iterations(exp.id).await.unwrap();
        let iter = &iterations[0];

        let mut input = make_upsert_row(exp.id.as_uuid(), iter.id.as_uuid(), "checkout");
        results_repo.upsert(&input).await.unwrap();

        // Second upsert with updated recommendation.
        input.recommendation = "keep_control".to_string();
        let returned = results_repo
            .upsert(&input)
            .await
            .expect("second upsert should succeed");
        assert_eq!(returned.recommendation, "keep_control");

        // Should still be exactly one row.
        let rows = results_repo
            .fetch_by_iteration(exp.id.as_uuid(), iter.id.as_uuid())
            .await
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].recommendation, "keep_control");
    }

    /// Multiple metrics can be stored for one iteration.
    #[sqlx::test(migrations = "./migrations")]
    async fn test_multiple_metrics_per_iteration(pool: sqlx::PgPool) {
        let (env_id, rule_id) = setup_experiment_deps(pool.clone()).await;
        let audit = Arc::new(PgAuditLogger::new(pool.clone()));
        let exp_repo = PgExperimentRepository::new(pool.clone(), audit);
        let results_repo = PgExperimentResultsRepository::new(pool.clone());

        let exp = make_experiment(env_id, rule_id);
        exp_repo.create(&exp).await.unwrap();
        exp_repo
            .apply_transition(exp.id, ExperimentStatus::Running, None)
            .await
            .unwrap();

        let iterations = exp_repo.list_iterations(exp.id).await.unwrap();
        let iter = &iterations[0];

        for key in &["checkout", "revenue", "add_to_cart"] {
            let input = make_upsert_row(exp.id.as_uuid(), iter.id.as_uuid(), key);
            results_repo.upsert(&input).await.unwrap();
        }

        let rows = results_repo
            .fetch_by_iteration(exp.id.as_uuid(), iter.id.as_uuid())
            .await
            .unwrap();
        assert_eq!(rows.len(), 3);
    }

    /// `fetch_latest` returns results for the latest iteration only.
    #[sqlx::test(migrations = "./migrations")]
    async fn test_fetch_latest_returns_newest_iteration(pool: sqlx::PgPool) {
        let (env_id, rule_id) = setup_experiment_deps(pool.clone()).await;
        let audit = Arc::new(PgAuditLogger::new(pool.clone()));
        let exp_repo = PgExperimentRepository::new(pool.clone(), audit);
        let results_repo = PgExperimentResultsRepository::new(pool.clone());

        let exp = make_experiment(env_id, rule_id);
        exp_repo.create(&exp).await.unwrap();

        // Iteration 1: draft → running
        exp_repo
            .apply_transition(exp.id, ExperimentStatus::Running, None)
            .await
            .unwrap();
        let iters1 = exp_repo.list_iterations(exp.id).await.unwrap();
        let iter1 = &iters1[0];

        let input1 = make_upsert_row(exp.id.as_uuid(), iter1.id.as_uuid(), "checkout");
        results_repo.upsert(&input1).await.unwrap();

        // running → paused → running (iteration 2)
        exp_repo
            .apply_transition(exp.id, ExperimentStatus::Paused, None)
            .await
            .unwrap();
        exp_repo
            .apply_transition(exp.id, ExperimentStatus::Running, None)
            .await
            .unwrap();
        let iters2 = exp_repo.list_iterations(exp.id).await.unwrap();
        assert_eq!(iters2.len(), 2);
        let iter2 = &iters2[1];

        let mut input2 = make_upsert_row(exp.id.as_uuid(), iter2.id.as_uuid(), "checkout");
        input2.recommendation = "iter2_recommendation".to_string();
        results_repo.upsert(&input2).await.unwrap();

        // fetch_latest should return iteration 2's results.
        let latest = results_repo
            .fetch_latest(exp.id.as_uuid())
            .await
            .expect("fetch_latest should succeed");
        assert_eq!(latest.len(), 1);
        assert_eq!(latest[0].recommendation, "iter2_recommendation");
        assert_eq!(latest[0].iteration_id, iter2.id.as_uuid());
    }

    /// `fetch_latest` returns empty when no results exist.
    #[sqlx::test(migrations = "./migrations")]
    async fn test_fetch_latest_empty_when_no_results(pool: sqlx::PgPool) {
        let (env_id, rule_id) = setup_experiment_deps(pool.clone()).await;
        let audit = Arc::new(PgAuditLogger::new(pool.clone()));
        let exp_repo = PgExperimentRepository::new(pool.clone(), audit);
        let results_repo = PgExperimentResultsRepository::new(pool.clone());

        let exp = make_experiment(env_id, rule_id);
        exp_repo.create(&exp).await.unwrap();
        exp_repo
            .apply_transition(exp.id, ExperimentStatus::Running, None)
            .await
            .unwrap();

        let latest = results_repo
            .fetch_latest(exp.id.as_uuid())
            .await
            .expect("fetch_latest on empty should return empty vec");
        assert!(latest.is_empty());
    }

    /// `is_stale` returns `true` when no results have been computed.
    #[sqlx::test(migrations = "./migrations")]
    async fn test_is_stale_true_when_no_results(pool: sqlx::PgPool) {
        let (env_id, rule_id) = setup_experiment_deps(pool.clone()).await;
        let audit = Arc::new(PgAuditLogger::new(pool.clone()));
        let exp_repo = PgExperimentRepository::new(pool.clone(), audit);
        let results_repo = PgExperimentResultsRepository::new(pool.clone());

        let exp = make_experiment(env_id, rule_id);
        exp_repo.create(&exp).await.unwrap();
        exp_repo
            .apply_transition(exp.id, ExperimentStatus::Running, None)
            .await
            .unwrap();

        let iters = exp_repo.list_iterations(exp.id).await.unwrap();
        let iter = &iters[0];

        let stale = results_repo
            .is_stale(exp.id.as_uuid(), iter.id.as_uuid())
            .await
            .expect("is_stale should not error");
        assert!(stale, "should be stale when no results exist");
    }

    /// `is_stale` returns `false` when results were computed after iteration start.
    #[sqlx::test(migrations = "./migrations")]
    async fn test_is_stale_false_when_fresh(pool: sqlx::PgPool) {
        let (env_id, rule_id) = setup_experiment_deps(pool.clone()).await;
        let audit = Arc::new(PgAuditLogger::new(pool.clone()));
        let exp_repo = PgExperimentRepository::new(pool.clone(), audit);
        let results_repo = PgExperimentResultsRepository::new(pool.clone());

        let exp = make_experiment(env_id, rule_id);
        exp_repo.create(&exp).await.unwrap();
        exp_repo
            .apply_transition(exp.id, ExperimentStatus::Running, None)
            .await
            .unwrap();

        let iters = exp_repo.list_iterations(exp.id).await.unwrap();
        let iter = &iters[0];

        // Use computed_at = now() which is after started_at (also ~now()).
        // To ensure computed_at > started_at, add a small offset.
        let mut input = make_upsert_row(exp.id.as_uuid(), iter.id.as_uuid(), "checkout");
        input.computed_at = Utc::now() + chrono::Duration::seconds(1);
        results_repo.upsert(&input).await.unwrap();

        let stale = results_repo
            .is_stale(exp.id.as_uuid(), iter.id.as_uuid())
            .await
            .expect("is_stale should not error");
        assert!(!stale, "should not be stale when computed_at > started_at");
    }

    /// `is_stale` returns `true` for unknown iteration IDs.
    #[sqlx::test(migrations = "./migrations")]
    async fn test_is_stale_true_for_unknown_iteration(pool: sqlx::PgPool) {
        let results_repo = PgExperimentResultsRepository::new(pool.clone());
        let stale = results_repo
            .is_stale(Uuid::new_v4(), Uuid::new_v4())
            .await
            .expect("is_stale should not error for unknown IDs");
        assert!(stale, "unknown iteration should be treated as stale");
    }
}
