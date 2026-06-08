//! Integration test for the whole-flag lock 409 path.
//!
//! Spins up an in-process mock `FlagService` that returns the
//! `flag_locked_by_experiment:<exp_id>` sentinel as a `FailedPrecondition`
//! status; routes a `PUT /v1/projects/{project_id}/flags/{flag_id}` through
//! the gateway router and asserts the response is HTTP 409 with the
//! structured error body the spec mandates.
//!
//! Also verifies the success path: when the mock returns a plain `Ok`
//! response, the same PUT succeeds with 200.

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
    EvaluatePreviewRequest, EvaluatePreviewResponse, FeatureFlag, GetFlagDefinitionsRequest,
    GetFlagRequest, ListFlagsRequest, ListFlagsResponse, MutateFlagRequest, MutateFlagResponse,
    UpdateFlagHashingRequest, UpdateFlagHashingResponse,
    flag_service_client::FlagServiceClient,
    flag_service_server::{FlagService, FlagServiceServer},
};
use stitchd_proto::management::v1::management_service_client::ManagementServiceClient;
use stitchd_proto::segments::v1::segmentation_service_client::SegmentationServiceClient;
use stitchd_proto::stats::v1::stats_service_client::StatsServiceClient;

use stitchd_gateway::state::GatewayState;

// Build a minimal axum router exercising the flag CRUD endpoints. The
// in-crate `test_router` is gated on `#[cfg(test)]` and isn't visible to
// integration tests, so we rebuild a focused router here against the same
// handlers.
fn build_router(state: Arc<GatewayState>) -> axum::Router {
    use axum::routing::{post, put};
    use stitchd_gateway::routes::flags as flag_routes;
    axum::Router::new()
        .route(
            "/v1/projects/{project_id}/flags/{flag_id}",
            put(flag_routes::update_flag).delete(flag_routes::delete_flag),
        )
        .route(
            "/v1/projects/{project_id}/flags/{flag_id}/variants",
            put(flag_routes::update_variants),
        )
        .route(
            "/v1/projects/{project_id}/flags/{flag_id}/rules",
            put(flag_routes::update_rules),
        )
        .route(
            "/v1/projects/{project_id}/flags/{flag_id}/archive",
            post(flag_routes::archive_flag),
        )
        .route(
            "/v1/projects/{project_id}/flags/{flag_id}/default-rule-distribution",
            post(flag_routes::set_default_rule_distribution),
        )
        .with_state(state)
}

// ─── Mock FlagService ─────────────────────────────────────────────────────────

#[derive(Default, Clone)]
struct LockedFlagService {
    /// Sentinel experiment ID — emitted via the `flag_locked_by_experiment:`
    /// prefix from `mutate_flag` so the gateway can rebuild the 409 body.
    locked_experiment_id: String,
    /// Override flag returned by `get_flag`.
    flag: Option<FeatureFlag>,
}

#[tonic::async_trait]
impl FlagService for LockedFlagService {
    async fn get_flag(
        &self,
        _req: tonic::Request<GetFlagRequest>,
    ) -> Result<Response<FeatureFlag>, Status> {
        Ok(Response::new(self.flag.clone().unwrap_or_else(|| {
            FeatureFlag {
                key: "my-flag".to_string(),
                ..Default::default()
            }
        })))
    }

    async fn list_flags(
        &self,
        _req: tonic::Request<ListFlagsRequest>,
    ) -> Result<Response<ListFlagsResponse>, Status> {
        Ok(Response::new(ListFlagsResponse {
            flags: vec![],
            total: 0,
        }))
    }

    async fn mutate_flag(
        &self,
        _req: tonic::Request<MutateFlagRequest>,
    ) -> Result<Response<MutateFlagResponse>, Status> {
        // Emit the same sentinel the real flag-service produces when
        // `FlagServiceError::FlagLocked` is mapped via the `From` impl.
        Err(Status::failed_precondition(format!(
            "flag_locked_by_experiment:{}",
            self.locked_experiment_id
        )))
    }

    async fn update_flag_hashing(
        &self,
        _req: tonic::Request<UpdateFlagHashingRequest>,
    ) -> Result<Response<UpdateFlagHashingResponse>, Status> {
        Err(Status::failed_precondition(format!(
            "flag_locked_by_experiment:{}",
            self.locked_experiment_id
        )))
    }

    async fn evaluate_preview(
        &self,
        _req: tonic::Request<EvaluatePreviewRequest>,
    ) -> Result<Response<EvaluatePreviewResponse>, Status> {
        Err(Status::unimplemented("not used in flag-lock tests"))
    }

    type GetFlagDefinitionsStream =
        tokio_stream::wrappers::ReceiverStream<Result<FeatureFlag, Status>>;

    async fn get_flag_definitions(
        &self,
        _req: tonic::Request<GetFlagDefinitionsRequest>,
    ) -> Result<Response<Self::GetFlagDefinitionsStream>, Status> {
        Err(Status::unimplemented("not used in flag-lock tests"))
    }

    async fn set_default_rule_distribution(
        &self,
        _req: tonic::Request<stitchd_proto::flags::v1::SetDefaultRuleDistributionRequest>,
    ) -> Result<Response<stitchd_proto::flags::v1::SetDefaultRuleDistributionResponse>, Status>
    {
        // Surface the locked sentinel — the integration test asserts the
        // 409 mapping for this RPC the same way it does for mutate_flag.
        Err(Status::failed_precondition(format!(
            "flag_locked_by_experiment:{}",
            self.locked_experiment_id
        )))
    }
    async fn set_prerequisites(
        &self,
        _req: tonic::Request<stitchd_proto::flags::v1::SetPrerequisitesRequest>,
    ) -> Result<Response<stitchd_proto::flags::v1::SetPrerequisitesResponse>, Status> {
        Err(Status::unimplemented("not used"))
    }
    async fn get_prerequisites(
        &self,
        _req: tonic::Request<stitchd_proto::flags::v1::GetPrerequisitesRequest>,
    ) -> Result<Response<stitchd_proto::flags::v1::GetPrerequisitesResponse>, Status> {
        Err(Status::unimplemented("not used"))
    }
}

async fn spawn_mock_flag_service(svc: LockedFlagService) -> FlagServiceClient<Channel> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        Server::builder()
            .add_service(FlagServiceServer::new(svc))
            .serve_with_incoming(tokio_stream::wrappers::TcpListenerStream::new(listener))
            .await
            .unwrap();
    });
    // Give the server a moment to bind. Connect waits on the listener so
    // this is mainly defensive.
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    FlagServiceClient::connect(format!("http://{addr}"))
        .await
        .unwrap()
}

fn make_state(flag_client: FlagServiceClient<Channel>) -> Arc<GatewayState> {
    let flag_channel = Channel::from_static("http://127.0.0.1:2").connect_lazy();
    let seg_channel = Channel::from_static("http://127.0.0.1:3").connect_lazy();
    let state = GatewayState::from_channels(
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
    );
    Arc::new(state)
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn put_flag_returns_409_when_flag_locked_by_experiment() {
    let exp_id = "11111111-1111-1111-1111-111111111111";
    let svc = LockedFlagService {
        locked_experiment_id: exp_id.to_string(),
        flag: None,
    };
    let flag_client = spawn_mock_flag_service(svc).await;
    let state = make_state(flag_client);
    let app = build_router(state);

    let resp = app
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri("/v1/projects/env-1/flags/my-flag")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"name":"renamed","enabled":true,"version":1}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(
        resp.status(),
        StatusCode::CONFLICT,
        "expected 409 when flag is locked; got {}",
        resp.status()
    );
    let body_bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let body: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
    assert_eq!(body["error"], "flag_locked_by_experiment");
    assert_eq!(body["experiment_id"], exp_id);
    assert!(
        body["message"]
            .as_str()
            .map(|m| m.contains(exp_id))
            .unwrap_or(false),
        "expected human-readable message to mention the experiment id; body={body}"
    );
}

#[tokio::test]
async fn delete_flag_returns_409_when_flag_locked_by_experiment() {
    let exp_id = "22222222-2222-2222-2222-222222222222";
    let svc = LockedFlagService {
        locked_experiment_id: exp_id.to_string(),
        flag: None,
    };
    let flag_client = spawn_mock_flag_service(svc).await;
    let state = make_state(flag_client);
    let app = build_router(state);

    let resp = app
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri("/v1/projects/env-1/flags/my-flag")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::CONFLICT);
    let body_bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let body: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
    assert_eq!(body["error"], "flag_locked_by_experiment");
    assert_eq!(body["experiment_id"], exp_id);
}

#[tokio::test]
async fn variant_replace_returns_409_when_flag_locked_by_experiment() {
    let exp_id = "33333333-3333-3333-3333-333333333333";
    // The variants handler first calls get_flag (which succeeds) and then
    // mutate_flag (which fails with the lock sentinel).
    let svc = LockedFlagService {
        locked_experiment_id: exp_id.to_string(),
        flag: Some(FeatureFlag {
            key: "my-flag".to_string(),
            enabled: true,
            value_type: stitchd_proto::flags::v1::FlagValueType::String as i32,
            ..Default::default()
        }),
    };
    let flag_client = spawn_mock_flag_service(svc).await;
    let state = make_state(flag_client);
    let app = build_router(state);

    let resp = app
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri("/v1/projects/env-1/flags/my-flag/variants")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"variants":[{"key":"on","value":"yes"},{"key":"off","value":"no"}],"version":1}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::CONFLICT);
}

// ─── Success path — unlocked flag accepts the mutation ────────────────────────
//
// A separate mock service that returns Ok on `mutate_flag` shows the
// non-locked path still works.

#[derive(Default, Clone)]
struct UnlockedFlagService;

#[tonic::async_trait]
impl FlagService for UnlockedFlagService {
    async fn get_flag(
        &self,
        _req: tonic::Request<GetFlagRequest>,
    ) -> Result<Response<FeatureFlag>, Status> {
        Ok(Response::new(FeatureFlag {
            key: "my-flag".to_string(),
            enabled: true,
            ..Default::default()
        }))
    }
    async fn list_flags(
        &self,
        _req: tonic::Request<ListFlagsRequest>,
    ) -> Result<Response<ListFlagsResponse>, Status> {
        Ok(Response::new(ListFlagsResponse {
            flags: vec![],
            total: 0,
        }))
    }
    async fn mutate_flag(
        &self,
        _req: tonic::Request<MutateFlagRequest>,
    ) -> Result<Response<MutateFlagResponse>, Status> {
        Ok(Response::new(MutateFlagResponse {
            flag: Some(FeatureFlag {
                key: "my-flag".to_string(),
                enabled: true,
                ..Default::default()
            }),
            version: 2,
        }))
    }
    async fn update_flag_hashing(
        &self,
        _req: tonic::Request<UpdateFlagHashingRequest>,
    ) -> Result<Response<UpdateFlagHashingResponse>, Status> {
        Err(Status::unimplemented("not exercised here"))
    }
    async fn evaluate_preview(
        &self,
        _req: tonic::Request<EvaluatePreviewRequest>,
    ) -> Result<Response<EvaluatePreviewResponse>, Status> {
        Err(Status::unimplemented("not exercised here"))
    }
    type GetFlagDefinitionsStream =
        tokio_stream::wrappers::ReceiverStream<Result<FeatureFlag, Status>>;

    async fn get_flag_definitions(
        &self,
        _req: tonic::Request<GetFlagDefinitionsRequest>,
    ) -> Result<Response<Self::GetFlagDefinitionsStream>, Status> {
        Err(Status::unimplemented("not exercised here"))
    }
    async fn set_default_rule_distribution(
        &self,
        _req: tonic::Request<stitchd_proto::flags::v1::SetDefaultRuleDistributionRequest>,
    ) -> Result<Response<stitchd_proto::flags::v1::SetDefaultRuleDistributionResponse>, Status>
    {
        Ok(Response::new(
            stitchd_proto::flags::v1::SetDefaultRuleDistributionResponse {
                flag: Some(FeatureFlag {
                    key: "my-flag".to_string(),
                    enabled: true,
                    ..Default::default()
                }),
                version: 2,
            },
        ))
    }
    async fn set_prerequisites(
        &self,
        _req: tonic::Request<stitchd_proto::flags::v1::SetPrerequisitesRequest>,
    ) -> Result<Response<stitchd_proto::flags::v1::SetPrerequisitesResponse>, Status> {
        Err(Status::unimplemented("not used"))
    }
    async fn get_prerequisites(
        &self,
        _req: tonic::Request<stitchd_proto::flags::v1::GetPrerequisitesRequest>,
    ) -> Result<Response<stitchd_proto::flags::v1::GetPrerequisitesResponse>, Status> {
        Err(Status::unimplemented("not used"))
    }
}

#[tokio::test]
async fn put_flag_succeeds_when_flag_not_locked() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        Server::builder()
            .add_service(FlagServiceServer::new(UnlockedFlagService))
            .serve_with_incoming(tokio_stream::wrappers::TcpListenerStream::new(listener))
            .await
            .unwrap();
    });
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    let flag_client = FlagServiceClient::connect(format!("http://{addr}"))
        .await
        .unwrap();
    let state = make_state(flag_client);
    let app = build_router(state);

    let resp = app
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri("/v1/projects/env-1/flags/my-flag")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"name":"renamed","enabled":true,"version":1}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "expected 200 when flag is not locked; got {}",
        resp.status()
    );
}

// ─── Default-rule distribution (Phase 7 Task 5) ──────────────────────────────

#[tokio::test]
async fn set_default_rule_distribution_returns_409_when_flag_locked() {
    let exp_id = "44444444-4444-4444-4444-444444444444";
    let svc = LockedFlagService {
        locked_experiment_id: exp_id.to_string(),
        flag: None,
    };
    let flag_client = spawn_mock_flag_service(svc).await;
    let state = make_state(flag_client);
    let app = build_router(state);

    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/projects/proj-1/flags/my-flag/default-rule-distribution")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"distribution":{"allocations":[{"variant_key":"on","percentage":100.0}]},"version":1}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CONFLICT);
    let body_bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let body: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
    assert_eq!(body["error"], "flag_locked_by_experiment");
    assert_eq!(body["experiment_id"], exp_id);
}

#[tokio::test]
async fn set_default_rule_distribution_succeeds_when_unlocked() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        Server::builder()
            .add_service(FlagServiceServer::new(UnlockedFlagService))
            .serve_with_incoming(tokio_stream::wrappers::TcpListenerStream::new(listener))
            .await
            .unwrap();
    });
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    let flag_client = FlagServiceClient::connect(format!("http://{addr}"))
        .await
        .unwrap();
    let state = make_state(flag_client);
    let app = build_router(state);

    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/projects/proj-1/flags/my-flag/default-rule-distribution")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"distribution":{"allocations":[{"variant_key":"on","percentage":60.0},{"variant_key":"off","percentage":40.0}]},"version":1}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body_bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let body: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
    assert_eq!(body["version"], 2);
}

/// Mock that returns the `invalid_distribution:` sentinel so the gateway can
/// rewrite it into a 422 with structured body.
#[derive(Default, Clone)]
struct InvalidDistFlagService;

#[tonic::async_trait]
impl FlagService for InvalidDistFlagService {
    async fn get_flag(
        &self,
        _req: tonic::Request<GetFlagRequest>,
    ) -> Result<Response<FeatureFlag>, Status> {
        Ok(Response::new(FeatureFlag {
            key: "my-flag".to_string(),
            ..Default::default()
        }))
    }
    async fn list_flags(
        &self,
        _req: tonic::Request<ListFlagsRequest>,
    ) -> Result<Response<ListFlagsResponse>, Status> {
        Ok(Response::new(ListFlagsResponse {
            flags: vec![],
            total: 0,
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
        Err(Status::invalid_argument(
            "invalid_distribution: allocations must sum to 100",
        ))
    }
    async fn set_prerequisites(
        &self,
        _req: tonic::Request<stitchd_proto::flags::v1::SetPrerequisitesRequest>,
    ) -> Result<Response<stitchd_proto::flags::v1::SetPrerequisitesResponse>, Status> {
        Err(Status::unimplemented("not used"))
    }
    async fn get_prerequisites(
        &self,
        _req: tonic::Request<stitchd_proto::flags::v1::GetPrerequisitesRequest>,
    ) -> Result<Response<stitchd_proto::flags::v1::GetPrerequisitesResponse>, Status> {
        Err(Status::unimplemented("not used"))
    }
}

#[tokio::test]
async fn set_default_rule_distribution_returns_422_for_invalid_payload() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        Server::builder()
            .add_service(FlagServiceServer::new(InvalidDistFlagService))
            .serve_with_incoming(tokio_stream::wrappers::TcpListenerStream::new(listener))
            .await
            .unwrap();
    });
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    let flag_client = FlagServiceClient::connect(format!("http://{addr}"))
        .await
        .unwrap();
    let state = make_state(flag_client);
    let app = build_router(state);

    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/projects/proj-1/flags/my-flag/default-rule-distribution")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"distribution":{"allocations":[{"variant_key":"on","percentage":33.0}]},"version":1}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let body_bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let body: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
    assert_eq!(body["error"], "invalid_distribution");
}
