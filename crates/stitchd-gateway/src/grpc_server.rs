//! gRPC server hosted by the gateway for SDK-facing traffic.
//!
//! This is the **gRPC half** of the SDK ↔ Gateway contract. The REST half
//! lives in `routes::sdk_backend`. SDKs poll the gateway's tonic server for
//! flag/segment definitions via `SdkService::SyncDefinitions`.
//!
//! ### Trust boundary
//!
//! Same model as the REST routes:
//! 1. SDK sends `x-sdk-key` in gRPC metadata.
//! 2. Gateway validates via `AuthServiceClient::validate_credential` (which
//!    internally uses `SdkKeyCache` from `db_optim_20260516`).
//! 3. On success, gateway forwards to the appropriate backend RPC with the
//!    resolved `x-env-id` metadata.
//! 4. Backend services trust `x-env-id` and never see the SDK key.
//!
//! Server lifecycle is managed by `main::serve_grpc` (spawned alongside the
//! Axum REST server).

use std::sync::Arc;

use tokio::sync::Mutex;
use tonic::{Request, Response, Status, transport::Channel};

use stitchd_proto::auth::v1::{
    CredentialRequest, RbacContext, auth_service_client::AuthServiceClient,
    credential_request::Credential,
};
use stitchd_proto::sdk::v1::{
    IngestSdkEvalLogRequest, IngestSdkEvalLogResponse, SyncDefinitionsRequest,
    SyncDefinitionsResponse, flag_sdk_backend_service_client::FlagSdkBackendServiceClient,
    sdk_service_server::SdkService,
};

const SDK_KEY_METADATA_KEY: &str = "x-sdk-key";
const ENV_ID_METADATA_KEY: &str = "x-env-id";

/// Implementation of [`SdkService`] hosted by the gateway. Forwards
/// authenticated SDK calls to the appropriate backend service.
pub struct GatewaySdkServiceImpl {
    auth_client: Arc<Mutex<AuthServiceClient<Channel>>>,
    flag_sdk_backend_client: Arc<Mutex<FlagSdkBackendServiceClient<Channel>>>,
}

impl GatewaySdkServiceImpl {
    #[must_use]
    pub const fn new(
        auth_client: Arc<Mutex<AuthServiceClient<Channel>>>,
        flag_sdk_backend_client: Arc<Mutex<FlagSdkBackendServiceClient<Channel>>>,
    ) -> Self {
        Self {
            auth_client,
            flag_sdk_backend_client,
        }
    }

    /// Validate `x-sdk-key` from inbound metadata and resolve the environment.
    /// Returns the resolved `RbacContext`, from which `environment_id` is read.
    async fn authenticate(
        &self,
        metadata: &tonic::metadata::MetadataMap,
    ) -> Result<RbacContext, Status> {
        let sdk_key = metadata
            .get(SDK_KEY_METADATA_KEY)
            .ok_or_else(|| Status::unauthenticated("missing x-sdk-key metadata"))?
            .to_str()
            .map_err(|_| Status::unauthenticated("x-sdk-key not valid UTF-8"))?
            .to_string();

        let credential_req = CredentialRequest {
            credential: Some(Credential::SdkKey(sdk_key)),
        };

        let resp = {
            let mut client = self.auth_client.lock().await;
            client
                .validate_credential(Request::new(credential_req))
                .await?
        };

        Ok(resp.into_inner())
    }
}

/// Build a `tonic::Request` carrying the inbound payload plus an `x-env-id`
/// metadata header. Used for every forwarded backend call.
#[allow(clippy::result_large_err)]
fn forward_request<T>(payload: T, env_id: &str) -> Result<Request<T>, Status> {
    let mut req = Request::new(payload);
    let value = env_id
        .parse()
        .map_err(|_| Status::internal(format!("env_id is not valid metadata: {env_id:?}")))?;
    req.metadata_mut().insert(ENV_ID_METADATA_KEY, value);
    Ok(req)
}

#[tonic::async_trait]
impl SdkService for GatewaySdkServiceImpl {
    async fn sync_definitions(
        &self,
        request: Request<SyncDefinitionsRequest>,
    ) -> Result<Response<SyncDefinitionsResponse>, Status> {
        let rbac = self.authenticate(request.metadata()).await?;
        let inner = request.into_inner();
        let forwarded = forward_request(inner, &rbac.environment_id)?;
        let mut client = self.flag_sdk_backend_client.lock().await;
        client.sync_definitions(forwarded).await
    }

    async fn ingest_sdk_eval_log(
        &self,
        request: Request<IngestSdkEvalLogRequest>,
    ) -> Result<Response<IngestSdkEvalLogResponse>, Status> {
        let rbac = self.authenticate(request.metadata()).await?;
        let mut inner = request.into_inner();
        // Authoritatively stamp environment_id on every event from the
        // resolved SDK context (mirror of REST events:batch behaviour).
        for ev in &mut inner.events {
            ev.environment_id.clone_from(&rbac.environment_id);
        }
        let forwarded = forward_request(inner, &rbac.environment_id)?;
        let mut client = self.flag_sdk_backend_client.lock().await;
        client.ingest_sdk_eval_log(forwarded).await
    }
}

/// Build a tonic `Server` (callable via `serve(addr)`) hosting `SdkService`.
///
/// Used by `main` to start the gRPC server alongside the Axum REST server.
#[must_use]
pub fn build_grpc_server(
    auth_client: Arc<Mutex<AuthServiceClient<Channel>>>,
    flag_sdk_backend_client: Arc<Mutex<FlagSdkBackendServiceClient<Channel>>>,
) -> tonic::transport::server::Router {
    use stitchd_proto::sdk::v1::sdk_service_server::SdkServiceServer;

    let impl_ = GatewaySdkServiceImpl::new(auth_client, flag_sdk_backend_client);
    tonic::transport::Server::builder().add_service(SdkServiceServer::new(impl_))
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    type StubClients = (
        Arc<Mutex<AuthServiceClient<Channel>>>,
        Arc<Mutex<FlagSdkBackendServiceClient<Channel>>>,
    );

    fn stub_clients() -> StubClients {
        // Lazily-connecting channels — never actually establish a connection
        // for short-circuit tests (auth failure paths don't reach the backend).
        let auth_channel = Channel::from_static("http://127.0.0.1:1").connect_lazy();
        let flag_channel = Channel::from_static("http://127.0.0.1:2").connect_lazy();
        (
            Arc::new(Mutex::new(AuthServiceClient::new(auth_channel))),
            Arc::new(Mutex::new(FlagSdkBackendServiceClient::new(flag_channel))),
        )
    }

    #[tokio::test]
    async fn sync_definitions_rejects_missing_sdk_key_metadata() {
        let (auth, flag) = stub_clients();
        let svc = GatewaySdkServiceImpl::new(auth, flag);
        let req = Request::new(SyncDefinitionsRequest {}); // no x-sdk-key
        let status = svc.sync_definitions(req).await.unwrap_err();
        assert_eq!(status.code(), tonic::Code::Unauthenticated);
        assert!(status.message().contains("x-sdk-key"));
    }

    #[tokio::test]
    async fn ingest_sdk_eval_log_rejects_missing_sdk_key_metadata() {
        let (auth, flag) = stub_clients();
        let svc = GatewaySdkServiceImpl::new(auth, flag);
        let req = Request::new(IngestSdkEvalLogRequest { events: vec![] });
        let status = svc.ingest_sdk_eval_log(req).await.unwrap_err();
        assert_eq!(status.code(), tonic::Code::Unauthenticated);
    }

    #[tokio::test]
    async fn sync_definitions_treats_unrelated_metadata_as_missing_key() {
        let (auth, flag) = stub_clients();
        let svc = GatewaySdkServiceImpl::new(auth, flag);
        let mut req = Request::new(SyncDefinitionsRequest {});
        // An unrelated header doesn't satisfy x-sdk-key — should still fail
        // with Unauthenticated for missing key.
        req.metadata_mut()
            .insert("user-agent", "stitchd-sdk/0.1".parse().unwrap());
        let status = svc.sync_definitions(req).await.unwrap_err();
        assert_eq!(status.code(), tonic::Code::Unauthenticated);
        assert!(status.message().contains("x-sdk-key"));
    }

    #[test]
    fn forward_request_attaches_env_id_metadata() {
        let req = forward_request(
            SyncDefinitionsRequest {},
            "00000000-0000-0000-0000-000000000001",
        )
        .unwrap();
        let val = req.metadata().get(ENV_ID_METADATA_KEY).unwrap();
        assert_eq!(
            val.to_str().unwrap(),
            "00000000-0000-0000-0000-000000000001"
        );
    }

    #[test]
    fn forward_request_rejects_invalid_metadata_value() {
        // Newline is invalid in HTTP/2 header values.
        let err = forward_request(SyncDefinitionsRequest {}, "bad\nvalue").unwrap_err();
        assert_eq!(err.code(), tonic::Code::Internal);
    }

    #[tokio::test]
    async fn build_grpc_server_returns_valid_router() {
        // Smoke test — compiles + constructs the gRPC Router without panicking.
        // Must run inside a tokio runtime since hyper-util initialises a reactor.
        let (auth, flag) = stub_clients();
        let _router = build_grpc_server(auth, flag);
    }
}
