//! Writes computed per-metric statistics to the `experiment_results` table.

use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;

/// A summarized metric result for one (variant_group, metric_key) pair,
/// ready to be written to `experiment_results`.
#[derive(Debug, Clone)]
pub struct MetricSummary {
    pub metric_key: String,
    /// JSON object mapping variant_key → event count.
    pub variant_stats: serde_json::Value,
    /// Optional frequentist analysis result (JSONB).
    pub frequentist_result: Option<serde_json::Value>,
    /// Optional bayesian analysis result (JSONB).
    pub bayesian_result: Option<serde_json::Value>,
    pub recommendation: String,
}

/// Upsert computed metric summaries into `experiment_results`.
///
/// Each `MetricSummary` maps to one row keyed on `(experiment_id, iteration_id, metric_key)`.
pub async fn write_results(
    pool: &PgPool,
    experiment_id: Uuid,
    iteration_id: Uuid,
    computed_at: DateTime<Utc>,
    summaries: &[MetricSummary],
) -> Result<(), sqlx::Error> {
    for summary in summaries {
        sqlx::query(
            r"
            INSERT INTO experiment_results
                (experiment_id, iteration_id, metric_key, metric_type,
                 variant_stats, frequentist_result, bayesian_result,
                 recommendation, computed_at)
            VALUES ($1, $2, $3, 'count', $4, $5, $6, $7, $8)
            ON CONFLICT (experiment_id, iteration_id, metric_key) DO UPDATE
                SET variant_stats      = EXCLUDED.variant_stats,
                    frequentist_result = EXCLUDED.frequentist_result,
                    bayesian_result    = EXCLUDED.bayesian_result,
                    recommendation     = EXCLUDED.recommendation,
                    computed_at        = EXCLUDED.computed_at
            ",
        )
        .bind(experiment_id)
        .bind(iteration_id)
        .bind(&summary.metric_key)
        .bind(&summary.variant_stats)
        .bind(&summary.frequentist_result)
        .bind(&summary.bayesian_result)
        .bind(&summary.recommendation)
        .bind(computed_at)
        .execute(pool)
        .await?;
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::Arc;

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
    use stitchd_db::{
        EnvironmentRepository, ExperimentRepository, FlagRepository, OrganisationRepository,
        ProjectRepository,
        repository::pg::{
            PgAuditLogger, PgEnvironmentRepository, PgExperimentRepository, PgFlagRepository,
            PgOrganisationRepository, PgProjectRepository,
        },
    };

    async fn setup_running_experiment(pool: sqlx::PgPool) -> (Uuid, Uuid) {
        let audit = Arc::new(PgAuditLogger::new(pool.clone()));
        let org_repo = PgOrganisationRepository::new(pool.clone(), audit.clone());
        let proj_repo = PgProjectRepository::new(pool.clone(), audit.clone());
        let env_repo = PgEnvironmentRepository::new(pool.clone(), audit.clone());
        let flag_repo = PgFlagRepository::new(pool.clone(), audit.clone());
        let exp_repo = PgExperimentRepository::new(pool.clone(), audit.clone());

        let org = Organisation {
            id: OrganisationId::new(),
            name: "RwOrg".into(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            deleted_at: None,
            version: 1,
            is_system: false,
        };
        org_repo.create(&org).await.unwrap();

        let project = Project {
            id: ProjectId::new(),
            organisation_id: org.id,
            name: "RwProj".into(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            deleted_at: None,
            version: 1,
        };
        proj_repo.create(&project).await.unwrap();

        let env = Environment {
            id: EnvironmentId::new(),
            project_id: project.id,
            name: "RwEnv".into(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            deleted_at: None,
            version: 1,
        };
        env_repo.create(&env).await.unwrap();

        let flag = FlagRecord {
            id: FlagId::new(),
            project_id: project.id,
            key: FlagKey::new("rw-flag").unwrap(),
            name: String::new(),
            description: String::new(),
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

        let rule_uuid: uuid::Uuid =
            sqlx::query_scalar("SELECT id FROM feature_flag_rules WHERE flag_id = $1 LIMIT 1")
                .bind(flag.id.as_uuid())
                .fetch_one(&pool)
                .await
                .unwrap();
        let flag_rule_id = RuleId::from_uuid(rule_uuid);

        let exp = Experiment {
            id: ExperimentId::new(),
            environment_id: env.id,
            flag_rule_id,
            name: "RwExp".into(),
            description: None,
            hypothesis: None,
            metric_keys: vec!["clicks".into()],
            traffic_allocation: 100.0,
            min_sample_size: None,
            scheduled_start_at: None,
            scheduled_end_at: None,
            status: ExperimentStatus::Draft,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            deleted_at: None,
            version: 1,
        };
        exp_repo.create(&exp).await.unwrap();
        exp_repo
            .apply_transition(exp.id, ExperimentStatus::Running, None)
            .await
            .unwrap();

        let iterations = exp_repo.list_iterations(exp.id).await.unwrap();
        let iter = &iterations[0];

        (exp.id.as_uuid(), iter.id.as_uuid())
    }

    #[sqlx::test(migrations = "../../crates/stitchd-db/migrations")]
    async fn test_write_results_inserts_row_with_correct_ids(pool: sqlx::PgPool) {
        let (exp_id, iter_id) = setup_running_experiment(pool.clone()).await;

        let summaries = vec![MetricSummary {
            metric_key: "clicks".into(),
            variant_stats: serde_json::json!({ "control": 50, "treatment": 70 }),
            frequentist_result: Some(serde_json::json!({ "p_value": 0.04 })),
            bayesian_result: None,
            recommendation: "ship_treatment".into(),
        }];

        write_results(&pool, exp_id, iter_id, Utc::now(), &summaries)
            .await
            .expect("write_results should succeed");

        let row: (Uuid, Uuid, String) = sqlx::query_as(
            "SELECT experiment_id, iteration_id, metric_key FROM experiment_results WHERE experiment_id = $1",
        )
        .bind(exp_id)
        .fetch_one(&pool)
        .await
        .unwrap();

        assert_eq!(row.0, exp_id, "experiment_id must match");
        assert_eq!(row.1, iter_id, "iteration_id must match");
        assert_eq!(row.2, "clicks");
    }

    #[sqlx::test(migrations = "../../crates/stitchd-db/migrations")]
    async fn test_write_results_upsert_is_idempotent(pool: sqlx::PgPool) {
        let (exp_id, iter_id) = setup_running_experiment(pool.clone()).await;

        let summaries = vec![MetricSummary {
            metric_key: "clicks".into(),
            variant_stats: serde_json::json!({ "control": 50 }),
            frequentist_result: None,
            bayesian_result: None,
            recommendation: "wait".into(),
        }];

        write_results(&pool, exp_id, iter_id, Utc::now(), &summaries)
            .await
            .unwrap();

        let updated = vec![MetricSummary {
            metric_key: "clicks".into(),
            variant_stats: serde_json::json!({ "control": 100 }),
            frequentist_result: Some(serde_json::json!({ "p_value": 0.01 })),
            bayesian_result: None,
            recommendation: "ship_treatment".into(),
        }];

        write_results(&pool, exp_id, iter_id, Utc::now(), &updated)
            .await
            .unwrap();

        let count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM experiment_results WHERE experiment_id = $1")
                .bind(exp_id)
                .fetch_one(&pool)
                .await
                .unwrap();

        assert_eq!(count, 1, "upsert must not duplicate rows");

        let rec: (String,) = sqlx::query_as(
            "SELECT recommendation FROM experiment_results WHERE experiment_id = $1",
        )
        .bind(exp_id)
        .fetch_one(&pool)
        .await
        .unwrap();

        assert_eq!(rec.0, "ship_treatment", "recommendation must be updated");
    }

    #[sqlx::test(migrations = "../../crates/stitchd-db/migrations")]
    async fn test_write_results_multiple_metrics(pool: sqlx::PgPool) {
        let (exp_id, iter_id) = setup_running_experiment(pool.clone()).await;

        let summaries = vec![
            MetricSummary {
                metric_key: "clicks".into(),
                variant_stats: serde_json::json!({ "control": 50 }),
                frequentist_result: None,
                bayesian_result: None,
                recommendation: "wait".into(),
            },
            MetricSummary {
                metric_key: "revenue".into(),
                variant_stats: serde_json::json!({ "control": 1000 }),
                frequentist_result: None,
                bayesian_result: None,
                recommendation: "wait".into(),
            },
        ];

        write_results(&pool, exp_id, iter_id, Utc::now(), &summaries)
            .await
            .unwrap();

        let count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM experiment_results WHERE experiment_id = $1")
                .bind(exp_id)
                .fetch_one(&pool)
                .await
                .unwrap();

        assert_eq!(count, 2, "one row per metric");
    }
}
