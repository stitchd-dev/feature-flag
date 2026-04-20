//! Integration tests for `FlagSyncService` gRPC endpoint.
//!
//! These tests spin up a real tonic server against a live test DB.

use std::sync::Arc;

use chrono::Utc;
use stitchd_core::{
    id::{EnvironmentId, OrganisationId, ProjectId, SdkKeyId},
    tenant::{Environment, Organisation, Project, SdkKey},
};
use stitchd_db::{
    EnvironmentRepository, OrganisationRepository, ProjectRepository, SdkKeyRepository,
    repository::pg::{
        PgAuditLogger, PgEnvironmentRepository, PgEventDefinitionRepository,
        PgExperimentRepository, PgFlagRepository, PgOrganisationRepository, PgProjectRepository,
        PgSdkKeyRepository, PgSegmentRepository, PgVariantRepository,
    },
};
use stitchd_proto::flags::v1::{SyncRequest, flag_sync_service_client::FlagSyncServiceClient};
use stitchd_server::{AppState, grpc::flag_sync::FlagSyncServiceImpl};

const SDK_RAW_KEY: &str = "test-sdk-key-grpc-integration";

async fn setup(pool: sqlx::PgPool) -> (AppState, EnvironmentId) {
    let audit = Arc::new(PgAuditLogger::new(pool.clone()));
    let org_repo = PgOrganisationRepository::new(pool.clone(), audit.clone());
    let proj_repo = PgProjectRepository::new(pool.clone(), audit.clone());
    let env_repo = PgEnvironmentRepository::new(pool.clone(), audit.clone());

    let org = Organisation {
        id: OrganisationId::new(),
        name: "gRPC Test Org".into(),
        created_at: Utc::now(),
        updated_at: Utc::now(),
        deleted_at: None,
        version: 1,
    };
    org_repo.create(&org).await.unwrap();

    let project = Project {
        id: ProjectId::new(),
        organisation_id: org.id,
        name: "gRPC Test Project".into(),
        created_at: Utc::now(),
        updated_at: Utc::now(),
        deleted_at: None,
        version: 1,
    };
    proj_repo.create(&project).await.unwrap();

    let env = Environment {
        id: EnvironmentId::new(),
        project_id: project.id,
        name: "grpc-test".into(),
        created_at: Utc::now(),
        updated_at: Utc::now(),
        deleted_at: None,
        version: 1,
    };
    env_repo.create(&env).await.unwrap();

    let sdk_key_repo = Arc::new(PgSdkKeyRepository::new(pool.clone(), audit.clone()));

    // Create an SDK key with SHA-256 hash of SDK_RAW_KEY
    let key_hash = stitchd_server::api::sdk_auth::hash_sdk_key(SDK_RAW_KEY);
    let sdk_key = SdkKey {
        id: SdkKeyId::new(),
        environment_id: env.id,
        key_hash,
        is_active: true,
        created_at: Utc::now(),
        revoked_at: None,
    };
    sdk_key_repo.create(&sdk_key).await.unwrap();

    let metrics_handle = metrics_exporter_prometheus::PrometheusBuilder::new()
        .build_recorder()
        .handle();

    let state = AppState {
        db: pool.clone(),
        metrics_handle,
        segment_repo: Arc::new(PgSegmentRepository::new(pool.clone(), audit.clone())),
        flag_repo: Arc::new(PgFlagRepository::new(pool.clone(), audit.clone())),
        variant_repo: Arc::new(PgVariantRepository::new(pool.clone(), audit.clone())),
        sdk_key_repo,
        event_definition_repo: Arc::new(PgEventDefinitionRepository::new(
            pool.clone(),
            audit.clone(),
        )),
        experiment_repo: Arc::new(PgExperimentRepository::new(pool.clone(), audit.clone())),
        results_repo: Arc::new(
            stitchd_db::experiment_results::PgExperimentResultsRepository::new(pool.clone()),
        ),
        ch_client: None,
        event_writer: None,
    };

    (state, env.id)
}

#[sqlx::test(migrations = "../stitchd-db/migrations")]
async fn sync_with_valid_key_returns_ok(pool: sqlx::PgPool) {
    let (state, _env_id) = setup(pool).await;

    // Bind the gRPC server on a random port.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let svc = stitchd_proto::flags::v1::flag_sync_service_server::FlagSyncServiceServer::new(
        FlagSyncServiceImpl::new(state),
    );

    tokio::spawn(
        tonic::transport::Server::builder()
            .add_service(svc)
            .serve_with_incoming(tokio_stream::wrappers::TcpListenerStream::new(listener)),
    );

    let mut client = FlagSyncServiceClient::connect(format!("http://{addr}"))
        .await
        .unwrap();

    let mut request = tonic::Request::new(SyncRequest { contexts: vec![] });
    request
        .metadata_mut()
        .insert("x-sdk-key", SDK_RAW_KEY.parse().unwrap());

    let response = client.sync(request).await.unwrap();
    let sync_response = response.into_inner();

    // No flags or segments in the empty environment — verify the call succeeds.
    assert!(sync_response.flags.is_empty());
    assert!(sync_response.rule_segments.is_empty());
    assert!(sync_response.list_segments.is_empty());
    assert!(sync_response.server_timestamp_ms > 0);
}

#[sqlx::test(migrations = "../stitchd-db/migrations")]
async fn sync_with_invalid_key_returns_unauthenticated(pool: sqlx::PgPool) {
    let (state, _env_id) = setup(pool).await;

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let svc = stitchd_proto::flags::v1::flag_sync_service_server::FlagSyncServiceServer::new(
        FlagSyncServiceImpl::new(state),
    );

    tokio::spawn(
        tonic::transport::Server::builder()
            .add_service(svc)
            .serve_with_incoming(tokio_stream::wrappers::TcpListenerStream::new(listener)),
    );

    let mut client = FlagSyncServiceClient::connect(format!("http://{addr}"))
        .await
        .unwrap();

    let mut request = tonic::Request::new(SyncRequest { contexts: vec![] });
    request
        .metadata_mut()
        .insert("x-sdk-key", "wrong-key".parse().unwrap());

    let result = client.sync(request).await;
    assert!(result.is_err());
    let status = result.unwrap_err();
    assert_eq!(status.code(), tonic::Code::Unauthenticated);
}

#[sqlx::test(migrations = "../stitchd-db/migrations")]
async fn sync_with_missing_key_returns_unauthenticated(pool: sqlx::PgPool) {
    let (state, _env_id) = setup(pool).await;

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let svc = stitchd_proto::flags::v1::flag_sync_service_server::FlagSyncServiceServer::new(
        FlagSyncServiceImpl::new(state),
    );

    tokio::spawn(
        tonic::transport::Server::builder()
            .add_service(svc)
            .serve_with_incoming(tokio_stream::wrappers::TcpListenerStream::new(listener)),
    );

    let mut client = FlagSyncServiceClient::connect(format!("http://{addr}"))
        .await
        .unwrap();

    // No x-sdk-key header
    let request = tonic::Request::new(SyncRequest { contexts: vec![] });

    let result = client.sync(request).await;
    assert!(result.is_err());
    let status = result.unwrap_err();
    assert_eq!(status.code(), tonic::Code::Unauthenticated);
}
