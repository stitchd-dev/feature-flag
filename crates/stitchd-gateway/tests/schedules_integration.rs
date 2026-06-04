//! Integration tests for the gateway `/schedules` routes (flag_lifecycle Phase 5
//! Task 4). Spin up an in-process mock `ScheduleService`, point the gateway state
//! at it via `GatewayState::with_schedule_client`, and drive the route handlers
//! with `tower::ServiceExt::oneshot`.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt as _;
use tonic::transport::{Channel, Server};
use tonic::{Response, Status};
use tower::ServiceExt as _;

use stitchd_proto::analytics::v1::analytics_service_client::AnalyticsServiceClient;
use stitchd_proto::auth::v1::{
    auth_provider_service_client::AuthProviderServiceClient,
    auth_service_client::AuthServiceClient, oidc_login_service_client::OidcLoginServiceClient,
    saml_login_service_client::SamlLoginServiceClient,
};
use stitchd_proto::experiments::v1::experimentation_service_client::ExperimentationServiceClient;
use stitchd_proto::flags::v1::flag_service_client::FlagServiceClient;
use stitchd_proto::management::v1::management_service_client::ManagementServiceClient;
use stitchd_proto::schedule::v1::{
    CancelScheduledChangeRequest, CreateScheduledChangeRequest, GetScheduledChangeRequest,
    ListScheduledChangesRequest, ListScheduledChangesResponse, PauseScheduledChangeRequest,
    ResumeScheduledChangeRequest, ScheduleEntityType, ScheduleKind, ScheduleRunOutcome,
    ScheduleStatus, ScheduledChange, ScheduledChangeRun,
    schedule_service_client::ScheduleServiceClient,
    schedule_service_server::{ScheduleService, ScheduleServiceServer},
};
use stitchd_proto::segments::v1::segmentation_service_client::SegmentationServiceClient;
use stitchd_proto::stats::v1::stats_service_client::StatsServiceClient;

use stitchd_gateway::state::GatewayState;

// ─── Mock ScheduleService ─────────────────────────────────────────────────────

#[derive(Clone)]
struct MockScheduleService {
    /// Returned by every successful RPC.
    change: ScheduledChange,
    /// When set, RPCs return this status instead.
    err: Option<tonic::Code>,
}

impl MockScheduleService {
    fn ok(change: ScheduledChange) -> Self {
        Self { change, err: None }
    }
    fn failing(code: tonic::Code) -> Self {
        Self {
            change: ScheduledChange::default(),
            err: Some(code),
        }
    }
    fn maybe_err(&self) -> Result<(), Status> {
        match self.err {
            Some(code) => Err(Status::new(code, "mock error")),
            None => Ok(()),
        }
    }
}

#[tonic::async_trait]
impl ScheduleService for MockScheduleService {
    async fn create_scheduled_change(
        &self,
        _req: tonic::Request<CreateScheduledChangeRequest>,
    ) -> Result<Response<ScheduledChange>, Status> {
        self.maybe_err()?;
        Ok(Response::new(self.change.clone()))
    }
    async fn list_scheduled_changes(
        &self,
        _req: tonic::Request<ListScheduledChangesRequest>,
    ) -> Result<Response<ListScheduledChangesResponse>, Status> {
        self.maybe_err()?;
        Ok(Response::new(ListScheduledChangesResponse {
            changes: vec![self.change.clone()],
        }))
    }
    async fn get_scheduled_change(
        &self,
        _req: tonic::Request<GetScheduledChangeRequest>,
    ) -> Result<Response<ScheduledChange>, Status> {
        self.maybe_err()?;
        Ok(Response::new(self.change.clone()))
    }
    async fn cancel_scheduled_change(
        &self,
        _req: tonic::Request<CancelScheduledChangeRequest>,
    ) -> Result<Response<ScheduledChange>, Status> {
        self.maybe_err()?;
        Ok(Response::new(self.change.clone()))
    }
    async fn pause_scheduled_change(
        &self,
        _req: tonic::Request<PauseScheduledChangeRequest>,
    ) -> Result<Response<ScheduledChange>, Status> {
        self.maybe_err()?;
        Ok(Response::new(self.change.clone()))
    }
    async fn resume_scheduled_change(
        &self,
        _req: tonic::Request<ResumeScheduledChangeRequest>,
    ) -> Result<Response<ScheduledChange>, Status> {
        self.maybe_err()?;
        Ok(Response::new(self.change.clone()))
    }
    async fn list_due_changes(
        &self,
        _req: tonic::Request<stitchd_proto::schedule::v1::ListDueChangesRequest>,
    ) -> Result<Response<stitchd_proto::schedule::v1::ListDueChangesResponse>, Status> {
        Err(Status::unimplemented("not used"))
    }
}

fn sample_change() -> ScheduledChange {
    ScheduledChange {
        id: "33333333-3333-3333-3333-333333333333".to_string(),
        entity_type: ScheduleEntityType::Flag as i32,
        entity_id: "11111111-1111-1111-1111-111111111111".to_string(),
        env_id: "22222222-2222-2222-2222-222222222222".to_string(),
        mutation_payload_json: r#"{"kind":"update","enabled_override":false}"#.to_string(),
        schedule_kind: ScheduleKind::OneShot as i32,
        scheduled_at_ms: 1_700_000_000_000,
        rrule: String::new(),
        tz: String::new(),
        status: ScheduleStatus::Pending as i32,
        next_run_at_ms: 1_700_000_000_000,
        last_run_at_ms: 0,
        created_at_ms: 1_699_000_000_000,
        updated_at_ms: 1_699_000_000_000,
        version: 1,
        runs: vec![ScheduledChangeRun {
            id: "44444444-4444-4444-4444-444444444444".to_string(),
            fired_at_ms: 1_700_000_000_000,
            outcome: ScheduleRunOutcome::Skipped as i32,
            detail: "flag_locked_by_experiment:55555555".to_string(),
        }],
    }
}

async fn spawn_mock(svc: MockScheduleService) -> ScheduleServiceClient<Channel> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        Server::builder()
            .add_service(ScheduleServiceServer::new(svc))
            .serve_with_incoming(tokio_stream::wrappers::TcpListenerStream::new(listener))
            .await
            .unwrap();
    });
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    ScheduleServiceClient::connect(format!("http://{addr}"))
        .await
        .unwrap()
}

fn make_state(schedule_client: ScheduleServiceClient<Channel>) -> Arc<GatewayState> {
    let flag_channel = Channel::from_static("http://127.0.0.1:2").connect_lazy();
    let seg_channel = Channel::from_static("http://127.0.0.1:3").connect_lazy();
    let state = GatewayState::from_channels(
        AuthServiceClient::new(Channel::from_static("http://127.0.0.1:1").connect_lazy()),
        FlagServiceClient::new(flag_channel.clone()),
        flag_channel,
        SegmentationServiceClient::new(seg_channel.clone()),
        seg_channel,
        AnalyticsServiceClient::new(Channel::from_static("http://127.0.0.1:4").connect_lazy()),
        ExperimentationServiceClient::new(
            Channel::from_static("http://127.0.0.1:5").connect_lazy(),
        ),
        ManagementServiceClient::new(Channel::from_static("http://127.0.0.1:6").connect_lazy()),
        AuthProviderServiceClient::new(Channel::from_static("http://127.0.0.1:7").connect_lazy()),
        OidcLoginServiceClient::new(Channel::from_static("http://127.0.0.1:8").connect_lazy()),
        SamlLoginServiceClient::new(Channel::from_static("http://127.0.0.1:9").connect_lazy()),
        StatsServiceClient::new(Channel::from_static("http://127.0.0.1:10").connect_lazy()),
    )
    .with_schedule_client(schedule_client);
    Arc::new(state)
}

fn build_router(state: Arc<GatewayState>) -> axum::Router {
    use axum::routing::{get, post};
    use stitchd_gateway::routes::schedules as s;
    axum::Router::new()
        .route(
            "/v1/environments/{environment_id}/{entity_kind}/{entity_id}/schedules",
            post(s::create_schedule).get(s::list_schedules),
        )
        .route(
            "/v1/environments/{environment_id}/schedules/{schedule_id}",
            get(s::get_schedule),
        )
        .route(
            "/v1/environments/{environment_id}/schedules/{schedule_id}/cancel",
            post(s::cancel_schedule),
        )
        .route(
            "/v1/environments/{environment_id}/schedules/{schedule_id}/pause",
            post(s::pause_schedule),
        )
        .route(
            "/v1/environments/{environment_id}/schedules/{schedule_id}/resume",
            post(s::resume_schedule),
        )
        .with_state(state)
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn create_schedule_returns_201_and_json() {
    let client = spawn_mock(MockScheduleService::ok(sample_change())).await;
    let app = build_router(make_state(client));

    let body = serde_json::json!({
        "mutation_payload": {"kind": "update", "enabled_override": false},
        "schedule_kind": "one_shot",
        "scheduled_at_ms": 1_700_000_000_000_i64,
    });
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/environments/22222222-2222-2222-2222-222222222222/flags/11111111-1111-1111-1111-111111111111/schedules")
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(json["entity_type"], "flag");
    assert_eq!(json["status"], "pending");
    assert_eq!(json["schedule_kind"], "one_shot");
    // Run history round-trips with the skip outcome + sentinel detail.
    assert_eq!(json["runs"][0]["outcome"], "skipped");
}

#[tokio::test]
async fn create_schedule_rejects_unknown_entity_kind() {
    let client = spawn_mock(MockScheduleService::ok(sample_change())).await;
    let app = build_router(make_state(client));
    let body = serde_json::json!({
        "mutation_payload": {},
        "schedule_kind": "one_shot",
    });
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/environments/env/widgets/eid/schedules")
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn create_schedule_rejects_unknown_schedule_kind() {
    let client = spawn_mock(MockScheduleService::ok(sample_change())).await;
    let app = build_router(make_state(client));
    let body = serde_json::json!({ "mutation_payload": {}, "schedule_kind": "hourly" });
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/environments/env/experiments/eid/schedules")
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn list_schedules_returns_array() {
    let client = spawn_mock(MockScheduleService::ok(sample_change())).await;
    let app = build_router(make_state(client));
    let resp = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/v1/environments/env/segments/sid/schedules")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert!(json.is_array());
    assert_eq!(json.as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn get_schedule_returns_change_with_runs() {
    let client = spawn_mock(MockScheduleService::ok(sample_change())).await;
    let app = build_router(make_state(client));
    let resp = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/v1/environments/env/schedules/33333333-3333-3333-3333-333333333333")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(json["id"], "33333333-3333-3333-3333-333333333333");
    assert_eq!(json["runs"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn cancel_pause_resume_return_200() {
    for verb in ["cancel", "pause", "resume"] {
        let client = spawn_mock(MockScheduleService::ok(sample_change())).await;
        let app = build_router(make_state(client));
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!(
                        "/v1/environments/env/schedules/33333333-3333-3333-3333-333333333333/{verb}"
                    ))
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"version":1}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK, "verb {verb}");
    }
}

#[tokio::test]
async fn get_schedule_maps_not_found_to_404() {
    let client = spawn_mock(MockScheduleService::failing(tonic::Code::NotFound)).await;
    let app = build_router(make_state(client));
    let resp = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/v1/environments/env/schedules/missing")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn cancel_maps_version_conflict_to_409() {
    // schedule-service returns FAILED_PRECONDITION for an invalid-state cancel;
    // the gateway maps it to 409.
    let client = spawn_mock(MockScheduleService::failing(
        tonic::Code::FailedPrecondition,
    ))
    .await;
    let app = build_router(make_state(client));
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/environments/env/schedules/sid/cancel")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"version":1}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CONFLICT);
}
