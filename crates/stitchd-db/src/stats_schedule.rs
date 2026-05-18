//! Repository trait and Postgres implementation for `stats_schedule`.
//!
//! Each row tracks the per-experiment schedule state: when stats were last
//! computed, when the next run is scheduled, and the current computation status.

use async_trait::async_trait;
use sqlx::PgPool;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Domain types
// ---------------------------------------------------------------------------

/// Computation status for a scheduled stats run.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::Type)]
#[sqlx(type_name = "text", rename_all = "snake_case")]
pub enum ComputationStatus {
    /// Stats have been computed and are up to date.
    Ready,
    /// Stats are currently being computed.
    Computing,
    /// Stats have never been computed for this experiment.
    NeverComputed,
}

/// A fully-hydrated row from the `stats_schedule` table.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct StatsScheduleRow {
    /// FK → `experiments.id`.
    pub experiment_id: Uuid,
    /// When stats were last computed, if ever.
    pub last_computed_at: Option<chrono::DateTime<chrono::Utc>>,
    /// When the next stats run is scheduled, if set.
    pub next_run_at: Option<chrono::DateTime<chrono::Utc>>,
    /// Current computation status.
    pub computation_status: ComputationStatus,
    /// When this row was last updated.
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

/// Input type for [`StatsScheduleRepository::upsert_schedule`].
#[derive(Debug, Clone)]
pub struct UpsertStatsSchedule {
    /// FK → `experiments.id`.
    pub experiment_id: Uuid,
    /// When stats were last computed, if ever.
    pub last_computed_at: Option<chrono::DateTime<chrono::Utc>>,
    /// When the next stats run is scheduled, if set.
    pub next_run_at: Option<chrono::DateTime<chrono::Utc>>,
    /// Current computation status.
    pub computation_status: ComputationStatus,
}

// ---------------------------------------------------------------------------
// Trait
// ---------------------------------------------------------------------------

/// Data-access interface for the `stats_schedule` table.
#[async_trait]
pub trait StatsScheduleRepository: Send + Sync {
    /// Upsert a schedule row for the given experiment.
    ///
    /// On conflict on `experiment_id` all mutable columns are updated in-place
    /// and `updated_at` is set to `NOW()`.
    async fn upsert_schedule(
        &self,
        input: &UpsertStatsSchedule,
    ) -> Result<StatsScheduleRow, sqlx::Error>;

    /// Fetch the schedule row for a given experiment, returning `None` if it
    /// does not exist.
    async fn get_schedule_for_experiment(
        &self,
        experiment_id: Uuid,
    ) -> Result<Option<StatsScheduleRow>, sqlx::Error>;

    /// Update `last_computed_at` for a given iteration's parent experiment.
    ///
    /// Sets `last_computed_at` to the provided timestamp (in milliseconds since
    /// the Unix epoch) and `updated_at` to `NOW()`. If no schedule row exists
    /// for the experiment yet, this is a no-op (returns `Ok(())`).
    async fn update_last_computed(
        &self,
        iteration_id: Uuid,
        last_computed_at_ms: i64,
    ) -> Result<(), sqlx::Error>;
}

// ---------------------------------------------------------------------------
// Postgres implementation
// ---------------------------------------------------------------------------

/// Postgres-backed implementation of [`StatsScheduleRepository`].
pub struct PgStatsScheduleRepository {
    pool: PgPool,
}

impl PgStatsScheduleRepository {
    /// Construct a new repository bound to `pool`.
    #[must_use]
    pub const fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl StatsScheduleRepository for PgStatsScheduleRepository {
    async fn upsert_schedule(
        &self,
        input: &UpsertStatsSchedule,
    ) -> Result<StatsScheduleRow, sqlx::Error> {
        sqlx::query_as::<_, StatsScheduleRow>(
            r"
            INSERT INTO stats_schedule
                (experiment_id, last_computed_at, next_run_at, computation_status)
            VALUES ($1, $2, $3, $4)
            ON CONFLICT (experiment_id) DO UPDATE
                SET last_computed_at    = EXCLUDED.last_computed_at,
                    next_run_at         = EXCLUDED.next_run_at,
                    computation_status  = EXCLUDED.computation_status,
                    updated_at          = NOW()
            RETURNING
                experiment_id,
                last_computed_at,
                next_run_at,
                computation_status,
                updated_at
            ",
        )
        .bind(input.experiment_id)
        .bind(input.last_computed_at)
        .bind(input.next_run_at)
        .bind(&input.computation_status)
        .fetch_one(&self.pool)
        .await
    }

    async fn get_schedule_for_experiment(
        &self,
        experiment_id: Uuid,
    ) -> Result<Option<StatsScheduleRow>, sqlx::Error> {
        sqlx::query_as::<_, StatsScheduleRow>(
            r"
            SELECT
                experiment_id,
                last_computed_at,
                next_run_at,
                computation_status,
                updated_at
            FROM stats_schedule
            WHERE experiment_id = $1
            ",
        )
        .bind(experiment_id)
        .fetch_optional(&self.pool)
        .await
    }

    async fn update_last_computed(
        &self,
        iteration_id: Uuid,
        last_computed_at_ms: i64,
    ) -> Result<(), sqlx::Error> {
        use chrono::TimeZone as _;

        // Resolve the experiment_id from the iteration.
        let experiment_id: Option<Uuid> = sqlx::query_scalar(
            "SELECT experiment_id FROM experiment_iterations WHERE id = $1",
        )
        .bind(iteration_id)
        .fetch_optional(&self.pool)
        .await?;

        let Some(experiment_id) = experiment_id else {
            // Iteration not found — no-op.
            return Ok(());
        };

        // Convert milliseconds to a timestamp.
        let secs = last_computed_at_ms / 1000;
        let nanos = (last_computed_at_ms % 1000) * 1_000_000;
        let ts = chrono::Utc
            .timestamp_opt(secs, u32::try_from(nanos).unwrap_or(0))
            .single()
            .unwrap_or_else(chrono::Utc::now);

        sqlx::query(
            r"
            UPDATE stats_schedule
            SET last_computed_at = $1,
                updated_at       = NOW()
            WHERE experiment_id = $2
            ",
        )
        .bind(ts)
        .bind(experiment_id)
        .execute(&self.pool)
        .await?;

        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
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

    /// Build the org → project → env → flag → rule chain; return (`env_id`, `flag_rule_id`).
    async fn build_env_context(pool: &sqlx::PgPool) -> (EnvironmentId, RuleId) {
        let audit = Arc::new(PgAuditLogger::new(pool.clone()));
        let org_repo = PgOrganisationRepository::new(pool.clone(), audit.clone());
        let proj_repo = PgProjectRepository::new(pool.clone(), audit.clone());
        let env_repo = PgEnvironmentRepository::new(pool.clone(), audit.clone());
        let flag_repo = PgFlagRepository::new(pool.clone(), audit.clone());

        let org = Organisation {
            id: OrganisationId::new(),
            name: "SchedTestOrg".into(),
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
            name: "SchedTestProj".into(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            deleted_at: None,
            version: 1,
        };
        proj_repo.create(&project).await.unwrap();

        let env = Environment {
            id: EnvironmentId::new(),
            project_id: project.id,
            name: "SchedTestEnv".into(),
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

        let rule_uuid: uuid::Uuid = sqlx::query_scalar!(
            "SELECT id FROM feature_flag_rules WHERE flag_id = $1 LIMIT 1",
            flag.id.as_uuid()
        )
        .fetch_one(pool)
        .await
        .unwrap();

        (env.id, RuleId::from_uuid(rule_uuid))
    }

    /// Create a minimal org → project → env → flag → rule → experiment chain.
    /// Returns the experiment's UUID, which satisfies the FK on `stats_schedule`.
    async fn setup_experiment(pool: sqlx::PgPool) -> Uuid {
        let (env_id, flag_rule_id) = build_env_context(&pool).await;
        let audit = Arc::new(PgAuditLogger::new(pool.clone()));
        let exp_repo = PgExperimentRepository::new(pool.clone(), audit);

        let exp = Experiment {
            id: ExperimentId::new(),
            environment_id: env_id,
            flag_rule_id,
            name: "Sched Test Experiment".into(),
            description: None,
            hypothesis: None,
            metric_keys: vec!["checkout".into()],
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
        exp.id.as_uuid()
    }

    fn make_upsert(experiment_id: Uuid, status: ComputationStatus) -> UpsertStatsSchedule {
        UpsertStatsSchedule {
            experiment_id,
            last_computed_at: None,
            next_run_at: None,
            computation_status: status,
        }
    }

    /// Upserting for a new `experiment_id` should create a row with the correct status.
    #[sqlx::test(migrations = "./migrations")]
    async fn test_upsert_schedule_creates_new(pool: sqlx::PgPool) {
        let experiment_id = setup_experiment(pool.clone()).await;
        let repo = PgStatsScheduleRepository::new(pool);

        let input = make_upsert(experiment_id, ComputationStatus::NeverComputed);
        let row = repo
            .upsert_schedule(&input)
            .await
            .expect("upsert_schedule should succeed");

        assert_eq!(row.experiment_id, experiment_id);
        assert_eq!(row.computation_status, ComputationStatus::NeverComputed);
        assert!(row.last_computed_at.is_none());
        assert!(row.next_run_at.is_none());
    }

    /// Upserting twice for the same `experiment_id` should update the row.
    #[sqlx::test(migrations = "./migrations")]
    async fn test_upsert_schedule_updates_existing(pool: sqlx::PgPool) {
        let experiment_id = setup_experiment(pool.clone()).await;
        let repo = PgStatsScheduleRepository::new(pool);

        let input1 = make_upsert(experiment_id, ComputationStatus::NeverComputed);
        repo.upsert_schedule(&input1)
            .await
            .expect("first upsert should succeed");

        let now = chrono::Utc::now();
        let input2 = UpsertStatsSchedule {
            experiment_id,
            last_computed_at: Some(now),
            next_run_at: Some(now + chrono::Duration::hours(1)),
            computation_status: ComputationStatus::Ready,
        };
        let row = repo
            .upsert_schedule(&input2)
            .await
            .expect("second upsert should succeed");

        assert_eq!(row.experiment_id, experiment_id);
        assert_eq!(row.computation_status, ComputationStatus::Ready);
        assert!(row.last_computed_at.is_some());
        assert!(row.next_run_at.is_some());
    }

    /// Upserting with `ComputationStatus::Computing` should persist that status.
    #[sqlx::test(migrations = "./migrations")]
    async fn test_upsert_schedule_sets_computation_status(pool: sqlx::PgPool) {
        let experiment_id = setup_experiment(pool.clone()).await;
        let repo = PgStatsScheduleRepository::new(pool);

        let input = make_upsert(experiment_id, ComputationStatus::Computing);
        let row = repo
            .upsert_schedule(&input)
            .await
            .expect("upsert_schedule should succeed");

        assert_eq!(row.computation_status, ComputationStatus::Computing);
    }

    /// After upserting, `get_schedule_for_experiment` should return the row with matching fields.
    #[sqlx::test(migrations = "./migrations")]
    async fn test_get_schedule_for_experiment(pool: sqlx::PgPool) {
        let experiment_id = setup_experiment(pool.clone()).await;
        let repo = PgStatsScheduleRepository::new(pool);
        let now = chrono::Utc::now();

        let input = UpsertStatsSchedule {
            experiment_id,
            last_computed_at: Some(now),
            next_run_at: Some(now + chrono::Duration::minutes(30)),
            computation_status: ComputationStatus::Ready,
        };
        repo.upsert_schedule(&input)
            .await
            .expect("upsert_schedule should succeed");

        let row = repo
            .get_schedule_for_experiment(experiment_id)
            .await
            .expect("get_schedule_for_experiment should not error")
            .expect("row should exist");

        assert_eq!(row.experiment_id, experiment_id);
        assert_eq!(row.computation_status, ComputationStatus::Ready);
        assert!(row.last_computed_at.is_some());
        assert!(row.next_run_at.is_some());
    }

    /// `get_schedule_for_experiment` should return `None` for an unknown experiment.
    #[sqlx::test(migrations = "./migrations")]
    async fn test_get_schedule_for_experiment_not_found(pool: sqlx::PgPool) {
        let repo = PgStatsScheduleRepository::new(pool);
        let result = repo
            .get_schedule_for_experiment(Uuid::new_v4())
            .await
            .expect("get_schedule_for_experiment should not error for missing id");

        assert!(result.is_none(), "expected None for unknown experiment_id");
    }
}
