use std::sync::Arc;
use tonic::{Request, Response, Status};

use stitchd_db::{ContextRegistryRepository, EventDefinitionRepository, SdkKeyRepository};
use stitchd_events::writer::EventWriter;
use stitchd_proto::analytics::v1::{
    ExperimentResult, GetContextIntelligenceRequest, GetContextIntelligenceResponse,
    GetEvalStatsRequest, GetEvalStatsResponse, GetExperimentResultRequest,
    IngestEventRequest, IngestEventResponse, ListContextParamsRequest,
    ListContextParamsResponse, ListContextTypesRequest, ListContextTypesResponse,
    ListExperimentResultsRequest, RegisterContextRequest, RegisterContextResponse,
    WriteExperimentResultsRequest, WriteExperimentResultsResponse,
    analytics_service_server::AnalyticsService,
};

use super::context_intel::handle_get_context_intelligence;
use super::context_registry::{
    handle_list_context_params, handle_list_context_types, handle_register_context,
};
use super::eval_stats::handle_get_eval_stats;
use super::event_ingestion::{EventIngestionState, handle_ingest_event};
use super::experiment_results::{
    ExperimentResultsRepository, ResultStream, handle_get_experiment_result,
    handle_list_experiment_results, handle_write_experiment_results,
};

pub struct ServiceState {
    pub pg_pool: Arc<sqlx::PgPool>,
    pub ch_client: Arc<clickhouse::Client>,
    pub event_def_repo: Arc<dyn EventDefinitionRepository>,
    pub sdk_key_repo: Arc<dyn SdkKeyRepository>,
    pub event_writer: EventWriter,
    pub context_registry: Arc<dyn ContextRegistryRepository>,
    /// Analytics-store backend for experiment results (Worker 3 provides the
    /// ClickHouse-backed implementation).
    pub experiment_results_repo: Arc<dyn ExperimentResultsRepository>,
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
        request: Request<IngestEventRequest>,
    ) -> Result<Response<IngestEventResponse>, Status> {
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
        request: Request<GetEvalStatsRequest>,
    ) -> Result<Response<GetEvalStatsResponse>, Status> {
        handle_get_eval_stats(&self.state.ch_client, request).await
    }

    async fn get_context_intelligence(
        &self,
        request: Request<GetContextIntelligenceRequest>,
    ) -> Result<Response<GetContextIntelligenceResponse>, Status> {
        handle_get_context_intelligence(&self.state.context_registry, request).await
    }

    async fn write_experiment_results(
        &self,
        request: Request<WriteExperimentResultsRequest>,
    ) -> Result<Response<WriteExperimentResultsResponse>, Status> {
        handle_write_experiment_results(&self.state.experiment_results_repo, request).await
    }

    type ListExperimentResultsStream = ResultStream;

    async fn list_experiment_results(
        &self,
        request: Request<ListExperimentResultsRequest>,
    ) -> Result<Response<Self::ListExperimentResultsStream>, Status> {
        handle_list_experiment_results(&self.state.experiment_results_repo, request).await
    }

    async fn get_experiment_result(
        &self,
        request: Request<GetExperimentResultRequest>,
    ) -> Result<Response<ExperimentResult>, Status> {
        handle_get_experiment_result(&self.state.experiment_results_repo, request).await
    }
}
