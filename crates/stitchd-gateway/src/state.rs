//! `GatewayState` — shared application state holding gRPC client handles.

use std::sync::Arc;
use tokio::sync::Mutex;
use tonic::transport::Channel;

use stitchd_proto::auth::v1::{
    auth_provider_service_client::AuthProviderServiceClient,
    auth_service_client::AuthServiceClient,
    oidc_login_service_client::OidcLoginServiceClient,
    saml_login_service_client::SamlLoginServiceClient,
};
use stitchd_proto::events::v1::event_ingestion_service_client::EventIngestionServiceClient;
use stitchd_proto::experiments::v1::experimentation_service_client::ExperimentationServiceClient;
use stitchd_proto::flags::v1::flag_service_client::FlagServiceClient;
use stitchd_proto::management::v1::management_service_client::ManagementServiceClient;
use stitchd_proto::segments::v1::segmentation_service_client::SegmentationServiceClient;

/// Shared state injected into every Axum handler via `State<Arc<GatewayState>>`.
#[derive(Clone)]
pub struct GatewayState {
    /// Auth service gRPC client.
    pub auth_client: Arc<Mutex<AuthServiceClient<Channel>>>,
    /// Flag service gRPC client.
    pub flag_client: Arc<Mutex<FlagServiceClient<Channel>>>,
    /// Segmentation service gRPC client.
    pub segmentation_client: Arc<Mutex<SegmentationServiceClient<Channel>>>,
    /// Event ingestion service gRPC client.
    pub event_client: Arc<Mutex<EventIngestionServiceClient<Channel>>>,
    /// Experimentation service gRPC client.
    pub experimentation_client: Arc<Mutex<ExperimentationServiceClient<Channel>>>,
    /// Management service gRPC client (hosted on the auth-service port).
    pub management_client: Arc<Mutex<ManagementServiceClient<Channel>>>,
    /// Auth provider CRUD service gRPC client (hosted on the auth-service port).
    pub auth_provider_client: Arc<Mutex<AuthProviderServiceClient<Channel>>>,
    /// OIDC login service gRPC client (hosted on the auth-service port).
    pub oidc_login_client: Arc<Mutex<OidcLoginServiceClient<Channel>>>,
    /// SAML login service gRPC client (hosted on the auth-service port).
    pub saml_login_client: Arc<Mutex<SamlLoginServiceClient<Channel>>>,
}

impl GatewayState {
    /// Connect to all downstream services using the provided addresses.
    ///
    /// # Errors
    /// Returns an error if any gRPC channel cannot be established.
    pub async fn connect(
        auth_addr: String,
        flag_addr: String,
        segmentation_addr: String,
        event_addr: String,
        experimentation_addr: String,
    ) -> Result<Self, anyhow::Error> {
        let auth_channel = Channel::from_shared(auth_addr.clone())
            .map_err(|e| anyhow::anyhow!("invalid Auth Service URI: {e}"))?
            .connect()
            .await
            .map_err(|e| anyhow::anyhow!("connect to Auth Service: {e}"))?;

        let mgmt_channel = Channel::from_shared(auth_addr.clone())
            .map_err(|e| anyhow::anyhow!("invalid Management Service URI: {e}"))?
            .connect()
            .await
            .map_err(|e| anyhow::anyhow!("connect to Management Service: {e}"))?;

        let auth_provider_channel = Channel::from_shared(auth_addr.clone())
            .map_err(|e| anyhow::anyhow!("invalid Auth Provider Service URI: {e}"))?
            .connect()
            .await
            .map_err(|e| anyhow::anyhow!("connect to Auth Provider Service: {e}"))?;

        let oidc_login_channel = Channel::from_shared(auth_addr.clone())
            .map_err(|e| anyhow::anyhow!("invalid OIDC Login Service URI: {e}"))?
            .connect()
            .await
            .map_err(|e| anyhow::anyhow!("connect to OIDC Login Service: {e}"))?;

        let saml_login_channel = Channel::from_shared(auth_addr)
            .map_err(|e| anyhow::anyhow!("invalid SAML Login Service URI: {e}"))?
            .connect()
            .await
            .map_err(|e| anyhow::anyhow!("connect to SAML Login Service: {e}"))?;

        let flag_channel = Channel::from_shared(flag_addr)
            .map_err(|e| anyhow::anyhow!("invalid Flag Service URI: {e}"))?
            .connect()
            .await
            .map_err(|e| anyhow::anyhow!("connect to Flag Service: {e}"))?;

        let seg_channel = Channel::from_shared(segmentation_addr)
            .map_err(|e| anyhow::anyhow!("invalid Segmentation Service URI: {e}"))?
            .connect()
            .await
            .map_err(|e| anyhow::anyhow!("connect to Segmentation Service: {e}"))?;

        let event_channel = Channel::from_shared(event_addr)
            .map_err(|e| anyhow::anyhow!("invalid Event Service URI: {e}"))?
            .connect()
            .await
            .map_err(|e| anyhow::anyhow!("connect to Event Service: {e}"))?;

        let exp_channel = Channel::from_shared(experimentation_addr)
            .map_err(|e| anyhow::anyhow!("invalid Experimentation Service URI: {e}"))?
            .connect()
            .await
            .map_err(|e| anyhow::anyhow!("connect to Experimentation Service: {e}"))?;

        Ok(Self {
            auth_client: Arc::new(Mutex::new(AuthServiceClient::new(auth_channel))),
            flag_client: Arc::new(Mutex::new(FlagServiceClient::new(flag_channel))),
            segmentation_client: Arc::new(Mutex::new(SegmentationServiceClient::new(seg_channel))),
            event_client: Arc::new(Mutex::new(EventIngestionServiceClient::new(event_channel))),
            experimentation_client: Arc::new(Mutex::new(ExperimentationServiceClient::new(
                exp_channel,
            ))),
            management_client: Arc::new(Mutex::new(ManagementServiceClient::new(mgmt_channel))),
            auth_provider_client: Arc::new(Mutex::new(AuthProviderServiceClient::new(
                auth_provider_channel,
            ))),
            oidc_login_client: Arc::new(Mutex::new(OidcLoginServiceClient::new(
                oidc_login_channel,
            ))),
            saml_login_client: Arc::new(Mutex::new(SamlLoginServiceClient::new(
                saml_login_channel,
            ))),
        })
    }

    /// Build a `GatewayState` from channels (used in tests).
    #[must_use]
    pub fn from_channels(
        auth_client: AuthServiceClient<Channel>,
        flag_client: FlagServiceClient<Channel>,
        segmentation_client: SegmentationServiceClient<Channel>,
        event_client: EventIngestionServiceClient<Channel>,
        experimentation_client: ExperimentationServiceClient<Channel>,
        management_client: ManagementServiceClient<Channel>,
        auth_provider_client: AuthProviderServiceClient<Channel>,
        oidc_login_client: OidcLoginServiceClient<Channel>,
        saml_login_client: SamlLoginServiceClient<Channel>,
    ) -> Self {
        Self {
            auth_client: Arc::new(Mutex::new(auth_client)),
            flag_client: Arc::new(Mutex::new(flag_client)),
            segmentation_client: Arc::new(Mutex::new(segmentation_client)),
            event_client: Arc::new(Mutex::new(event_client)),
            experimentation_client: Arc::new(Mutex::new(experimentation_client)),
            management_client: Arc::new(Mutex::new(management_client)),
            auth_provider_client: Arc::new(Mutex::new(auth_provider_client)),
            oidc_login_client: Arc::new(Mutex::new(oidc_login_client)),
            saml_login_client: Arc::new(Mutex::new(saml_login_client)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gateway_state_is_clone() {
        fn assert_clone<T: Clone>() {}
        assert_clone::<GatewayState>();
    }

    #[test]
    fn gateway_state_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<GatewayState>();
    }
}
