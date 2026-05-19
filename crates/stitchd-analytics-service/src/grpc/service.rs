use std::sync::Arc;
use tonic::{Request, Response, Status};

use stitchd_db::{ContextRegistryRepository, EventDefinitionRepository, SdkKeyRepository};
use stitchd_event_writer::writer::EventWriter;
use stitchd_proto::analytics::v1::{
    CreateMetricRequest, DeleteMetricRequest, DeleteMetricResponse, ExperimentResult,
    GetContextIntelligenceRequest, GetContextIntelligenceResponse, GetEvalStatsRequest,
    GetEvalStatsResponse, GetExperimentResultRequest, GetMetricRequest, IngestEventRequest,
    IngestEventResponse, ListContextParamsRequest, ListContextParamsResponse,
    ListContextTypesRequest, ListContextTypesResponse, ListExperimentResultsRequest,
    ListMetricsRequest, ListMetricsResponse, MetricDefinition, PreviewMetricRequest,
    PreviewMetricResponse, RegisterContextRequest, RegisterContextResponse, TrackEventsRequest,
    TrackEventsResponse, UpdateMetricRequest, WriteExperimentResultsRequest,
    WriteExperimentResultsResponse, analytics_service_server::AnalyticsService,
};

use super::context_intel::handle_get_context_intelligence;
use super::context_registry::{
    handle_list_context_params, handle_list_context_types, handle_register_context,
};
use super::eval_stats::handle_get_eval_stats;
use super::event_ingestion::{EventIngestionState, handle_ingest_event};
use super::experiment_results::{
    ResultStream, handle_get_experiment_result, handle_list_experiment_results,
    handle_write_experiment_results,
};
use super::ingestion::{EventDefinitionCache, TrackEventsState, handle_track_events};
use crate::repo::experiment_results::ExperimentResultsRepository;

pub struct ServiceState {
    pub pg_pool: Arc<sqlx::PgPool>,
    pub ch_client: Arc<clickhouse::Client>,
    pub event_def_repo: Arc<dyn EventDefinitionRepository>,
    pub sdk_key_repo: Arc<dyn SdkKeyRepository>,
    pub event_writer: EventWriter,
    pub context_registry: Arc<dyn ContextRegistryRepository>,
    /// ClickHouse-backed repository for computed experiment results.
    pub experiment_results_repo: Arc<dyn ExperimentResultsRepository>,
    /// 60-second in-process cache for event-definition validation lookups
    /// — backs the `TrackEvents` RPC hot path.
    pub event_def_cache: EventDefinitionCache,
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

    async fn track_events(
        &self,
        request: Request<TrackEventsRequest>,
    ) -> Result<Response<TrackEventsResponse>, Status> {
        let track_state = TrackEventsState {
            event_def_repo: Arc::clone(&self.state.event_def_repo),
            event_def_cache: self.state.event_def_cache.clone(),
            ch_client: Arc::clone(&self.state.ch_client),
        };
        handle_track_events(&track_state, request).await
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

    // -----------------------------------------------------------------------
    // Experiment Results RPCs
    // -----------------------------------------------------------------------

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

    // ── Metric definitions CRUD ─────────────────────────────────────────────
    //
    // These RPCs are wired in Phase 3 (`MetricService` implementation in
    // `crates/stitchd-analytics-service/src/grpc/metric.rs`). For now they
    // return `Unimplemented` so the proto-generated trait is satisfied and
    // the service binary compiles.

    async fn create_metric(
        &self,
        _request: Request<CreateMetricRequest>,
    ) -> Result<Response<MetricDefinition>, Status> {
        Err(Status::unimplemented(
            "create_metric — wired in events_metrics_20260519 Phase 3",
        ))
    }

    async fn get_metric(
        &self,
        _request: Request<GetMetricRequest>,
    ) -> Result<Response<MetricDefinition>, Status> {
        Err(Status::unimplemented(
            "get_metric — wired in events_metrics_20260519 Phase 3",
        ))
    }

    async fn list_metrics(
        &self,
        _request: Request<ListMetricsRequest>,
    ) -> Result<Response<ListMetricsResponse>, Status> {
        Err(Status::unimplemented(
            "list_metrics — wired in events_metrics_20260519 Phase 3",
        ))
    }

    async fn update_metric(
        &self,
        _request: Request<UpdateMetricRequest>,
    ) -> Result<Response<MetricDefinition>, Status> {
        Err(Status::unimplemented(
            "update_metric — wired in events_metrics_20260519 Phase 3",
        ))
    }

    async fn delete_metric(
        &self,
        _request: Request<DeleteMetricRequest>,
    ) -> Result<Response<DeleteMetricResponse>, Status> {
        Err(Status::unimplemented(
            "delete_metric — wired in events_metrics_20260519 Phase 3",
        ))
    }

    async fn preview_metric(
        &self,
        _request: Request<PreviewMetricRequest>,
    ) -> Result<Response<PreviewMetricResponse>, Status> {
        Err(Status::unimplemented(
            "preview_metric — wired in events_metrics_20260519 Phase 3",
        ))
    }
}
