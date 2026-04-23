//! Periodic scheduler that iterates running experiments and triggers stats computation.

use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;

/// A running experiment with its active iteration, ready for stats computation.
#[derive(Debug, Clone)]
pub struct RunningExperiment {
    pub experiment_id: Uuid,
    pub env_id: Uuid,
    pub metric_keys: Vec<String>,
    pub iteration_id: Uuid,
    pub started_at: DateTime<Utc>,
    /// `None` while the iteration is still active (experiment is running).
    pub ended_at: Option<DateTime<Utc>>,
}

/// Fetch all running experiments together with their active iteration from PostgreSQL.
///
/// Only returns experiments with `status = 'running'` and an active (non-ended) iteration.
pub async fn fetch_running_experiments(pool: &PgPool) -> Result<Vec<RunningExperiment>, sqlx::Error> {
    sqlx::query_as::<_, RunningExperimentRow>(
        r"
        SELECT
            e.id          AS experiment_id,
            e.env_id      AS env_id,
            ei.metric_keys AS metric_keys,
            ei.id         AS iteration_id,
            ei.started_at AS started_at,
            ei.ended_at   AS ended_at
        FROM experiments e
        INNER JOIN experiment_iterations ei ON ei.experiment_id = e.id
        WHERE e.status = 'running'
          AND e.deleted_at IS NULL
          AND ei.ended_at IS NULL
        ",
    )
    .fetch_all(pool)
    .await
    .map(|rows| rows.into_iter().map(RunningExperiment::from).collect())
}

#[derive(sqlx::FromRow)]
struct RunningExperimentRow {
    experiment_id: Uuid,
    env_id: Uuid,
    metric_keys: Vec<String>,
    iteration_id: Uuid,
    started_at: DateTime<Utc>,
    ended_at: Option<DateTime<Utc>>,
}

impl From<RunningExperimentRow> for RunningExperiment {
    fn from(r: RunningExperimentRow) -> Self {
        Self {
            experiment_id: r.experiment_id,
            env_id: r.env_id,
            metric_keys: r.metric_keys,
            iteration_id: r.iteration_id,
            started_at: r.started_at,
            ended_at: r.ended_at,
        }
    }
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

    async fn setup_experiment(pool: sqlx::PgPool) -> (EnvironmentId, RuleId) {
        let audit = Arc::new(PgAuditLogger::new(pool.clone()));
        let org_repo = PgOrganisationRepository::new(pool.clone(), audit.clone());
        let proj_repo = PgProjectRepository::new(pool.clone(), audit.clone());
        let env_repo = PgEnvironmentRepository::new(pool.clone(), audit.clone());
        let flag_repo = PgFlagRepository::new(pool.clone(), audit.clone());

        let org = Organisation {
            id: OrganisationId::new(),
            name: "SchedOrg".into(),
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
            name: "SchedProj".into(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            deleted_at: None,
            version: 1,
        };
        proj_repo.create(&project).await.unwrap();

        let env = Environment {
            id: EnvironmentId::new(),
            project_id: project.id,
            name: "SchedEnv".into(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            deleted_at: None,
            version: 1,
        };
        env_repo.create(&env).await.unwrap();

        let flag = FlagRecord {
            id: FlagId::new(),
            project_id: project.id,
            key: FlagKey::new("sched-flag").unwrap(),
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

        let rule_uuid: uuid::Uuid = sqlx::query_scalar(
            "SELECT id FROM feature_flag_rules WHERE flag_id = $1 LIMIT 1",
        )
        .bind(flag.id.as_uuid())
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
            name: "Sched Test".into(),
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
        }
    }

    #[sqlx::test(migrations = "../../crates/stitchd-db/migrations")]
    async fn test_fetch_running_experiments_returns_only_running(pool: sqlx::PgPool) {
        let (env_id, rule_id) = setup_experiment(pool.clone()).await;
        let audit = Arc::new(PgAuditLogger::new(pool.clone()));
        let exp_repo = PgExperimentRepository::new(pool.clone(), audit);

        // Draft experiment — should NOT be returned.
        let draft_exp = make_experiment(env_id, rule_id);
        exp_repo.create(&draft_exp).await.unwrap();

        let results = fetch_running_experiments(&pool).await.unwrap();
        assert!(
            results.is_empty(),
            "draft experiment should not appear in running list"
        );
    }

    #[sqlx::test(migrations = "../../crates/stitchd-db/migrations")]
    async fn test_fetch_running_experiments_includes_running(pool: sqlx::PgPool) {
        let (env_id, rule_id) = setup_experiment(pool.clone()).await;
        let audit = Arc::new(PgAuditLogger::new(pool.clone()));
        let exp_repo = PgExperimentRepository::new(pool.clone(), audit);

        let exp = make_experiment(env_id, rule_id);
        exp_repo.create(&exp).await.unwrap();
        exp_repo
            .apply_transition(exp.id, ExperimentStatus::Running, None)
            .await
            .unwrap();

        let results = fetch_running_experiments(&pool).await.unwrap();
        assert_eq!(results.len(), 1, "running experiment should appear");
        assert_eq!(results[0].experiment_id, exp.id.as_uuid());
        assert!(!results[0].metric_keys.is_empty());
        assert!(results[0].ended_at.is_none(), "active iteration has no ended_at");
    }

    #[sqlx::test(migrations = "../../crates/stitchd-db/migrations")]
    async fn test_fetch_running_experiments_excludes_paused(pool: sqlx::PgPool) {
        let (env_id, rule_id) = setup_experiment(pool.clone()).await;
        let audit = Arc::new(PgAuditLogger::new(pool.clone()));
        let exp_repo = PgExperimentRepository::new(pool.clone(), audit);

        let exp = make_experiment(env_id, rule_id);
        exp_repo.create(&exp).await.unwrap();
        exp_repo
            .apply_transition(exp.id, ExperimentStatus::Running, None)
            .await
            .unwrap();
        exp_repo
            .apply_transition(exp.id, ExperimentStatus::Paused, None)
            .await
            .unwrap();

        let results = fetch_running_experiments(&pool).await.unwrap();
        assert!(results.is_empty(), "paused experiment should not appear");
    }
}
