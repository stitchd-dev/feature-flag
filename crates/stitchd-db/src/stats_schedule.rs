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
    pub fn new(pool: PgPool) -> Self {
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
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn make_upsert(
        experiment_id: Uuid,
        status: ComputationStatus,
    ) -> UpsertStatsSchedule {
        UpsertStatsSchedule {
            experiment_id,
            last_computed_at: None,
            next_run_at: None,
            computation_status: status,
        }
    }

    /// Upserting for a new experiment_id should create a row with the correct status.
    #[sqlx::test(migrations = "./migrations")]
    async fn test_upsert_schedule_creates_new(pool: sqlx::PgPool) {
        let repo = PgStatsScheduleRepository::new(pool);
        let experiment_id = Uuid::new_v4();

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

    /// Upserting twice for the same experiment_id should update the row.
    #[sqlx::test(migrations = "./migrations")]
    async fn test_upsert_schedule_updates_existing(pool: sqlx::PgPool) {
        let repo = PgStatsScheduleRepository::new(pool);
        let experiment_id = Uuid::new_v4();

        // First upsert.
        let input1 = make_upsert(experiment_id, ComputationStatus::NeverComputed);
        repo.upsert_schedule(&input1)
            .await
            .expect("first upsert should succeed");

        // Second upsert with different status and timestamps.
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
        let repo = PgStatsScheduleRepository::new(pool);
        let experiment_id = Uuid::new_v4();

        let input = make_upsert(experiment_id, ComputationStatus::Computing);
        let row = repo
            .upsert_schedule(&input)
            .await
            .expect("upsert_schedule should succeed");

        assert_eq!(row.computation_status, ComputationStatus::Computing);
    }

    /// After upserting, `get_schedule_for_experiment` should return the row with
    /// matching fields.
    #[sqlx::test(migrations = "./migrations")]
    async fn test_get_schedule_for_experiment(pool: sqlx::PgPool) {
        let repo = PgStatsScheduleRepository::new(pool);
        let experiment_id = Uuid::new_v4();
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

        let fetched = repo
            .get_schedule_for_experiment(experiment_id)
            .await
            .expect("get_schedule_for_experiment should not error");

        let row = fetched.expect("row should exist");
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
