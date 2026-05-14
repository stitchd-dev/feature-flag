//! Updates `stats_schedule` after a successful stats computation run.

use chrono::{DateTime, Duration, Utc};
use sqlx::PgPool;
use uuid::Uuid;

/// Update `stats_schedule` after a successful computation run.
///
/// Sets `last_computed_at = computed_at`, `next_run_at = computed_at + interval`,
/// and `computation_status = 'ready'`.
pub async fn update_schedule_after_run(
    pool: &PgPool,
    experiment_id: Uuid,
    computed_at: DateTime<Utc>,
    interval: Duration,
) -> Result<(), sqlx::Error> {
    let next_run_at = computed_at + interval;

    sqlx::query(
        r"
        INSERT INTO stats_schedule (experiment_id, last_computed_at, next_run_at, computation_status, updated_at)
        VALUES ($1, $2, $3, 'ready', now())
        ON CONFLICT (experiment_id) DO UPDATE
            SET last_computed_at    = EXCLUDED.last_computed_at,
                next_run_at        = EXCLUDED.next_run_at,
                computation_status = 'ready',
                updated_at         = now()
        ",
    )
    .bind(experiment_id)
    .bind(computed_at)
    .bind(next_run_at)
    .execute(pool)
    .await?;

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

    async fn setup_running_experiment_id(pool: sqlx::PgPool) -> Uuid {
        let audit = Arc::new(PgAuditLogger::new(pool.clone()));
        let org_repo = PgOrganisationRepository::new(pool.clone(), audit.clone());
        let proj_repo = PgProjectRepository::new(pool.clone(), audit.clone());
        let env_repo = PgEnvironmentRepository::new(pool.clone(), audit.clone());
        let flag_repo = PgFlagRepository::new(pool.clone(), audit.clone());
        let exp_repo = PgExperimentRepository::new(pool.clone(), audit.clone());

        let org = Organisation {
            id: OrganisationId::new(),
            name: "SuOrg".into(),
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
            name: "SuProj".into(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            deleted_at: None,
            version: 1,
        };
        proj_repo.create(&project).await.unwrap();

        let env = Environment {
            id: EnvironmentId::new(),
            project_id: project.id,
            name: "SuEnv".into(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            deleted_at: None,
            version: 1,
        };
        env_repo.create(&env).await.unwrap();

        let flag = FlagRecord {
            id: FlagId::new(),
            project_id: project.id,
            key: FlagKey::new("su-flag").unwrap(),
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
                name: None,
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
            name: "SuExp".into(),
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

        exp.id.as_uuid()
    }

    #[sqlx::test(migrations = "../../crates/stitchd-db/migrations")]
    async fn test_update_schedule_sets_last_computed_at(pool: sqlx::PgPool) {
        let exp_id = setup_running_experiment_id(pool.clone()).await;
        let computed_at = Utc::now();
        let interval = Duration::hours(1);

        update_schedule_after_run(&pool, exp_id, computed_at, interval)
            .await
            .expect("update_schedule_after_run should succeed");

        let stored: DateTime<Utc> = sqlx::query_scalar(
            "SELECT last_computed_at FROM stats_schedule WHERE experiment_id = $1",
        )
        .bind(exp_id)
        .fetch_one(&pool)
        .await
        .unwrap();

        // DB stores microsecond precision; truncate to millis for comparison.
        assert_eq!(
            stored.timestamp_millis(),
            computed_at.timestamp_millis(),
            "last_computed_at must equal computed_at"
        );
    }

    #[sqlx::test(migrations = "../../crates/stitchd-db/migrations")]
    async fn test_update_schedule_sets_next_run_at(pool: sqlx::PgPool) {
        let exp_id = setup_running_experiment_id(pool.clone()).await;
        let computed_at = Utc::now();
        let interval = Duration::hours(1);

        update_schedule_after_run(&pool, exp_id, computed_at, interval)
            .await
            .unwrap();

        let next_run: DateTime<Utc> =
            sqlx::query_scalar("SELECT next_run_at FROM stats_schedule WHERE experiment_id = $1")
                .bind(exp_id)
                .fetch_one(&pool)
                .await
                .unwrap();

        let expected = computed_at + interval;
        assert_eq!(
            next_run.timestamp_millis(),
            expected.timestamp_millis(),
            "next_run_at must be computed_at + interval"
        );
    }

    #[sqlx::test(migrations = "../../crates/stitchd-db/migrations")]
    async fn test_update_schedule_sets_status_ready(pool: sqlx::PgPool) {
        let exp_id = setup_running_experiment_id(pool.clone()).await;

        update_schedule_after_run(&pool, exp_id, Utc::now(), Duration::hours(1))
            .await
            .unwrap();

        let status: String = sqlx::query_scalar(
            "SELECT computation_status FROM stats_schedule WHERE experiment_id = $1",
        )
        .bind(exp_id)
        .fetch_one(&pool)
        .await
        .unwrap();

        assert_eq!(status, "ready");
    }

    #[sqlx::test(migrations = "../../crates/stitchd-db/migrations")]
    async fn test_update_schedule_is_idempotent(pool: sqlx::PgPool) {
        let exp_id = setup_running_experiment_id(pool.clone()).await;
        let interval = Duration::hours(1);

        update_schedule_after_run(&pool, exp_id, Utc::now(), interval)
            .await
            .unwrap();

        let later_computed_at = Utc::now() + Duration::hours(1);
        update_schedule_after_run(&pool, exp_id, later_computed_at, interval)
            .await
            .unwrap();

        let count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM stats_schedule WHERE experiment_id = $1")
                .bind(exp_id)
                .fetch_one(&pool)
                .await
                .unwrap();

        assert_eq!(count, 1, "second call must upsert, not insert a new row");

        let stored_last: DateTime<Utc> = sqlx::query_scalar(
            "SELECT last_computed_at FROM stats_schedule WHERE experiment_id = $1",
        )
        .bind(exp_id)
        .fetch_one(&pool)
        .await
        .unwrap();

        assert_eq!(
            stored_last.timestamp_millis(),
            later_computed_at.timestamp_millis(),
            "second call must update last_computed_at"
        );
    }
}
