//! gRPC client for fetching flag and segment definitions from the server.

use stitchd_proto::flags::v1::{
    SyncRequest, SyncResponse, flag_sync_service_client::FlagSyncServiceClient,
};

use crate::error::SdkError;

pub struct SdkGrpcClient {
    grpc_url: String,
    sdk_key: String,
}

impl SdkGrpcClient {
    pub fn new(grpc_url: impl Into<String>, sdk_key: impl Into<String>) -> Self {
        Self {
            grpc_url: grpc_url.into(),
            sdk_key: sdk_key.into(),
        }
    }

    pub async fn fetch_definitions(&self) -> Result<SyncResponse, SdkError> {
        let mut client = FlagSyncServiceClient::connect(self.grpc_url.clone())
            .await
            .map_err(|e| SdkError::GrpcTransport(e.to_string()))?;

        let mut request = tonic::Request::new(SyncRequest { contexts: vec![] });
        request.metadata_mut().insert(
            "x-sdk-key",
            self.sdk_key.parse().expect("sdk_key must be valid ASCII"),
        );

        let response = client.sync(request).await?;
        Ok(response.into_inner())
    }
}
