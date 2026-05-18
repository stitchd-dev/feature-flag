use std::sync::Arc;
use tonic::{Request, Response, Status};

use stitchd_db::{ContextRegistryRepository, EventDefinitionRepository, SdkKeyRepository};
use stitchd_events::writer::EventWriter;
use stitchd_proto::analytics::v1::{
    GetContextIntelligenceRequest, GetContextIntelligenceResponse, GetEvalStatsRequest,
    GetEvalStatsResponse, ListContextParamsRequest, ListContextParamsResponse,
    ListContextTypesRequest, ListContextTypesResponse, RegisterContextRequest,
    RegisterContextResponse,
    analytics_service_server::AnalyticsService,
};
use stitchd_proto::events::v1::{IngestRequest, IngestResponse};

use super::context_registry::{
    handle_list_context_params, handle_list_context_types, handle_register_context,
};
use super::event_ingestion::{EventIngestionState, handle_ingest_event};

pub struct ServiceState {
    pub pg_pool: Arc<sqlx::PgPool>,
    pub ch_client: Arc<clickhouse::Client>,
    pub event_def_repo: Arc<dyn EventDefinitionRepository>,
    pub sdk_key_repo: Arc<dyn SdkKeyRepository>,
    pub event_writer: EventWriter,
    pub context_registry: Arc<dyn ContextRegistryRepository>,
}

pub struct AnalyticsServiceImpl {
    state: Arc<ServiceState>,
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
        request: Request<IngestRequest>,
    ) -> Result<Response<IngestResponse>, Status> {
        let ingestion_state = EventIngestionState {
            event_def_repo: Arc::clone(&self.state.event_def_repo),
            sdk_key_repo: Arc::clone(&self.state.sdk_key_repo),
            event_writer: self.state.event_writer.clone(),
        };
        handle_ingest_event(&ingestion_state, request).await
    }

    async fn register_context(
        &self,
        request: Request<RegisterContextRequest>,
    ) -> Result<Response<RegisterContextResponse>, Status> {
        handle_register_context(&self.state.context_registry, request).await
    }

    async fn list_context_types(
        &self,
        request: Request<ListContextTypesRequest>,
    ) -> Result<Response<ListContextTypesResponse>, Status> {
        handle_list_context_types(&self.state.context_registry, request).await
    }

    async fn list_context_params(
        &self,
        request: Request<ListContextParamsRequest>,
    ) -> Result<Response<ListContextParamsResponse>, Status> {
        handle_list_context_params(&self.state.context_registry, request).await
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
