//! Repository trait and Postgres implementation for `stats_jobs`.
//!
//! Each row represents one scheduled statistics computation job, tracking its
//! lifecycle from pending through running to completed or failed.

use async_trait::async_trait;
use sqlx::PgPool;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Domain types
// ---------------------------------------------------------------------------

/// Status of a stats computation job.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::Type)]
#[sqlx(type_name = "text", rename_all = "snake_case")]
pub enum StatsJobStatus {
    /// Job has been created but not yet picked up by a worker.
    Pending,
    /// Job is currently being processed by a worker.
    Running,
    /// Job completed successfully.
    Completed,
    /// Job failed; see `error` field for details.
    Failed,
}

/// A fully-hydrated row from the `stats_jobs` table.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct StatsJobRow {
    /// Primary key.
    pub id: Uuid,
    /// FK → `experiments.id` (nullable for jobs not tied to a specific experiment).
    pub experiment_id: Option<Uuid>,
    /// Current lifecycle status of the job.
    pub status: StatsJobStatus,
    /// When this job was inserted.
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// When a worker began processing this job (set on Pending → Running).
    pub started_at: Option<chrono::DateTime<chrono::Utc>>,
    /// When the job finished, whether successfully or with an error.
    pub completed_at: Option<chrono::DateTime<chrono::Utc>>,
    /// Error message if the job failed.
    pub error: Option<String>,
}

/// Input type for [`StatsJobRepository::create_job`].
#[derive(Debug, Clone)]
pub struct CreateStatsJob {
    /// FK → `experiments.id`.
    pub experiment_id: Uuid,
}

// ---------------------------------------------------------------------------
// Trait
// ---------------------------------------------------------------------------

/// Data-access interface for the `stats_jobs` table.
#[async_trait]
pub trait StatsJobRepository: Send + Sync {
    /// Insert a new job in `Pending` status.
    async fn create_job(&self, input: &CreateStatsJob) -> Result<StatsJobRow, sqlx::Error>;

    /// Fetch a job by its primary key; returns `None` if not found.
    async fn get_job(&self, job_id: Uuid) -> Result<Option<StatsJobRow>, sqlx::Error>;

    /// Transition a job's status, setting timestamps appropriately:
    /// - `Running`   → sets `started_at = now()`
    /// - `Completed` → sets `completed_at = now()`
    /// - `Failed`    → sets `completed_at = now()`, stores `error`
    async fn update_job_status(
        &self,
        job_id: Uuid,
        status: StatsJobStatus,
        error: Option<String>,
    ) -> Result<StatsJobRow, sqlx::Error>;
}

// ---------------------------------------------------------------------------
// Postgres implementation
// ---------------------------------------------------------------------------

/// Postgres-backed implementation of [`StatsJobRepository`].
pub struct PgStatsJobRepository {
    pool: PgPool,
}

impl PgStatsJobRepository {
    /// Construct a new repository bound to `pool`.
    #[must_use]
    pub const fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl StatsJobRepository for PgStatsJobRepository {
    async fn create_job(&self, input: &CreateStatsJob) -> Result<StatsJobRow, sqlx::Error> {
        sqlx::query_as::<_, StatsJobRow>(
            r"
            INSERT INTO stats_jobs (id, experiment_id, status, created_at)
            VALUES (gen_random_uuid(), $1, 'pending', now())
            RETURNING
                id,
                experiment_id,
                status,
                created_at,
                started_at,
                completed_at,
                error
            ",
        )
        .bind(input.experiment_id)
        .fetch_one(&self.pool)
        .await
    }

    async fn get_job(&self, job_id: Uuid) -> Result<Option<StatsJobRow>, sqlx::Error> {
        sqlx::query_as::<_, StatsJobRow>(
            r"
            SELECT
                id,
                experiment_id,
                status,
                created_at,
                started_at,
                completed_at,
                error
            FROM stats_jobs
            WHERE id = $1
            ",
        )
        .bind(job_id)
        .fetch_optional(&self.pool)
        .await
    }

    async fn update_job_status(
        &self,
        job_id: Uuid,
        status: StatsJobStatus,
        error: Option<String>,
    ) -> Result<StatsJobRow, sqlx::Error> {
        sqlx::query_as::<_, StatsJobRow>(
            r"
            UPDATE stats_jobs
            SET
                status       = $2,
                started_at   = CASE WHEN $2 = 'running'   THEN now() ELSE started_at   END,
                completed_at = CASE WHEN $2 IN ('completed', 'failed') THEN now() ELSE completed_at END,
                error        = $3
            WHERE id = $1
            RETURNING
                id,
                experiment_id,
                status,
                created_at,
                started_at,
                completed_at,
                error
            ",
        )
        .bind(job_id)
        .bind(status)
        .bind(error)
        .fetch_one(&self.pool)
        .await
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Create a job and assert it starts in Pending status.
    ///
    /// NOTE: This test will fail at runtime with "relation `stats_jobs` does not
    /// exist" until the migration is added in Task 3 (Red phase of TDD).
    #[sqlx::test(migrations = "./migrations")]
    async fn test_create_job(pool: sqlx::PgPool) {
        let repo = PgStatsJobRepository::new(pool);
        let experiment_id = Uuid::new_v4();

        let job = repo
            .create_job(&CreateStatsJob { experiment_id })
            .await
            .expect("create_job should succeed");

        assert_eq!(job.experiment_id, Some(experiment_id));
        assert_eq!(job.status, StatsJobStatus::Pending);
        assert!(job.started_at.is_none());
        assert!(job.completed_at.is_none());
        assert!(job.error.is_none());
    }

    /// Create a job then fetch it by id; verify returned fields match.
    ///
    /// NOTE: This test will fail at runtime with "relation `stats_jobs` does not
    /// exist" until the migration is added in Task 3 (Red phase of TDD).
    #[sqlx::test(migrations = "./migrations")]
    async fn test_get_job(pool: sqlx::PgPool) {
        let repo = PgStatsJobRepository::new(pool);
        let experiment_id = Uuid::new_v4();

        let created = repo
            .create_job(&CreateStatsJob { experiment_id })
            .await
            .expect("create_job should succeed");

        let fetched = repo
            .get_job(created.id)
            .await
            .expect("get_job should not error")
            .expect("job should be found");

        assert_eq!(fetched.id, created.id);
        assert_eq!(fetched.experiment_id, Some(experiment_id));
        assert_eq!(fetched.status, StatsJobStatus::Pending);
    }

    /// Fetch a non-existent UUID; verify None is returned.
    ///
    /// NOTE: This test will fail at runtime with "relation `stats_jobs` does not
    /// exist" until the migration is added in Task 3 (Red phase of TDD).
    #[sqlx::test(migrations = "./migrations")]
    async fn test_get_job_not_found(pool: sqlx::PgPool) {
        let repo = PgStatsJobRepository::new(pool);

        let result = repo
            .get_job(Uuid::new_v4())
            .await
            .expect("get_job should not error for missing id");

        assert!(result.is_none());
    }

    /// Create a job then update it to Running; verify `started_at` is set.
    ///
    /// NOTE: This test will fail at runtime with "relation `stats_jobs` does not
    /// exist" until the migration is added in Task 3 (Red phase of TDD).
    #[sqlx::test(migrations = "./migrations")]
    async fn test_update_job_status_to_running(pool: sqlx::PgPool) {
        let repo = PgStatsJobRepository::new(pool);
        let experiment_id = Uuid::new_v4();

        let job = repo
            .create_job(&CreateStatsJob { experiment_id })
            .await
            .expect("create_job should succeed");

        let updated = repo
            .update_job_status(job.id, StatsJobStatus::Running, None)
            .await
            .expect("update_job_status should succeed");

        assert_eq!(updated.status, StatsJobStatus::Running);
        assert!(
            updated.started_at.is_some(),
            "started_at should be set when transitioning to Running"
        );
        assert!(updated.completed_at.is_none());
        assert!(updated.error.is_none());
    }

    /// Create → Running → Completed; verify `completed_at` is set.
    ///
    /// NOTE: This test will fail at runtime with "relation `stats_jobs` does not
    /// exist" until the migration is added in Task 3 (Red phase of TDD).
    #[sqlx::test(migrations = "./migrations")]
    async fn test_update_job_status_to_completed(pool: sqlx::PgPool) {
        let repo = PgStatsJobRepository::new(pool);
        let experiment_id = Uuid::new_v4();

        let job = repo
            .create_job(&CreateStatsJob { experiment_id })
            .await
            .expect("create_job should succeed");

        repo.update_job_status(job.id, StatsJobStatus::Running, None)
            .await
            .expect("transition to Running should succeed");

        let completed = repo
            .update_job_status(job.id, StatsJobStatus::Completed, None)
            .await
            .expect("transition to Completed should succeed");

        assert_eq!(completed.status, StatsJobStatus::Completed);
        assert!(completed.started_at.is_some());
        assert!(
            completed.completed_at.is_some(),
            "completed_at should be set when transitioning to Completed"
        );
        assert!(completed.error.is_none());
    }

    /// Create → Running → Failed with error message; verify error is stored.
    ///
    /// NOTE: This test will fail at runtime with "relation `stats_jobs` does not
    /// exist" until the migration is added in Task 3 (Red phase of TDD).
    #[sqlx::test(migrations = "./migrations")]
    async fn test_update_job_status_to_failed(pool: sqlx::PgPool) {
        let repo = PgStatsJobRepository::new(pool);
        let experiment_id = Uuid::new_v4();

        let job = repo
            .create_job(&CreateStatsJob { experiment_id })
            .await
            .expect("create_job should succeed");

        repo.update_job_status(job.id, StatsJobStatus::Running, None)
            .await
            .expect("transition to Running should succeed");

        let error_message = "connection timeout after 30s".to_string();
        let failed = repo
            .update_job_status(job.id, StatsJobStatus::Failed, Some(error_message.clone()))
            .await
            .expect("transition to Failed should succeed");

        assert_eq!(failed.status, StatsJobStatus::Failed);
        assert!(failed.started_at.is_some());
        assert!(
            failed.completed_at.is_some(),
            "completed_at should be set when transitioning to Failed"
        );
        assert_eq!(
            failed.error.as_deref(),
            Some(error_message.as_str()),
            "error message should be stored"
        );
    }
}
