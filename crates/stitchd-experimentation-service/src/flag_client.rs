//! Flag Service gRPC client — verifies flags exist before activating experiments.

use std::sync::Arc;
use tonic::transport::Channel;

use stitchd_proto::flags::v1::{GetFlagRequest, flag_service_client::FlagServiceClient};

/// A thin wrapper around the Flag Service gRPC client.
#[derive(Clone)]
pub struct FlagClient {
    inner: Arc<tokio::sync::Mutex<FlagServiceClient<Channel>>>,
}

impl FlagClient {
    /// Construct a new `FlagClient` connected to `addr`.
    ///
    /// # Errors
    /// Returns an error if the gRPC channel cannot be established.
    pub async fn connect(addr: String) -> Result<Self, anyhow::Error> {
        let channel = Channel::from_shared(addr)
            .map_err(|e| anyhow::anyhow!("invalid Flag Service URI: {e}"))?
            .connect()
            .await
            .map_err(|e| anyhow::anyhow!("connect to Flag Service: {e}"))?;
        let client = FlagServiceClient::new(channel);
        Ok(Self {
            inner: Arc::new(tokio::sync::Mutex::new(client)),
        })
    }

    /// Verify that a flag with `flag_key` exists in `environment_id`.
    ///
    /// Returns `Ok(())` if the flag exists.
    /// Returns `Err(tonic::Status)` with `NOT_FOUND` code if the flag is absent,
    /// or any other gRPC error code on transport/internal failures.
    pub async fn verify_flag_exists(
        &self,
        environment_id: &str,
        flag_key: &str,
    ) -> Result<(), tonic::Status> {
        let request = tonic::Request::new(GetFlagRequest {
            environment_id: environment_id.to_string(),
            flag_key: flag_key.to_string(),
            project_id: String::new(),
        });
        let mut client = self.inner.lock().await;
        client.get_flag(request).await.map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flag_client_struct_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<FlagClient>();
    }
}
