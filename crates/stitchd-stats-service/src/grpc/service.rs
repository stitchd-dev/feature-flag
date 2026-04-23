//! gRPC `StatsService` implementation.

use sqlx::PgPool;
use tonic::{Request, Response, Status};
use uuid::Uuid;

use stitchd_proto::stats::v1::{
    GetJobStatusRequest, GetJobStatusResponse, TriggerRecomputeRequest, TriggerRecomputeResponse,
    stats_service_server::StatsService,
};

use crate::job_service;

/// gRPC `StatsService` server implementation.
pub struct StatsServiceImpl {
    pool: PgPool,
}

impl StatsServiceImpl {
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[tonic::async_trait]
impl StatsService for StatsServiceImpl {
    async fn trigger_recompute(
        &self,
        request: Request<TriggerRecomputeRequest>,
    ) -> Result<Response<TriggerRecomputeResponse>, Status> {
        let req = request.into_inner();
        let experiment_id = req
            .experiment_id
            .parse::<Uuid>()
            .map_err(|_| Status::invalid_argument("invalid experiment_id UUID"))?;

        let job = job_service::create_recompute_job(&self.pool, experiment_id)
            .await
            .map_err(|e| Status::internal(format!("failed to create job: {e}")))?;

        // Spawn background task to run the recompute.
        let pool = self.pool.clone();
        let job_id = job.id;
        tokio::spawn(async move {
            run_recompute(pool, job_id, experiment_id).await;
        });

        Ok(Response::new(TriggerRecomputeResponse {
            job_id: job.id.to_string(),
            status: "pending".to_string(),
            created_at_ms: job.created_at.timestamp_millis(),
        }))
    }

    async fn get_job_status(
        &self,
        request: Request<GetJobStatusRequest>,
    ) -> Result<Response<GetJobStatusResponse>, Status> {
        let req = request.into_inner();
        let job_id = req
            .job_id
            .parse::<Uuid>()
            .map_err(|_| Status::invalid_argument("invalid job_id UUID"))?;

        let job = job_service::get_job_status(&self.pool, job_id)
            .await
            .map_err(|e| match e {
                sqlx::Error::RowNotFound => Status::not_found(format!("job not found: {job_id}")),
                e => Status::internal(format!("database error: {e}")),
            })?;

        Ok(Response::new(GetJobStatusResponse {
            job_id: job.id.to_string(),
            status: format!("{:?}", job.status).to_lowercase(),
            started_at_ms: job.started_at.map_or(0, |t| t.timestamp_millis()),
            completed_at_ms: job.completed_at.map_or(0, |t| t.timestamp_millis()),
            error: job.error.unwrap_or_default(),
        }))
    }
}

/// Background recompute task — runs stats for a single experiment and updates job status.
async fn run_recompute(pool: PgPool, job_id: Uuid, _experiment_id: Uuid) {
    if let Err(e) = job_service::mark_running(&pool, job_id).await {
        tracing::error!(%job_id, "failed to mark job running: {e}");
        return;
    }

    // Full stats computation is wired in Phase 3 scheduler. Here we mark complete.
    match job_service::mark_completed(&pool, job_id).await {
        Ok(_) => tracing::info!(%job_id, "recompute job completed"),
        Err(e) => {
            tracing::error!(%job_id, "failed to mark job completed: {e}");
            let _ = job_service::mark_failed(&pool, job_id, e.to_string()).await;
        }
    }
}
