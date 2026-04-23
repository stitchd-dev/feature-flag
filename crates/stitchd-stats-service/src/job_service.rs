//! Job service: thin façade over `StatsJobRepository` for recompute job lifecycle.

use sqlx::PgPool;
use uuid::Uuid;

use stitchd_db::{
    CreateStatsJob, PgStatsJobRepository, StatsJobRepository, StatsJobRow, StatsJobStatus,
};

/// Create a new recompute job for the given experiment, returning the persisted row.
pub async fn create_recompute_job(
    pool: &PgPool,
    experiment_id: Uuid,
) -> Result<StatsJobRow, sqlx::Error> {
    let repo = PgStatsJobRepository::new(pool.clone());
    repo.create_job(&CreateStatsJob { experiment_id })
        .await
}

/// Fetch the current status of a job by its ID.
///
/// Returns `Err(sqlx::Error::RowNotFound)` when the job does not exist.
pub async fn get_job_status(pool: &PgPool, job_id: Uuid) -> Result<StatsJobRow, sqlx::Error> {
    let repo = PgStatsJobRepository::new(pool.clone());
    repo.get_job(job_id)
        .await?
        .ok_or(sqlx::Error::RowNotFound)
}

/// Transition a job to `running`.
pub async fn mark_running(pool: &PgPool, job_id: Uuid) -> Result<StatsJobRow, sqlx::Error> {
    let repo = PgStatsJobRepository::new(pool.clone());
    repo.update_job_status(job_id, StatsJobStatus::Running, None)
        .await
}

/// Transition a job to `completed`.
pub async fn mark_completed(pool: &PgPool, job_id: Uuid) -> Result<StatsJobRow, sqlx::Error> {
    let repo = PgStatsJobRepository::new(pool.clone());
    repo.update_job_status(job_id, StatsJobStatus::Completed, None)
        .await
}

/// Transition a job to `failed` with an error message.
pub async fn mark_failed(
    pool: &PgPool,
    job_id: Uuid,
    error: String,
) -> Result<StatsJobRow, sqlx::Error> {
    let repo = PgStatsJobRepository::new(pool.clone());
    repo.update_job_status(job_id, StatsJobStatus::Failed, Some(error))
        .await
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[sqlx::test(migrations = "../../crates/stitchd-db/migrations")]
    async fn test_create_recompute_job_returns_pending(pool: sqlx::PgPool) {
        let exp_id = Uuid::new_v4();
        let job = create_recompute_job(&pool, exp_id)
            .await
            .expect("create_recompute_job should succeed");

        assert_eq!(job.status, StatsJobStatus::Pending, "initial status must be pending");
        assert_eq!(job.experiment_id, Some(exp_id), "experiment_id must be set");
        assert!(job.started_at.is_none(), "started_at must be null initially");
        assert!(job.completed_at.is_none(), "completed_at must be null initially");
        assert!(job.error.is_none());
    }

    #[sqlx::test(migrations = "../../crates/stitchd-db/migrations")]
    async fn test_create_recompute_job_returns_unique_job_id(pool: sqlx::PgPool) {
        let exp_id = Uuid::new_v4();
        let job1 = create_recompute_job(&pool, exp_id).await.unwrap();
        let job2 = create_recompute_job(&pool, exp_id).await.unwrap();
        assert_ne!(job1.id, job2.id, "each job must have a unique id");
    }

    #[sqlx::test(migrations = "../../crates/stitchd-db/migrations")]
    async fn test_get_job_status_returns_current_status(pool: sqlx::PgPool) {
        let exp_id = Uuid::new_v4();
        let created = create_recompute_job(&pool, exp_id).await.unwrap();

        let fetched = get_job_status(&pool, created.id)
            .await
            .expect("get_job_status should succeed");

        assert_eq!(fetched.id, created.id);
        assert_eq!(fetched.status, StatsJobStatus::Pending);
    }

    #[sqlx::test(migrations = "../../crates/stitchd-db/migrations")]
    async fn test_get_job_status_reflects_running_transition(pool: sqlx::PgPool) {
        let exp_id = Uuid::new_v4();
        let created = create_recompute_job(&pool, exp_id).await.unwrap();
        mark_running(&pool, created.id).await.unwrap();

        let fetched = get_job_status(&pool, created.id).await.unwrap();
        assert_eq!(fetched.status, StatsJobStatus::Running);
        assert!(fetched.started_at.is_some(), "started_at must be set after running transition");
    }

    #[sqlx::test(migrations = "../../crates/stitchd-db/migrations")]
    async fn test_get_job_status_reflects_completed_transition(pool: sqlx::PgPool) {
        let exp_id = Uuid::new_v4();
        let created = create_recompute_job(&pool, exp_id).await.unwrap();
        mark_running(&pool, created.id).await.unwrap();
        mark_completed(&pool, created.id).await.unwrap();

        let fetched = get_job_status(&pool, created.id).await.unwrap();
        assert_eq!(fetched.status, StatsJobStatus::Completed);
        assert!(fetched.completed_at.is_some());
    }
}
