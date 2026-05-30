//! Characterization tests for the `event_admin` gateway routes (`/v1/events`).
//!
//! These tests pin the CURRENT behavior of the `create_event` handler so that
//! the refactoring work in GL-xx (moving domain defaults into the analytics
//! service) has a green baseline to verify against.
//!
//! Specifically:
//! - `POST /v1/events` with `name` absent → gateway forwards `name = event_key`
//! - `POST /v1/events` with `name` present → gateway forwards the provided name
//!
//! Uses a mock `AnalyticsService` that records the `name` field passed to
//! `CreateEventDefinition` and returns a stub `EventDefinitionMsg`.

use std::sync::{Arc, Mutex};

use axum::body::Body;
use axum::http::{Request, StatusCode};
use tonic::transport::{Channel, Server};
use tonic::{Response, Status};
use tower::ServiceExt as _;

use stitchd_proto::analytics::v1::{
    CreateEventDefinitionRequest, CreateMetricRequest, DeleteEventDefinitionRequest,
    DeleteEventDefinitionResponse, DeleteMetricRequest, DeleteMetricResponse, EventDefinitionMsg,
    ExperimentResult, GetContextIntelligenceRequest, GetContextIntelligenceResponse,
    GetEvalStatsRequest, GetEvalStatsResponse, GetEventDefinitionRequest, GetEventFiringsRequest,
    GetEventFiringsResponse, GetEventStatsRequest, GetEventStatsResponse,
    GetExperimentResultRequest, GetMetricRequest, IngestEventRequest, IngestEventResponse,
    ListContextParamsRequest, ListContextParamsResponse, ListContextTypesRequest,
    ListContextTypesResponse, ListEventDefinitionsRequest, ListEventDefinitionsResponse,
    ListExperimentResultsRequest, ListMetricsRequest, ListMetricsResponse, MetricDefinition,
    PreviewMetricRequest, PreviewMetricResponse, RegisterContextRequest, RegisterContextResponse,
    TrackEventsRequest, TrackEventsResponse, UpdateEventDefinitionRequest, UpdateMetricRequest,
    WriteExperimentResultsRequest, WriteExperimentResultsResponse,
    analytics_service_client::AnalyticsServiceClient,
    analytics_service_server::{AnalyticsService, AnalyticsServiceServer},
};
use stitchd_proto::auth::v1::{
    auth_provider_service_client::AuthProviderServiceClient,
    auth_service_client::AuthServiceClient, oidc_login_service_client::OidcLoginServiceClient,
    saml_login_service_client::SamlLoginServiceClient,
};
use stitchd_proto::experiments::v1::experimentation_service_client::ExperimentationServiceClient;
use stitchd_proto::flags::v1::flag_service_client::FlagServiceClient;
use stitchd_proto::management::v1::management_service_client::ManagementServiceClient;
use stitchd_proto::segments::v1::segmentation_service_client::SegmentationServiceClient;
use stitchd_proto::stats::v1::stats_service_client::StatsServiceClient;

use stitchd_gateway::state::GatewayState;

// ─── Mock AnalyticsService ────────────────────────────────────────────────────

/// Captures the `name` field from each `CreateEventDefinition` call.
#[derive(Clone, Default)]
struct RecordingAnalyticsService {
    /// Names recorded from `CreateEventDefinition` requests (in call order).
    recorded_names: Arc<Mutex<Vec<String>>>,
}

type ExperimentResultsStream =
    tokio_stream::wrappers::ReceiverStream<Result<ExperimentResult, Status>>;

#[tonic::async_trait]
impl AnalyticsService for RecordingAnalyticsService {
    async fn create_event_definition(
        &self,
        req: tonic::Request<CreateEventDefinitionRequest>,
    ) -> Result<Response<EventDefinitionMsg>, Status> {
        let inner = req.into_inner();
        self.recorded_names.lock().unwrap().push(inner.name.clone());
        Ok(Response::new(EventDefinitionMsg {
            id: "evt-uuid".to_string(),
            environment_id: inner.environment_id,
            key: inner.key.clone(),
            name: inner.name,
            description: inner.description,
            value_type: inner.metric_type.clone(), // stub — matches test expectations
            metric_type: inner.metric_type,
            schema_json: String::new(),
            version: 1,
            created_at: "2026-01-01T00:00:00Z".to_string(),
            updated_at: "2026-01-01T00:00:00Z".to_string(),
            deleted_at: None,
            last_fired_at: None,
            count_24h: None,
        }))
    }

    // ── All other RPCs are unimplemented stubs ────────────────────────────────

    async fn ingest_event(
        &self,
        _req: tonic::Request<IngestEventRequest>,
    ) -> Result<Response<IngestEventResponse>, Status> {
        Err(Status::unimplemented("not used"))
    }

    async fn track_events(
        &self,
        _req: tonic::Request<TrackEventsRequest>,
    ) -> Result<Response<TrackEventsResponse>, Status> {
        Err(Status::unimplemented("not used"))
    }

    async fn register_context(
        &self,
        _req: tonic::Request<RegisterContextRequest>,
    ) -> Result<Response<RegisterContextResponse>, Status> {
        Err(Status::unimplemented("not used"))
    }

    async fn list_context_types(
        &self,
        _req: tonic::Request<ListContextTypesRequest>,
    ) -> Result<Response<ListContextTypesResponse>, Status> {
        Err(Status::unimplemented("not used"))
    }

    async fn list_context_params(
        &self,
        _req: tonic::Request<ListContextParamsRequest>,
    ) -> Result<Response<ListContextParamsResponse>, Status> {
        Err(Status::unimplemented("not used"))
    }

    async fn get_eval_stats(
        &self,
        _req: tonic::Request<GetEvalStatsRequest>,
    ) -> Result<Response<GetEvalStatsResponse>, Status> {
        Err(Status::unimplemented("not used"))
    }

    async fn get_context_intelligence(
        &self,
        _req: tonic::Request<GetContextIntelligenceRequest>,
    ) -> Result<Response<GetContextIntelligenceResponse>, Status> {
        Err(Status::unimplemented("not used"))
    }

    async fn write_experiment_results(
        &self,
        _req: tonic::Request<WriteExperimentResultsRequest>,
    ) -> Result<Response<WriteExperimentResultsResponse>, Status> {
        Err(Status::unimplemented("not used"))
    }

    type ListExperimentResultsStream = ExperimentResultsStream;

    async fn list_experiment_results(
        &self,
        _req: tonic::Request<ListExperimentResultsRequest>,
    ) -> Result<Response<Self::ListExperimentResultsStream>, Status> {
        Err(Status::unimplemented("not used"))
    }

    async fn get_experiment_result(
        &self,
        _req: tonic::Request<GetExperimentResultRequest>,
    ) -> Result<Response<ExperimentResult>, Status> {
        Err(Status::unimplemented("not used"))
    }

    async fn create_metric(
        &self,
        _req: tonic::Request<CreateMetricRequest>,
    ) -> Result<Response<MetricDefinition>, Status> {
        Err(Status::unimplemented("not used"))
    }

    async fn get_metric(
        &self,
        _req: tonic::Request<GetMetricRequest>,
    ) -> Result<Response<MetricDefinition>, Status> {
        Err(Status::unimplemented("not used"))
    }

    async fn list_metrics(
        &self,
        _req: tonic::Request<ListMetricsRequest>,
    ) -> Result<Response<ListMetricsResponse>, Status> {
        Err(Status::unimplemented("not used"))
    }

    async fn update_metric(
        &self,
        _req: tonic::Request<UpdateMetricRequest>,
    ) -> Result<Response<MetricDefinition>, Status> {
        Err(Status::unimplemented("not used"))
    }

    async fn delete_metric(
        &self,
        _req: tonic::Request<DeleteMetricRequest>,
    ) -> Result<Response<DeleteMetricResponse>, Status> {
        Err(Status::unimplemented("not used"))
    }

    async fn preview_metric(
        &self,
        _req: tonic::Request<PreviewMetricRequest>,
    ) -> Result<Response<PreviewMetricResponse>, Status> {
        Err(Status::unimplemented("not used"))
    }

    async fn get_event_firings(
        &self,
        _req: tonic::Request<GetEventFiringsRequest>,
    ) -> Result<Response<GetEventFiringsResponse>, Status> {
        Err(Status::unimplemented("not used"))
    }

    async fn get_event_stats(
        &self,
        _req: tonic::Request<GetEventStatsRequest>,
    ) -> Result<Response<GetEventStatsResponse>, Status> {
        Err(Status::unimplemented("not used"))
    }

    async fn get_event_definition(
        &self,
        _req: tonic::Request<GetEventDefinitionRequest>,
    ) -> Result<Response<EventDefinitionMsg>, Status> {
        Err(Status::unimplemented("not used"))
    }

    async fn list_event_definitions(
        &self,
        _req: tonic::Request<ListEventDefinitionsRequest>,
    ) -> Result<Response<ListEventDefinitionsResponse>, Status> {
        Err(Status::unimplemented("not used"))
    }

    async fn update_event_definition(
        &self,
        _req: tonic::Request<UpdateEventDefinitionRequest>,
    ) -> Result<Response<EventDefinitionMsg>, Status> {
        Err(Status::unimplemented("not used"))
    }

    async fn delete_event_definition(
        &self,
        _req: tonic::Request<DeleteEventDefinitionRequest>,
    ) -> Result<Response<DeleteEventDefinitionResponse>, Status> {
        Err(Status::unimplemented("not used"))
    }
}

// ─── Helpers ─────────────────────────────────────────────────────────────────

async fn spawn_mock(
    svc: RecordingAnalyticsService,
) -> (AnalyticsServiceClient<Channel>, Arc<Mutex<Vec<String>>>) {
    let names = Arc::clone(&svc.recorded_names);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        Server::builder()
            .add_service(AnalyticsServiceServer::new(svc))
            .serve_with_incoming(tokio_stream::wrappers::TcpListenerStream::new(listener))
            .await
            .unwrap();
    });
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    let client = AnalyticsServiceClient::connect(format!("http://{addr}"))
        .await
        .unwrap();
    (client, names)
}

fn make_state(analytics_client: AnalyticsServiceClient<Channel>) -> Arc<GatewayState> {
    let flag_channel = Channel::from_static("http://127.0.0.1:2").connect_lazy();
    let seg_channel = Channel::from_static("http://127.0.0.1:3").connect_lazy();
    Arc::new(GatewayState::from_channels(
        AuthServiceClient::new(Channel::from_static("http://127.0.0.1:1").connect_lazy()),
        FlagServiceClient::new(flag_channel.clone()),
        flag_channel,
        SegmentationServiceClient::new(seg_channel.clone()),
        seg_channel,
        analytics_client,
        ExperimentationServiceClient::new(
            Channel::from_static("http://127.0.0.1:5").connect_lazy(),
        ),
        ManagementServiceClient::new(Channel::from_static("http://127.0.0.1:6").connect_lazy()),
        AuthProviderServiceClient::new(Channel::from_static("http://127.0.0.1:7").connect_lazy()),
        OidcLoginServiceClient::new(Channel::from_static("http://127.0.0.1:8").connect_lazy()),
        SamlLoginServiceClient::new(Channel::from_static("http://127.0.0.1:9").connect_lazy()),
        StatsServiceClient::new(Channel::from_static("http://127.0.0.1:10").connect_lazy()),
    ))
}

fn build_router(state: Arc<GatewayState>) -> axum::Router {
    use axum::routing::get;
    use stitchd_gateway::routes::event_admin as ea;
    axum::Router::new()
        .route("/v1/events", get(ea::list_events).post(ea::create_event))
        .route(
            "/v1/events/{event_key}",
            get(ea::get_event)
                .patch(ea::update_event)
                .delete(ea::delete_event),
        )
        .with_state(state)
}

// ─── Characterization tests ───────────────────────────────────────────────────

/// When `name` is absent from the request body, the gateway defaults `name`
/// to `event_key` before calling `CreateEventDefinition`.
///
/// This pins the domain-logic default that currently lives in the gateway's
/// `create_event` handler (line: `let name = body.name.unwrap_or_else(|| body.event_key.clone())`).
/// After refactoring (GL-xx), this default will move into the analytics service.
/// The external API contract — "omitting name → name equals event_key" — must
/// survive the refactor unchanged.
#[tokio::test]
async fn create_event_without_name_defaults_name_to_event_key() {
    let svc = RecordingAnalyticsService::default();
    let (analytics_client, recorded_names) = spawn_mock(svc).await;
    let state = make_state(analytics_client);
    let app = build_router(state);

    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/events")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{
                        "environment_id": "env-uuid-1",
                        "event_key": "button_clicked",
                        "metric_type": "count"
                    }"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(
        resp.status(),
        StatusCode::CREATED,
        "expected 201 Created from mock analytics service"
    );

    let names = recorded_names.lock().unwrap();
    assert_eq!(
        names.len(),
        1,
        "expected exactly one CreateEventDefinition call"
    );
    assert_eq!(
        names[0], "button_clicked",
        "gateway must default name to event_key when name is absent; got {:?}",
        names[0]
    );
}

/// When `name` is present in the request body, the gateway forwards it
/// unchanged to `CreateEventDefinition`.
#[tokio::test]
async fn create_event_with_name_forwards_provided_name() {
    let svc = RecordingAnalyticsService::default();
    let (analytics_client, recorded_names) = spawn_mock(svc).await;
    let state = make_state(analytics_client);
    let app = build_router(state);

    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/events")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{
                        "environment_id": "env-uuid-1",
                        "event_key": "button_clicked",
                        "name": "Button Clicked",
                        "metric_type": "count"
                    }"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(
        resp.status(),
        StatusCode::CREATED,
        "expected 201 Created from mock analytics service"
    );

    let names = recorded_names.lock().unwrap();
    assert_eq!(
        names.len(),
        1,
        "expected exactly one CreateEventDefinition call"
    );
    assert_eq!(
        names[0], "Button Clicked",
        "gateway must forward the provided name unchanged; got {:?}",
        names[0]
    );
}
