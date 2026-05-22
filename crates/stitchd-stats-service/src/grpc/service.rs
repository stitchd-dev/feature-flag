//! gRPC `StatsService` implementation.

use std::sync::Arc;

use sqlx::PgPool;
use tonic::{Request, Response, Status};
use uuid::Uuid;

use stitchd_proto::stats::v1::{
    GetExperimentTimeseriesRequest, GetExperimentTimeseriesResponse, GetJobStatusRequest,
    GetJobStatusResponse, TimeseriesBucket, TriggerRecomputeRequest, TriggerRecomputeResponse,
    stats_service_server::StatsService,
};

use crate::job_service;
use crate::timeseries_reader::{TimeseriesReader, TimeseriesReaderError};

/// gRPC `StatsService` server implementation.
pub struct StatsServiceImpl {
    pool: PgPool,
    /// Optional timeseries reader. When `None`, `GetExperimentTimeseries`
    /// returns `Status::unimplemented` so the service can boot in
    /// stripped-down test configurations.
    timeseries_reader: Option<Arc<dyn TimeseriesReader>>,
}

impl StatsServiceImpl {
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self {
            pool,
            timeseries_reader: None,
        }
    }

    /// Attach a [`TimeseriesReader`] for the `GetExperimentTimeseries` RPC.
    #[must_use]
    pub fn with_timeseries_reader(mut self, reader: Arc<dyn TimeseriesReader>) -> Self {
        self.timeseries_reader = Some(reader);
        self
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

    async fn get_experiment_timeseries(
        &self,
        request: Request<GetExperimentTimeseriesRequest>,
    ) -> Result<Response<GetExperimentTimeseriesResponse>, Status> {
        let req = request.into_inner();

        let experiment_id = req
            .experiment_id
            .parse::<Uuid>()
            .map_err(|_| Status::invalid_argument("invalid experiment_id UUID"))?;
        let metric_id = req
            .metric_id
            .parse::<Uuid>()
            .map_err(|_| Status::invalid_argument("invalid metric_id UUID"))?;
        if req.context_type.is_empty() {
            return Err(Status::invalid_argument("context_type is required"));
        }

        // Clamp days to [1, 90]; default 7.
        let days = if req.days == 0 { 7 } else { req.days };
        let days = days.clamp(1, 90);

        let reader = self.timeseries_reader.as_ref().ok_or_else(|| {
            Status::unimplemented("timeseries_reader not configured on this service instance")
        })?;

        let rows = reader
            .get_timeseries(experiment_id, metric_id, &req.context_type, days)
            .await
            .map_err(timeseries_err_to_status)?;

        let buckets = rows
            .into_iter()
            .map(|r| TimeseriesBucket {
                day: r.day,
                variant_key: r.variant_key,
                value: r.value,
            })
            .collect();

        Ok(Response::new(GetExperimentTimeseriesResponse { buckets }))
    }
}

#[allow(clippy::result_large_err)]
fn timeseries_err_to_status(e: TimeseriesReaderError) -> Status {
    match e {
        TimeseriesReaderError::MetricNotFound(m) => Status::not_found(m),
        TimeseriesReaderError::IterationNotFound(m) => Status::not_found(m),
        TimeseriesReaderError::InvalidMetricKind(m) => Status::failed_precondition(m),
        TimeseriesReaderError::Clickhouse(m) | TimeseriesReaderError::Internal(m) => {
            Status::internal(m)
        }
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
        // Create the job directly to avoid spawning a background task (which can
        // hold pool connections past test teardown and break subsequent test setup).
        let exp_id = Uuid::new_v4();
        let job = crate::job_service::create_recompute_job(&pool, exp_id)
            .await
            .unwrap();

        let svc = StatsServiceImpl::new(pool);
        let resp = svc
            .get_job_status(Request::new(GetJobStatusRequest {
                job_id: job.id.to_string(),
            }))
            .await
            .unwrap()
            .into_inner();
        assert!(!resp.status.is_empty());
    }

    // ── GetExperimentTimeseries tests ────────────────────────────────────────

    use crate::timeseries_reader::testing::StubTimeseriesReader;
    use crate::timeseries_reader::{TimeseriesBucket as CoreBucket, TimeseriesReaderError};

    #[sqlx::test(migrations = "../../crates/stitchd-db/migrations")]
    async fn timeseries_returns_unimplemented_without_reader(pool: PgPool) {
        let svc = StatsServiceImpl::new(pool);
        let req = Request::new(GetExperimentTimeseriesRequest {
            experiment_id: Uuid::new_v4().to_string(),
            metric_id: Uuid::new_v4().to_string(),
            context_type: "user".to_string(),
            days: 7,
        });
        let err = svc.get_experiment_timeseries(req).await.unwrap_err();
        assert_eq!(err.code(), tonic::Code::Unimplemented);
    }

    #[sqlx::test(migrations = "../../crates/stitchd-db/migrations")]
    async fn timeseries_invalid_experiment_id_returns_invalid_argument(pool: PgPool) {
        let svc = StatsServiceImpl::new(pool)
            .with_timeseries_reader(Arc::new(StubTimeseriesReader::with_rows(vec![])));
        let req = Request::new(GetExperimentTimeseriesRequest {
            experiment_id: "not-a-uuid".to_string(),
            metric_id: Uuid::new_v4().to_string(),
            context_type: "user".to_string(),
            days: 7,
        });
        let err = svc.get_experiment_timeseries(req).await.unwrap_err();
        assert_eq!(err.code(), tonic::Code::InvalidArgument);
    }

    #[sqlx::test(migrations = "../../crates/stitchd-db/migrations")]
    async fn timeseries_empty_context_type_returns_invalid_argument(pool: PgPool) {
        let svc = StatsServiceImpl::new(pool)
            .with_timeseries_reader(Arc::new(StubTimeseriesReader::with_rows(vec![])));
        let req = Request::new(GetExperimentTimeseriesRequest {
            experiment_id: Uuid::new_v4().to_string(),
            metric_id: Uuid::new_v4().to_string(),
            context_type: String::new(),
            days: 7,
        });
        let err = svc.get_experiment_timeseries(req).await.unwrap_err();
        assert_eq!(err.code(), tonic::Code::InvalidArgument);
    }

    #[sqlx::test(migrations = "../../crates/stitchd-db/migrations")]
    async fn timeseries_clamps_days_to_valid_range(pool: PgPool) {
        let reader = StubTimeseriesReader::with_rows(vec![]);
        let calls_handle = reader.calls.clone();
        let svc = StatsServiceImpl::new(pool).with_timeseries_reader(Arc::new(reader));
        // Pass 0 → default to 7.
        let req = Request::new(GetExperimentTimeseriesRequest {
            experiment_id: Uuid::new_v4().to_string(),
            metric_id: Uuid::new_v4().to_string(),
            context_type: "user".to_string(),
            days: 0,
        });
        svc.get_experiment_timeseries(req).await.unwrap();
        let calls = calls_handle.lock().unwrap();
        assert_eq!(calls.last().map(|c| c.3), Some(7));
    }

    #[sqlx::test(migrations = "../../crates/stitchd-db/migrations")]
    async fn timeseries_clamps_days_above_90(pool: PgPool) {
        let reader = StubTimeseriesReader::with_rows(vec![]);
        let calls_handle = reader.calls.clone();
        let svc = StatsServiceImpl::new(pool).with_timeseries_reader(Arc::new(reader));
        let req = Request::new(GetExperimentTimeseriesRequest {
            experiment_id: Uuid::new_v4().to_string(),
            metric_id: Uuid::new_v4().to_string(),
            context_type: "user".to_string(),
            days: 365,
        });
        svc.get_experiment_timeseries(req).await.unwrap();
        let calls = calls_handle.lock().unwrap();
        assert_eq!(calls.last().map(|c| c.3), Some(90));
    }

    #[sqlx::test(migrations = "../../crates/stitchd-db/migrations")]
    async fn timeseries_returns_buckets(pool: PgPool) {
        let bucket = CoreBucket {
            day: "2026-05-21T00:00:00Z".to_string(),
            variant_key: "treatment".to_string(),
            value: 12.5,
        };
        let svc = StatsServiceImpl::new(pool)
            .with_timeseries_reader(Arc::new(StubTimeseriesReader::with_rows(vec![bucket])));
        let req = Request::new(GetExperimentTimeseriesRequest {
            experiment_id: Uuid::new_v4().to_string(),
            metric_id: Uuid::new_v4().to_string(),
            context_type: "user".to_string(),
            days: 14,
        });
        let resp = svc
            .get_experiment_timeseries(req)
            .await
            .unwrap()
            .into_inner();
        assert_eq!(resp.buckets.len(), 1);
        assert_eq!(resp.buckets[0].variant_key, "treatment");
        assert!((resp.buckets[0].value - 12.5).abs() < 1e-9);
    }

    #[sqlx::test(migrations = "../../crates/stitchd-db/migrations")]
    async fn timeseries_metric_not_found_returns_not_found(pool: PgPool) {
        let svc = StatsServiceImpl::new(pool).with_timeseries_reader(Arc::new(
            StubTimeseriesReader::with_err(TimeseriesReaderError::MetricNotFound("xxx".into())),
        ));
        let req = Request::new(GetExperimentTimeseriesRequest {
            experiment_id: Uuid::new_v4().to_string(),
            metric_id: Uuid::new_v4().to_string(),
            context_type: "user".to_string(),
            days: 7,
        });
        let err = svc.get_experiment_timeseries(req).await.unwrap_err();
        assert_eq!(err.code(), tonic::Code::NotFound);
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
