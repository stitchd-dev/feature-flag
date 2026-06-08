//! Flag Service gRPC client — verifies flags exist before activating experiments.

use std::sync::Arc;
use tonic::transport::Channel;

use stitchd_proto::flags::v1::{
    BanditUpdateAllocationRequest, GetFlagRequest, flag_service_client::FlagServiceClient,
};

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

    /// Fetch the full [`FeatureFlag`] for `flag_key` in `environment_id`.
    ///
    /// Used by the binding validator to inspect flag rules (e.g. verifying that
    /// at least one rule produces a percentage allocation when the experiment
    /// binds to a specific rule via `flag_rule_id`).
    ///
    /// Returns `Err(tonic::Status::not_found(...))` if the flag does not exist.
    pub async fn get_flag(
        &self,
        environment_id: &str,
        flag_key: &str,
    ) -> Result<stitchd_proto::flags::v1::FeatureFlag, tonic::Status> {
        let request = tonic::Request::new(GetFlagRequest {
            environment_id: environment_id.to_string(),
            flag_key: flag_key.to_string(),
            project_id: String::new(),
        });
        let mut client = self.inner.lock().await;
        client.get_flag(request).await.map(|r| r.into_inner())
    }

    /// Push a newly-computed bandit allocation to the flag-service's privileged
    /// `BanditUpdateAllocation` RPC (`bandit_20260608` Phase 3). The flag-service
    /// verifies `experiment_id` against the flag's current lock owner and bypasses
    /// the whole-flag human lock only for that owning experiment.
    ///
    /// `flag_rule_id` selects a custom rule; `None` targets the default rule.
    /// Returns the post-write flag version on success, or the gRPC `Status`
    /// (e.g. `ABORTED` on version conflict, `FAILED_PRECONDITION` when the
    /// experiment is not the lock owner).
    #[allow(clippy::too_many_arguments)]
    pub async fn bandit_update_allocation(
        &self,
        environment_id: &str,
        flag_key: &str,
        experiment_id: &str,
        flag_rule_id: Option<&str>,
        allocations: Vec<stitchd_proto::flags::v1::AllocationBucket>,
        expected_version: u64,
        realtime_model: Option<stitchd_proto::flags::v1::RealtimeBanditModel>,
    ) -> Result<u64, tonic::Status> {
        use stitchd_proto::flags::v1::bandit_update_allocation_request::Target;
        let target = match flag_rule_id {
            Some(rule_id) => Target::FlagRuleId(rule_id.to_string()),
            None => Target::DefaultRule(true),
        };
        let request = tonic::Request::new(BanditUpdateAllocationRequest {
            project_id: String::new(),
            flag_key: flag_key.to_string(),
            environment_id: environment_id.to_string(),
            experiment_id: experiment_id.to_string(),
            version: expected_version,
            allocations,
            target: Some(target),
            realtime_model,
        });
        let mut client = self.inner.lock().await;
        client
            .bandit_update_allocation(request)
            .await
            .map(|r| r.into_inner().version)
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
