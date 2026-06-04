//! `GatewayState` — shared application state holding gRPC client handles.

use std::sync::Arc;
use tokio::sync::Mutex;
use tonic::transport::Channel;

use stitchd_proto::analytics::v1::analytics_service_client::AnalyticsServiceClient;
use stitchd_proto::auth::v1::{
    auth_provider_service_client::AuthProviderServiceClient,
    auth_service_client::AuthServiceClient, oidc_login_service_client::OidcLoginServiceClient,
    saml_login_service_client::SamlLoginServiceClient,
};
use stitchd_proto::experiments::v1::experimentation_service_client::ExperimentationServiceClient;
use stitchd_proto::flags::v1::flag_service_client::FlagServiceClient;
use stitchd_proto::management::v1::management_service_client::ManagementServiceClient;
use stitchd_proto::schedule::v1::schedule_service_client::ScheduleServiceClient;
use stitchd_proto::sdk::v1::{
    flag_sdk_backend_service_client::FlagSdkBackendServiceClient,
    segmentation_sdk_backend_service_client::SegmentationSdkBackendServiceClient,
};
use stitchd_proto::segments::v1::segmentation_service_client::SegmentationServiceClient;
use stitchd_proto::stats::v1::stats_service_client::StatsServiceClient;

/// Shared state injected into every Axum handler via `State<Arc<GatewayState>>`.
#[derive(Clone)]
pub struct GatewayState {
    /// Auth service gRPC client.
    pub auth_client: Arc<Mutex<AuthServiceClient<Channel>>>,
    /// Flag service gRPC client.
    pub flag_client: Arc<Mutex<FlagServiceClient<Channel>>>,
    /// Segmentation service gRPC client.
    pub segmentation_client: Arc<Mutex<SegmentationServiceClient<Channel>>>,
    /// Analytics service gRPC client — event ingestion, context registry, analytics reads.
    pub analytics_client: Arc<Mutex<AnalyticsServiceClient<Channel>>>,
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
    /// Stats service gRPC client.
    pub stats_client: Arc<Mutex<StatsServiceClient<Channel>>>,
    /// SDK backend client for flag-service (SyncDefinitions + IngestSdkEvalLog).
    pub flag_sdk_backend_client: Arc<Mutex<FlagSdkBackendServiceClient<Channel>>>,
    /// SDK backend client for segmentation-service (BatchCheckListMembership).
    pub segmentation_sdk_backend_client: Arc<Mutex<SegmentationSdkBackendServiceClient<Channel>>>,
    /// Schedule service gRPC client (scheduled-change CRUD/lifecycle).
    /// (flag_lifecycle_20260604)
    pub schedule_client: Arc<Mutex<ScheduleServiceClient<Channel>>>,
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
        analytics_addr: String,
        experimentation_addr: String,
        stats_addr: String,
        schedule_addr: String,
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

        let analytics_channel = Channel::from_shared(analytics_addr)
            .map_err(|e| anyhow::anyhow!("invalid Analytics Service URI: {e}"))?
            .connect()
            .await
            .map_err(|e| anyhow::anyhow!("connect to Analytics Service: {e}"))?;

        let exp_channel = Channel::from_shared(experimentation_addr)
            .map_err(|e| anyhow::anyhow!("invalid Experimentation Service URI: {e}"))?
            .connect()
            .await
            .map_err(|e| anyhow::anyhow!("connect to Experimentation Service: {e}"))?;

        let stats_channel = Channel::from_shared(stats_addr)
            .map_err(|e| anyhow::anyhow!("invalid Stats Service URI: {e}"))?
            .connect()
            .await
            .map_err(|e| anyhow::anyhow!("connect to Stats Service: {e}"))?;

        let schedule_channel = Channel::from_shared(schedule_addr)
            .map_err(|e| anyhow::anyhow!("invalid Schedule Service URI: {e}"))?
            .connect()
            .await
            .map_err(|e| anyhow::anyhow!("connect to Schedule Service: {e}"))?;

        let flag_channel_for_sdk = flag_channel.clone();
        let seg_channel_for_sdk = seg_channel.clone();

        Ok(Self {
            auth_client: Arc::new(Mutex::new(AuthServiceClient::new(auth_channel))),
            flag_client: Arc::new(Mutex::new(FlagServiceClient::new(flag_channel))),
            segmentation_client: Arc::new(Mutex::new(SegmentationServiceClient::new(seg_channel))),
            analytics_client: Arc::new(Mutex::new(AnalyticsServiceClient::new(analytics_channel))),
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
            stats_client: Arc::new(Mutex::new(StatsServiceClient::new(stats_channel))),
            flag_sdk_backend_client: Arc::new(Mutex::new(FlagSdkBackendServiceClient::new(
                flag_channel_for_sdk,
            ))),
            segmentation_sdk_backend_client: Arc::new(Mutex::new(
                SegmentationSdkBackendServiceClient::new(seg_channel_for_sdk),
            )),
            schedule_client: Arc::new(Mutex::new(ScheduleServiceClient::new(schedule_channel))),
        })
    }

    /// Build a `GatewayState` from pre-constructed clients (used in tests).
    #[must_use]
    #[allow(clippy::too_many_arguments)]
    pub fn from_channels(
        auth_client: AuthServiceClient<Channel>,
        flag_client: FlagServiceClient<Channel>,
        flag_channel: Channel,
        segmentation_client: SegmentationServiceClient<Channel>,
        segmentation_channel: Channel,
        analytics_client: AnalyticsServiceClient<Channel>,
        experimentation_client: ExperimentationServiceClient<Channel>,
        management_client: ManagementServiceClient<Channel>,
        auth_provider_client: AuthProviderServiceClient<Channel>,
        oidc_login_client: OidcLoginServiceClient<Channel>,
        saml_login_client: SamlLoginServiceClient<Channel>,
        stats_client: StatsServiceClient<Channel>,
    ) -> Self {
        Self {
            auth_client: Arc::new(Mutex::new(auth_client)),
            flag_client: Arc::new(Mutex::new(flag_client)),
            segmentation_client: Arc::new(Mutex::new(segmentation_client)),
            analytics_client: Arc::new(Mutex::new(analytics_client)),
            experimentation_client: Arc::new(Mutex::new(experimentation_client)),
            management_client: Arc::new(Mutex::new(management_client)),
            auth_provider_client: Arc::new(Mutex::new(auth_provider_client)),
            oidc_login_client: Arc::new(Mutex::new(oidc_login_client)),
            saml_login_client: Arc::new(Mutex::new(saml_login_client)),
            stats_client: Arc::new(Mutex::new(stats_client)),
            flag_sdk_backend_client: Arc::new(Mutex::new(FlagSdkBackendServiceClient::new(
                flag_channel,
            ))),
            segmentation_sdk_backend_client: Arc::new(Mutex::new(
                SegmentationSdkBackendServiceClient::new(segmentation_channel),
            )),
            // Lazy placeholder: tests that exercise schedule routes override this
            // via `with_schedule_client`; others never dial it.
            schedule_client: Arc::new(Mutex::new(ScheduleServiceClient::new(
                Channel::from_static("http://127.0.0.1:1").connect_lazy(),
            ))),
        }
    }

    /// Override the schedule-service client (used by schedule-route tests to
    /// point the gateway at an in-process mock `ScheduleService`).
    #[must_use]
    pub fn with_schedule_client(mut self, client: ScheduleServiceClient<Channel>) -> Self {
        self.schedule_client = Arc::new(Mutex::new(client));
        self
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
