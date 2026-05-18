//! AnalyticsServiceImpl — stub that will be filled in during Phases 2–4.

use std::sync::Arc;
use tonic::{Request, Response, Status};

use stitchd_proto::analytics::v1::{
    GetContextIntelligenceRequest, GetContextIntelligenceResponse, GetEvalStatsRequest,
    GetEvalStatsResponse, ListContextParamsRequest, ListContextParamsResponse,
    ListContextTypesRequest, ListContextTypesResponse, RegisterContextRequest,
    RegisterContextResponse,
    analytics_service_server::AnalyticsService,
};
use stitchd_proto::events::v1::{IngestRequest, IngestResponse};

pub struct ServiceState {
    pub pg_pool: Arc<sqlx::PgPool>,
    pub ch_client: Arc<clickhouse::Client>,
}

pub struct AnalyticsServiceImpl {
    pub state: Arc<ServiceState>,
}

impl AnalyticsServiceImpl {
    pub fn new(state: ServiceState) -> Self {
        Self {
            state: Arc::new(state),
        }
    }
}

#[tonic::async_trait]
impl AnalyticsService for AnalyticsServiceImpl {
    async fn ingest_event(
        &self,
        _request: Request<IngestRequest>,
    ) -> Result<Response<IngestResponse>, Status> {
        // Implemented in Phase 2.
        Err(Status::unimplemented("IngestEvent: Phase 2 pending"))
    }

    async fn register_context(
        &self,
        _request: Request<RegisterContextRequest>,
    ) -> Result<Response<RegisterContextResponse>, Status> {
        Err(Status::unimplemented("RegisterContext: Phase 3 pending"))
    }

    async fn list_context_types(
        &self,
        _request: Request<ListContextTypesRequest>,
    ) -> Result<Response<ListContextTypesResponse>, Status> {
        Err(Status::unimplemented("ListContextTypes: Phase 3 pending"))
    }

    async fn list_context_params(
        &self,
        _request: Request<ListContextParamsRequest>,
    ) -> Result<Response<ListContextParamsResponse>, Status> {
        Err(Status::unimplemented("ListContextParams: Phase 3 pending"))
    }

    async fn get_eval_stats(
        &self,
        _request: Request<GetEvalStatsRequest>,
    ) -> Result<Response<GetEvalStatsResponse>, Status> {
        Err(Status::unimplemented("GetEvalStats: Phase 4 pending"))
    }

    async fn get_context_intelligence(
        &self,
        _request: Request<GetContextIntelligenceRequest>,
    ) -> Result<Response<GetContextIntelligenceResponse>, Status> {
        Err(Status::unimplemented(
            "GetContextIntelligence: Phase 4 pending",
        ))
    }
}
