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

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::PgPool;

    #[sqlx::test(migrations = "../../crates/stitchd-db/migrations")]
    async fn trigger_recompute_invalid_uuid_returns_invalid_argument(pool: PgPool) {
        let svc = StatsServiceImpl::new(pool);
        let req = Request::new(TriggerRecomputeRequest {
            experiment_id: "not-a-uuid".to_string(),
        });
        let err = svc.trigger_recompute(req).await.unwrap_err();
        assert_eq!(err.code(), tonic::Code::InvalidArgument);
    }

    #[sqlx::test(migrations = "../../crates/stitchd-db/migrations")]
    async fn trigger_recompute_valid_uuid_returns_pending_job(pool: PgPool) {
        let svc = StatsServiceImpl::new(pool);
        let req = Request::new(TriggerRecomputeRequest {
            experiment_id: Uuid::new_v4().to_string(),
        });
        let resp = svc.trigger_recompute(req).await.unwrap().into_inner();
        assert_eq!(resp.status, "pending");
        assert!(!resp.job_id.is_empty());
        assert!(resp.created_at_ms > 0);
    }

    #[sqlx::test(migrations = "../../crates/stitchd-db/migrations")]
    async fn get_job_status_invalid_uuid_returns_invalid_argument(pool: PgPool) {
        let svc = StatsServiceImpl::new(pool);
        let req = Request::new(GetJobStatusRequest {
            job_id: "bad-uuid".to_string(),
        });
        let err = svc.get_job_status(req).await.unwrap_err();
        assert_eq!(err.code(), tonic::Code::InvalidArgument);
    }

    #[sqlx::test(migrations = "../../crates/stitchd-db/migrations")]
    async fn get_job_status_unknown_job_returns_not_found(pool: PgPool) {
        let svc = StatsServiceImpl::new(pool);
        let req = Request::new(GetJobStatusRequest {
            job_id: Uuid::new_v4().to_string(),
        });
        let err = svc.get_job_status(req).await.unwrap_err();
        assert_eq!(err.code(), tonic::Code::NotFound);
    }

    #[sqlx::test(migrations = "../../crates/stitchd-db/migrations")]
    async fn get_job_status_returns_status_for_existing_job(pool: PgPool) {
        let svc = StatsServiceImpl::new(pool);
        let exp_id = Uuid::new_v4().to_string();
        let trigger = svc
            .trigger_recompute(Request::new(TriggerRecomputeRequest {
                experiment_id: exp_id,
            }))
            .await
            .unwrap()
            .into_inner();

        let resp = svc
            .get_job_status(Request::new(GetJobStatusRequest {
                job_id: trigger.job_id,
            }))
            .await
            .unwrap()
            .into_inner();
        assert!(!resp.status.is_empty());
    }

    #[sqlx::test(migrations = "../../crates/stitchd-db/migrations")]
    async fn run_recompute_transitions_job_to_completed(pool: PgPool) {
        let exp_id = Uuid::new_v4();
        let job = crate::job_service::create_recompute_job(&pool, exp_id)
            .await
            .unwrap();
        run_recompute(pool.clone(), job.id, exp_id).await;
        let status_row = crate::job_service::get_job_status(&pool, job.id)
            .await
            .unwrap();
        assert!(
            format!("{:?}", status_row.status)
                .to_lowercase()
                .contains("completed")
        );
    }
}
