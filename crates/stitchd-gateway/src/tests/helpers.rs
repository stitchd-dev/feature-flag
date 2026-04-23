//! Test helper factories for `GatewayState`.

use std::sync::Arc;
use tonic::transport::Channel;

use stitchd_proto::auth::v1::{
    auth_provider_service_client::AuthProviderServiceClient,
    auth_service_client::AuthServiceClient, oidc_login_service_client::OidcLoginServiceClient,
    saml_login_service_client::SamlLoginServiceClient,
};
use stitchd_proto::events::v1::event_ingestion_service_client::EventIngestionServiceClient;
use stitchd_proto::experiments::v1::experimentation_service_client::ExperimentationServiceClient;
use stitchd_proto::flags::v1::flag_service_client::FlagServiceClient;
use stitchd_proto::management::v1::management_service_client::ManagementServiceClient;
use stitchd_proto::segments::v1::segmentation_service_client::SegmentationServiceClient;
use stitchd_proto::stats::v1::stats_service_client::StatsServiceClient;

use crate::state::GatewayState;

/// Lazily-connected clients pointing at unused localhost ports.
/// The connections are lazy — they fail only when the first RPC is made.
pub fn make_stub_state() -> Arc<GatewayState> {
    let auth = AuthServiceClient::new(Channel::from_static("http://127.0.0.1:1").connect_lazy());
    let flag = FlagServiceClient::new(Channel::from_static("http://127.0.0.1:2").connect_lazy());
    let seg =
        SegmentationServiceClient::new(Channel::from_static("http://127.0.0.1:3").connect_lazy());
    let event =
        EventIngestionServiceClient::new(Channel::from_static("http://127.0.0.1:4").connect_lazy());
    let exp = ExperimentationServiceClient::new(
        Channel::from_static("http://127.0.0.1:5").connect_lazy(),
    );
    let mgmt =
        ManagementServiceClient::new(Channel::from_static("http://127.0.0.1:6").connect_lazy());
    let auth_provider =
        AuthProviderServiceClient::new(Channel::from_static("http://127.0.0.1:7").connect_lazy());
    let oidc_login =
        OidcLoginServiceClient::new(Channel::from_static("http://127.0.0.1:8").connect_lazy());
    let saml_login =
        SamlLoginServiceClient::new(Channel::from_static("http://127.0.0.1:9").connect_lazy());
    let stats = StatsServiceClient::new(Channel::from_static("http://127.0.0.1:10").connect_lazy());
    Arc::new(GatewayState::from_channels(
        auth,
        flag,
        seg,
        event,
        exp,
        mgmt,
        auth_provider,
        oidc_login,
        saml_login,
        stats,
    ))
}

/// Same as `make_stub_state` — the `_with_flag` variant exists for tests that
/// import it but currently rely on the lazy-connection behavior anyway.
pub fn make_stub_state_with_flag() -> Arc<GatewayState> {
    make_stub_state()
}
