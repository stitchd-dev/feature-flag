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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_sets_fields() {
        let client = SdkGrpcClient::new("http://grpc:9090", "my-sdk-key");
        assert_eq!(client.grpc_url, "http://grpc:9090");
        assert_eq!(client.sdk_key, "my-sdk-key");
    }

    #[test]
    fn new_accepts_owned_strings() {
        let url = String::from("http://localhost:9090");
        let key = String::from("abc123");
        let client = SdkGrpcClient::new(url, key);
        assert_eq!(client.grpc_url, "http://localhost:9090");
        assert_eq!(client.sdk_key, "abc123");
    }

    #[tokio::test]
    async fn fetch_definitions_fails_when_server_unreachable() {
        // Nothing listening on this port, so gRPC connect should fail.
        let client = SdkGrpcClient::new("http://127.0.0.1:19999", "key");
        let result = client.fetch_definitions().await;
        assert!(result.is_err());
        match result.unwrap_err() {
            SdkError::GrpcTransport(_) => {}
            other => panic!("expected GrpcTransport, got {other:?}"),
        }
    }
}
