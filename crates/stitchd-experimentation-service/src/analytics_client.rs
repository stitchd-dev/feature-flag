//! Analytics Service gRPC client — streams experiment results from ClickHouse
//! via the analytics-service `ListExperimentResults` RPC.
//!
//! The [`AnalyticsResultsPort`] trait is the seam used in tests: production
//! code gets [`AnalyticsClient`]; tests inject a mock implementing the trait.

use std::sync::Arc;
use tonic::transport::Channel;

use stitchd_proto::analytics::v1::{
    ExperimentResult, ListExperimentResultsRequest,
    analytics_service_client::AnalyticsServiceClient,
};

// ---------------------------------------------------------------------------
// Testability seam
// ---------------------------------------------------------------------------

/// Trait over the analytics-service operations that `ExperimentationService`
/// depends on.  A mock implementing this trait is used in unit tests.
#[async_trait::async_trait]
pub trait AnalyticsResultsPort: Send + Sync {
    /// Collect all experiment results for `(env_id, experiment_id[, iteration_id])`.
    async fn list_experiment_results(
        &self,
        env_id: &str,
        experiment_id: &str,
        iteration_id: Option<&str>,
    ) -> Result<Vec<ExperimentResult>, tonic::Status>;
}

// ---------------------------------------------------------------------------
// Production implementation
// ---------------------------------------------------------------------------

/// A thin wrapper around the Analytics Service gRPC client used by
/// `ExperimentationService` to fetch experiment results.
#[derive(Clone)]
pub struct AnalyticsClient {
    inner: Arc<tokio::sync::Mutex<AnalyticsServiceClient<Channel>>>,
}

impl AnalyticsClient {
    /// Construct a new `AnalyticsClient` connected to `addr`.
    ///
    /// # Errors
    /// Returns an error if the gRPC channel cannot be established.
    pub async fn connect(addr: String) -> Result<Self, anyhow::Error> {
        let channel = Channel::from_shared(addr)
            .map_err(|e| anyhow::anyhow!("invalid Analytics Service URI: {e}"))?
            .connect()
            .await
            .map_err(|e| anyhow::anyhow!("connect to Analytics Service: {e}"))?;
        let client = AnalyticsServiceClient::new(channel);
        Ok(Self {
            inner: Arc::new(tokio::sync::Mutex::new(client)),
        })
    }
}

#[async_trait::async_trait]
impl AnalyticsResultsPort for AnalyticsClient {
    /// Stream all experiment results for the given experiment (and optional
    /// iteration) from analytics-service ClickHouse.
    async fn list_experiment_results(
        &self,
        env_id: &str,
        experiment_id: &str,
        iteration_id: Option<&str>,
    ) -> Result<Vec<ExperimentResult>, tonic::Status> {
        use tokio_stream::StreamExt as _;

        let request = tonic::Request::new(ListExperimentResultsRequest {
            env_id: env_id.to_string(),
            experiment_id: experiment_id.to_string(),
            iteration_id: iteration_id.map(str::to_string),
        });

        let mut client = self.inner.lock().await;
        let stream = client.list_experiment_results(request).await?.into_inner();
        let results: Vec<ExperimentResult> = stream
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .collect::<Result<Vec<_>, _>>()?;
        Ok(results)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn analytics_client_struct_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<AnalyticsClient>();
    }
}
