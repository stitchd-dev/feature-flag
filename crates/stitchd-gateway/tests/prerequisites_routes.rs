//! Integration tests for the gateway prerequisite routes
//! (`flag_lifecycle_20260604` Phase 4 Task 4).
//!
//! Spins up an in-process mock `FlagService` and routes
//! `GET`/`PUT /v1/projects/{project_id}/flags/{flag_id}/prerequisites` through
//! the real gateway handlers via `tower::ServiceExt::oneshot`, asserting the
//! happy-path bodies and the structured `409 DEPENDENCY_EXISTS` mapping of the
//! `dependency_exists:<ids>` sentinel.

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
use stitchd_proto::flags::v1::{
    EvaluatePreviewRequest, EvaluatePreviewResponse, FeatureFlag, FlagPrerequisite,
    GetFlagDefinitionsRequest, GetFlagRequest, GetPrerequisitesRequest, GetPrerequisitesResponse,
    ListFlagsRequest, ListFlagsResponse, MutateFlagRequest, MutateFlagResponse,
    SetPrerequisitesRequest, SetPrerequisitesResponse, UpdateFlagHashingRequest,
    UpdateFlagHashingResponse,
    flag_service_client::FlagServiceClient,
    flag_service_server::{FlagService, FlagServiceServer},
};
use stitchd_proto::management::v1::management_service_client::ManagementServiceClient;
use stitchd_proto::segments::v1::segmentation_service_client::SegmentationServiceClient;
use stitchd_proto::stats::v1::stats_service_client::StatsServiceClient;

use stitchd_gateway::state::GatewayState;

fn build_router(state: Arc<GatewayState>) -> axum::Router {
    use axum::routing::get;
    use stitchd_gateway::routes::flags as flag_routes;
    axum::Router::new()
        .route(
            "/v1/projects/{project_id}/flags/{flag_id}/prerequisites",
            get(flag_routes::get_prerequisites).put(flag_routes::set_prerequisites),
        )
        .with_state(state)
}

// ─── Mock FlagService ───────────────────────────────────────────────────────

#[derive(Default, Clone)]
struct MockFlagService {
    /// When set, `set_prerequisites` fails with this sentinel status.
    set_error: Option<String>,
}

#[tonic::async_trait]
impl FlagService for MockFlagService {
    async fn get_flag(
        &self,
        _req: tonic::Request<GetFlagRequest>,
    ) -> Result<Response<FeatureFlag>, Status> {
        Ok(Response::new(FeatureFlag::default()))
    }
    async fn list_flags(
        &self,
        _req: tonic::Request<ListFlagsRequest>,
    ) -> Result<Response<ListFlagsResponse>, Status> {
        Ok(Response::new(ListFlagsResponse {
            flags: vec![],
            next_cursor: String::new(),
        }))
    }
    async fn mutate_flag(
        &self,
        _req: tonic::Request<MutateFlagRequest>,
    ) -> Result<Response<MutateFlagResponse>, Status> {
        Ok(Response::new(MutateFlagResponse {
            flag: None,
            version: 1,
        }))
    }
    async fn update_flag_hashing(
        &self,
        _req: tonic::Request<UpdateFlagHashingRequest>,
    ) -> Result<Response<UpdateFlagHashingResponse>, Status> {
        Err(Status::unimplemented("not used"))
    }
    async fn evaluate_preview(
        &self,
        _req: tonic::Request<EvaluatePreviewRequest>,
    ) -> Result<Response<EvaluatePreviewResponse>, Status> {
        Err(Status::unimplemented("not used"))
    }
    type GetFlagDefinitionsStream =
        tokio_stream::wrappers::ReceiverStream<Result<FeatureFlag, Status>>;
    async fn get_flag_definitions(
        &self,
        _req: tonic::Request<GetFlagDefinitionsRequest>,
    ) -> Result<Response<Self::GetFlagDefinitionsStream>, Status> {
        Err(Status::unimplemented("not used"))
    }
    async fn set_default_rule_distribution(
        &self,
        _req: tonic::Request<stitchd_proto::flags::v1::SetDefaultRuleDistributionRequest>,
    ) -> Result<Response<stitchd_proto::flags::v1::SetDefaultRuleDistributionResponse>, Status>
    {
        Err(Status::unimplemented("not used"))
    }

    async fn set_prerequisites(
        &self,
        _req: tonic::Request<SetPrerequisitesRequest>,
    ) -> Result<Response<SetPrerequisitesResponse>, Status> {
        if let Some(sentinel) = &self.set_error {
            return Err(Status::failed_precondition(sentinel.clone()));
        }
        Ok(Response::new(SetPrerequisitesResponse {
            flag: Some(FeatureFlag {
                key: "dependent".to_string(),
                prerequisites: vec![FlagPrerequisite {
                    prerequisite_flag_id: "11111111-1111-1111-1111-111111111111".to_string(),
                    prerequisite_flag_key: "prereq".to_string(),
                    required_variant_id: "22222222-2222-2222-2222-222222222222".to_string(),
                    required_variant_key: "on".to_string(),
                }],
                fallback_variant_key: "fb".to_string(),
                ..Default::default()
            }),
            version: 2,
        }))
    }

    async fn get_prerequisites(
        &self,
        _req: tonic::Request<GetPrerequisitesRequest>,
    ) -> Result<Response<GetPrerequisitesResponse>, Status> {
        Ok(Response::new(GetPrerequisitesResponse {
            prerequisites: vec![FlagPrerequisite {
                prerequisite_flag_id: "11111111-1111-1111-1111-111111111111".to_string(),
                prerequisite_flag_key: "prereq".to_string(),
                required_variant_id: "22222222-2222-2222-2222-222222222222".to_string(),
                required_variant_key: "on".to_string(),
            }],
            fallback_variant_key: "fb".to_string(),
        }))
    }
    async fn bandit_update_allocation(
        &self,
        _req: tonic::Request<stitchd_proto::flags::v1::BanditUpdateAllocationRequest>,
    ) -> Result<Response<stitchd_proto::flags::v1::BanditUpdateAllocationResponse>, Status> {
        Err(Status::unimplemented("not used"))
    }
}

async fn spawn_mock(svc: MockFlagService) -> FlagServiceClient<Channel> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        Server::builder()
            .add_service(FlagServiceServer::new(svc))
            .serve_with_incoming(tokio_stream::wrappers::TcpListenerStream::new(listener))
            .await
            .unwrap();
    });
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    FlagServiceClient::connect(format!("http://{addr}"))
        .await
        .unwrap()
}

fn make_state(flag_client: FlagServiceClient<Channel>) -> Arc<GatewayState> {
    let flag_channel = Channel::from_static("http://127.0.0.1:2").connect_lazy();
    let seg_channel = Channel::from_static("http://127.0.0.1:3").connect_lazy();
    Arc::new(GatewayState::from_channels(
        AuthServiceClient::new(Channel::from_static("http://127.0.0.1:1").connect_lazy()),
        flag_client,
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
    ))
}

// ─── Tests ──────────────────────────────────────────────────────────────────

#[tokio::test]
async fn get_prerequisites_returns_gate() {
    let client = spawn_mock(MockFlagService::default()).await;
    let router = build_router(make_state(client));

    let resp = router
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/v1/projects/p1/flags/dependent/prerequisites")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["fallback_variant_key"], "fb");
    assert_eq!(json["prerequisites"][0]["prerequisite_flag_key"], "prereq");
    assert_eq!(json["prerequisites"][0]["required_variant_key"], "on");
}

#[tokio::test]
async fn put_prerequisites_updates_gate() {
    let client = spawn_mock(MockFlagService::default()).await;
    let router = build_router(make_state(client));

    let body = serde_json::json!({
        "prerequisites": [
            { "prerequisite_flag_key": "prereq", "required_variant_key": "on" }
        ],
        "fallback_variant_key": "fb",
        "version": 1
    });
    let resp = router
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri("/v1/projects/p1/flags/dependent/prerequisites")
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let out = resp.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(json["prerequisites"][0]["prerequisite_flag_key"], "prereq");
    assert_eq!(json["fallback_variant_key"], "fb");
}

#[tokio::test]
async fn put_prerequisites_cycle_maps_to_400() {
    // The flag-service rejects a cycle with INVALID_ARGUMENT (carrying the cycle
    // path); the gateway maps that to HTTP 400.
    let cycle_client =
        spawn_mock_invalid_argument("prerequisite cycle detected: a -> b -> a").await;
    let router = build_router(make_state(cycle_client));

    let body = serde_json::json!({ "prerequisites": [], "fallback_variant_key": "", "version": 0 });
    let resp = router
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri("/v1/projects/p1/flags/a/prerequisites")
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn put_prerequisites_dependency_exists_maps_to_409() {
    // The delete-block sentinel is also surfaced on a FailedPrecondition; verify
    // the gateway decodes it to the structured 409 body when proxied.
    let client = spawn_mock(MockFlagService {
        set_error: Some("dependency_exists:33333333-3333-3333-3333-333333333333".to_string()),
    })
    .await;
    let router = build_router(make_state(client));

    let body = serde_json::json!({ "prerequisites": [], "fallback_variant_key": "", "version": 0 });
    let resp = router
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri("/v1/projects/p1/flags/a/prerequisites")
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CONFLICT);
    let out = resp.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(json["error"], "dependency_exists");
    assert_eq!(
        json["dependents"][0],
        "33333333-3333-3333-3333-333333333333"
    );
}

/// Spawn a mock whose `set_prerequisites` returns an `INVALID_ARGUMENT` status
/// (the gateway maps it to 400).
async fn spawn_mock_invalid_argument(message: &str) -> FlagServiceClient<Channel> {
    #[derive(Clone)]
    struct Invalid {
        message: String,
    }
    #[tonic::async_trait]
    impl FlagService for Invalid {
        async fn get_flag(
            &self,
            _req: tonic::Request<GetFlagRequest>,
        ) -> Result<Response<FeatureFlag>, Status> {
            Ok(Response::new(FeatureFlag::default()))
        }
        async fn list_flags(
            &self,
            _req: tonic::Request<ListFlagsRequest>,
        ) -> Result<Response<ListFlagsResponse>, Status> {
            Ok(Response::new(ListFlagsResponse {
                flags: vec![],
                next_cursor: String::new(),
            }))
        }
        async fn mutate_flag(
            &self,
            _req: tonic::Request<MutateFlagRequest>,
        ) -> Result<Response<MutateFlagResponse>, Status> {
            Err(Status::unimplemented("not used"))
        }
        async fn update_flag_hashing(
            &self,
            _req: tonic::Request<UpdateFlagHashingRequest>,
        ) -> Result<Response<UpdateFlagHashingResponse>, Status> {
            Err(Status::unimplemented("not used"))
        }
        async fn evaluate_preview(
            &self,
            _req: tonic::Request<EvaluatePreviewRequest>,
        ) -> Result<Response<EvaluatePreviewResponse>, Status> {
            Err(Status::unimplemented("not used"))
        }
        type GetFlagDefinitionsStream =
            tokio_stream::wrappers::ReceiverStream<Result<FeatureFlag, Status>>;
        async fn get_flag_definitions(
            &self,
            _req: tonic::Request<GetFlagDefinitionsRequest>,
        ) -> Result<Response<Self::GetFlagDefinitionsStream>, Status> {
            Err(Status::unimplemented("not used"))
        }
        async fn set_default_rule_distribution(
            &self,
            _req: tonic::Request<stitchd_proto::flags::v1::SetDefaultRuleDistributionRequest>,
        ) -> Result<Response<stitchd_proto::flags::v1::SetDefaultRuleDistributionResponse>, Status>
        {
            Err(Status::unimplemented("not used"))
        }
        async fn set_prerequisites(
            &self,
            _req: tonic::Request<SetPrerequisitesRequest>,
        ) -> Result<Response<SetPrerequisitesResponse>, Status> {
            Err(Status::invalid_argument(self.message.clone()))
        }
        async fn get_prerequisites(
            &self,
            _req: tonic::Request<GetPrerequisitesRequest>,
        ) -> Result<Response<GetPrerequisitesResponse>, Status> {
            Err(Status::unimplemented("not used"))
        }
        async fn bandit_update_allocation(
            &self,
            _req: tonic::Request<stitchd_proto::flags::v1::BanditUpdateAllocationRequest>,
        ) -> Result<Response<stitchd_proto::flags::v1::BanditUpdateAllocationResponse>, Status>
        {
            Err(Status::unimplemented("not used"))
        }
    }

    let svc = Invalid {
        message: message.to_string(),
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        Server::builder()
            .add_service(FlagServiceServer::new(svc))
            .serve_with_incoming(tokio_stream::wrappers::TcpListenerStream::new(listener))
            .await
            .unwrap();
    });
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    FlagServiceClient::connect(format!("http://{addr}"))
        .await
        .unwrap()
}
