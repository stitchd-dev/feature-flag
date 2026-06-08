//! Integration tests for the admin flag GET projection — proves the gateway
//! surfaces every per-rule UUID + name and the flag-level
//! `locked_by_experiment_id` once the flag-service emits them on the wire
//! (feature-flag-1p6).
//!
//! These coverage points unblock the admin UI:
//! - `CreateExperimentModal` binds an experiment to a real `rule.rule_id`
//!   instead of fabricating index-derived placeholders.
//! - `EditFlagDefaultRule` renders the lock badge on mount instead of after
//!   a failing save round-trip.

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
    EvaluatePreviewRequest, EvaluatePreviewResponse, FeatureFlag, FlagRule,
    GetFlagDefinitionsRequest, GetFlagRequest, ListFlagsRequest, ListFlagsResponse,
    MutateFlagRequest, MutateFlagResponse, UpdateFlagHashingRequest, UpdateFlagHashingResponse,
    flag_rule::Output,
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
            "/v1/projects/{project_id}/flags/{flag_id}",
            get(flag_routes::get_flag),
        )
        .with_state(state)
}

/// Mock `FlagService` that returns a pre-baked `FeatureFlag` with both new
/// admin-metadata fields populated.
#[derive(Clone)]
struct MetadataFlagService {
    flag: FeatureFlag,
}

#[tonic::async_trait]
impl FlagService for MetadataFlagService {
    async fn get_flag(
        &self,
        _req: tonic::Request<GetFlagRequest>,
    ) -> Result<Response<FeatureFlag>, Status> {
        Ok(Response::new(self.flag.clone()))
    }
    async fn list_flags(
        &self,
        _req: tonic::Request<ListFlagsRequest>,
    ) -> Result<Response<ListFlagsResponse>, Status> {
        Ok(Response::new(ListFlagsResponse {
            flags: vec![self.flag.clone()],
            total: 1,
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

async fn spawn_mock(svc: MetadataFlagService) -> FlagServiceClient<Channel> {
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

const EXP_ID: &str = "11111111-aaaa-bbbb-cccc-222222222222";
const RULE_ID: &str = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";

fn baked_flag() -> FeatureFlag {
    FeatureFlag {
        key: "my-flag".to_string(),
        enabled: true,
        flag_id: "flag-uuid".to_string(),
        rules: vec![FlagRule {
            rule_payload: b"null".to_vec(),
            output: Some(Output::VariantKey("on".to_string())),
            name: "premium-cohort".to_string(),
            rule_id: RULE_ID.to_string(),
        }],
        locked_by_experiment_id: EXP_ID.to_string(),
        ..Default::default()
    }
}

#[tokio::test]
async fn get_flag_surfaces_rule_id_and_name() {
    let flag_client = spawn_mock(MetadataFlagService { flag: baked_flag() }).await;
    let app = build_router(make_state(flag_client));

    let resp = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/v1/projects/proj-1/flags/my-flag")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body_bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let body: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();

    let rules = body["rules"].as_array().expect("rules array");
    assert_eq!(rules.len(), 1);
    assert_eq!(
        rules[0]["rule_id"], RULE_ID,
        "gateway must surface the per-rule UUID so the admin UI does not fake it"
    );
    assert_eq!(rules[0]["name"], "premium-cohort");
}

#[tokio::test]
async fn get_flag_surfaces_locked_by_experiment_id_when_locked() {
    let flag_client = spawn_mock(MetadataFlagService { flag: baked_flag() }).await;
    let app = build_router(make_state(flag_client));

    let resp = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/v1/projects/proj-1/flags/my-flag")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body_bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let body: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
    assert_eq!(
        body["locked_by_experiment_id"], EXP_ID,
        "gateway must surface the experiment UUID locking the flag"
    );
}

#[tokio::test]
async fn get_flag_omits_locked_by_experiment_id_when_unlocked() {
    let mut flag = baked_flag();
    flag.locked_by_experiment_id = String::new();
    let flag_client = spawn_mock(MetadataFlagService { flag }).await;
    let app = build_router(make_state(flag_client));

    let resp = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/v1/projects/proj-1/flags/my-flag")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body_bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let body: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
    assert!(
        body.get("locked_by_experiment_id").is_none(),
        "empty proto value must omit the JSON key entirely; body={body}",
    );
}
