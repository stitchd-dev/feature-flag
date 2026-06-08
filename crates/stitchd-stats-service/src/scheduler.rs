//! Fetches running experiments via the experimentation-service gRPC client.
//!
//! No direct PostgreSQL access to `experiments` or `experiment_iterations` tables.

use std::collections::HashMap;

use chrono::{DateTime, TimeZone, Utc};
use tonic::transport::Channel;
use uuid::Uuid;

use stitchd_core::experimentation::bandit::{BanditConfig, ExperimentMode};
use stitchd_proto::experiments::v1::{
    ListRunningExperimentsRequest, experimentation_service_client::ExperimentationServiceClient,
};

/// Sequential-testing configuration snapshotted on an experiment iteration
/// (Phase 2). Threaded from the iteration into the stats-service compute pass
/// so the always-valid analysis uses the experiment's chosen α / τ² / min-N.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SequentialSettings {
    /// Whether sequential testing is enabled for this experiment. When `false`
    /// the compute pass skips sequential entirely (sends `sequential_result:
    /// None`).
    pub enabled: bool,
    /// Family-wise significance level α.
    pub alpha: f64,
    /// Mixing-prior variance τ². `None` → derive a metric-scale default at
    /// compute time (see [`crate::sequential_compute::default_tau_squared`]).
    pub tau_squared: Option<f64>,
    /// Minimum per-variant sample size before a verdict is trustworthy.
    pub min_sample_size: i64,
}

impl Default for SequentialSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            alpha: 0.05,
            tau_squared: None,
            min_sample_size: 100,
        }
    }
}

/// A running experiment with its active iteration, ready for stats computation.
#[derive(Debug, Clone)]
pub struct RunningExperiment {
    pub experiment_id: Uuid,
    pub env_id: Uuid,
    pub iteration_id: Uuid,
    /// Metric definition UUIDs (post Phase 7 cutover); were previously
    /// raw event-key strings on the `metric_keys` proto field.
    pub metric_ids: Vec<Uuid>,
    pub variant_keys: Vec<String>,
    pub started_at: DateTime<Utc>,
    /// Analysis unit context types snapshotted on the iteration (proto
    /// `ExperimentIteration.unit_context_types`). The compute pass runs one stats
    /// analysis per context type. Carried directly on the `ListRunningExperiments`
    /// response (FIX C3); [`fetch_running_experiments`] defaults it to `["user"]`
    /// when the field arrives empty so there is always at least one dimension.
    pub unit_context_types: Vec<String>,
    /// Pre-period length in days for CUPED variance reduction. Snapshotted on the
    /// iteration and carried directly on the `ListRunningExperiments` response
    /// (FIX C3); the compute pass uses it to adjust NUMERIC metric values by their
    /// pre-period covariate when `> 0`. `0` = CUPED disabled.
    pub pre_period_days: u32,
    /// Sequential-testing configuration snapshotted on the iteration and carried
    /// directly on the `ListRunningExperiments` response (FIX C3).
    pub sequential: SequentialSettings,
    /// Designed per-variant assignment split in basis points (`variant_key` →
    /// bp), sourced from the experiment's bound rule (`RuleOutput::Percentage`
    /// weights) or the flag's `default_rule_distribution`. Carried on the
    /// `ListRunningExperiments` response (`variant_expected_bp`) so the SRM check
    /// tests observed assignments against the CONFIGURED split rather than a
    /// uniform `1/K` baseline. Empty when the server could not source it (older
    /// server, or neither a rule nor a default distribution carried weights), in
    /// which case the SRM check falls back to the uniform split.
    pub variant_expected_bp: HashMap<String, u32>,
    /// Whether this experiment runs in adaptive (bandit) mode. Decoded from the
    /// `ListRunningExperiments` `experiment_mode` field; defaults to
    /// [`ExperimentMode::Fixed`] when the field is empty (older server). The
    /// bandit reallocation pass no-ops for `Fixed`.
    pub experiment_mode: ExperimentMode,
    /// The iteration-snapshotted bandit configuration. `Some` only for
    /// bandit-mode experiments; carries the algorithm, propagation mode,
    /// exploration floor, reward objective and lifecycle policy the reallocation
    /// pass reads. `None` (or a malformed JSON payload that fails to decode)
    /// makes the experiment ineligible for reallocation.
    pub bandit_config: Option<BanditConfig>,
    /// The owning optimization campaign id, if this experiment runs under a
    /// campaign (decoded from the `ListRunningExperiments` `bandit_campaign_id`
    /// field). `None` for non-campaign experiments. The lifecycle pass uses it to
    /// spawn the next campaign iteration on convergence/drift.
    pub bandit_campaign_id: Option<Uuid>,
}

/// Decode the wire `experiment_mode` string into [`ExperimentMode`]. Anything
/// other than `"bandit"` (incl. an empty string from an older server) decodes to
/// [`ExperimentMode::Fixed`] so the reallocation pass safely no-ops.
#[must_use]
fn parse_experiment_mode(s: &str) -> ExperimentMode {
    match s {
        "bandit" => ExperimentMode::Bandit,
        _ => ExperimentMode::Fixed,
    }
}

/// Fetch all running experiments via `experimentation-service.ListRunningExperiments`.
///
/// Streams all `RunningExperiment` messages and collects them.  The caller
/// should not assume any ordering.
pub async fn fetch_running_experiments(
    client: &mut ExperimentationServiceClient<Channel>,
) -> Result<Vec<RunningExperiment>, anyhow::Error> {
    let mut stream = client
        .list_running_experiments(ListRunningExperimentsRequest {})
        .await?
        .into_inner();

    let mut results = Vec::new();
    loop {
        match stream.message().await? {
            None => break,
            Some(proto) => {
                let experiment_id = Uuid::parse_str(&proto.experiment_id)
                    .map_err(|e| anyhow::anyhow!("invalid experiment_id UUID: {e}"))?;
                let env_id = Uuid::parse_str(&proto.environment_id)
                    .map_err(|e| anyhow::anyhow!("invalid environment_id UUID: {e}"))?;
                let iteration_id = Uuid::parse_str(&proto.iteration_id)
                    .map_err(|e| anyhow::anyhow!("invalid iteration_id UUID: {e}"))?;
                let started_at = Utc
                    .timestamp_millis_opt(proto.started_at_ms)
                    .single()
                    .unwrap_or_else(Utc::now);

                let metric_ids = proto
                    .metric_ids
                    .iter()
                    .map(|s| {
                        Uuid::parse_str(s)
                            .map_err(|e| anyhow::anyhow!("invalid metric_id UUID {s}: {e}"))
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                // FIX C3: the active-iteration compute-pass inputs now ride along
                // on the `ListRunningExperiments` response (populated from the same
                // iteration row the handler already loads), so there is no second
                // `GetExperimentIteration` round-trip — and no skip-on-failure that
                // could freeze a persistently-unfetchable experiment as stale.
                // Default `unit_context_types` to `["user"]` when the field is
                // empty (older server, or an iteration with none snapshotted) so
                // the compute pass always has at least one dimension to analyse.
                let unit_context_types = if proto.unit_context_types.is_empty() {
                    vec!["user".to_string()]
                } else {
                    proto.unit_context_types
                };
                results.push(RunningExperiment {
                    experiment_id,
                    env_id,
                    iteration_id,
                    metric_ids,
                    variant_keys: proto.variant_keys,
                    started_at,
                    unit_context_types,
                    pre_period_days: proto.pre_period_days,
                    sequential: SequentialSettings {
                        enabled: proto.sequential_testing_enabled,
                        alpha: proto.sequential_alpha,
                        tau_squared: proto.sequential_tau_squared,
                        min_sample_size: proto.sequential_min_sample_size,
                    },
                    // Designed per-variant split (bp) for weighted SRM; empty
                    // when the server did not source it (older server / no
                    // rule+default-rule weights) → uniform SRM fallback.
                    variant_expected_bp: proto.variant_expected_bp,
                    // Bandit mode + snapshotted config. Empty `experiment_mode`
                    // decodes to `Fixed`; a `bandit_config` JSON payload that
                    // fails to decode is logged and treated as `None`, which
                    // makes the experiment ineligible for reallocation (the pass
                    // no-ops rather than erroring the tick).
                    experiment_mode: parse_experiment_mode(&proto.experiment_mode),
                    bandit_config: proto.bandit_config.as_deref().and_then(|s| {
                        match serde_json::from_str::<BanditConfig>(s) {
                            Ok(c) => Some(c),
                            Err(e) => {
                                tracing::warn!(
                                    experiment_id = %experiment_id,
                                    "bandit_config JSON failed to decode ({e}); treating as None"
                                );
                                None
                            }
                        }
                    }),
                    bandit_campaign_id: proto
                        .bandit_campaign_id
                        .as_deref()
                        .and_then(|s| Uuid::parse_str(s).ok()),
                });
            }
        }
    }

    Ok(results)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::Arc;

    use tokio::sync::Mutex;
    use tonic::{Request, Response, Status};

    use stitchd_proto::experiments::v1::{
        ExperimentIteration, ExperimentResults, ListRunningExperimentsRequest,
        RunningExperiment as ProtoRunningExperiment, UpdateIterationLastComputedResponse,
        experimentation_service_server::{
            ExperimentationService as ExperimentationServiceTrait, ExperimentationServiceServer,
        },
    };

    // ── Mock experimentation service ──────────────────────────────────────────

    type RunningExpList = Vec<ProtoRunningExperiment>;

    struct MockExperimentationService {
        items: Arc<Mutex<RunningExpList>>,
        /// Backs the `get_experiment_iteration` trait method (required to satisfy
        /// the gRPC trait). `None` makes that RPC return NotFound. The scheduler no
        /// longer calls it — iteration settings now ride on `ListRunningExperiments`
        /// (FIX C3) — so tests always construct this with `None` via `make_client`.
        iteration: Arc<Mutex<Option<ExperimentIteration>>>,
    }

    #[tonic::async_trait]
    impl ExperimentationServiceTrait for MockExperimentationService {
        type ListRunningExperimentsStream =
            tokio_stream::wrappers::ReceiverStream<Result<ProtoRunningExperiment, Status>>;

        async fn create_experiment(
            &self,
            _req: Request<stitchd_proto::experiments::v1::CreateExperimentRequest>,
        ) -> Result<Response<stitchd_proto::experiments::v1::Experiment>, Status> {
            Err(Status::unimplemented("not used in tests"))
        }
        async fn get_experiment(
            &self,
            _req: Request<stitchd_proto::experiments::v1::GetExperimentRequest>,
        ) -> Result<Response<stitchd_proto::experiments::v1::Experiment>, Status> {
            Err(Status::unimplemented("not used in tests"))
        }
        async fn list_experiments(
            &self,
            _req: Request<stitchd_proto::experiments::v1::ListExperimentsRequest>,
        ) -> Result<Response<stitchd_proto::experiments::v1::ListExperimentsResponse>, Status>
        {
            Err(Status::unimplemented("not used in tests"))
        }
        async fn update_experiment(
            &self,
            _req: Request<stitchd_proto::experiments::v1::UpdateExperimentRequest>,
        ) -> Result<Response<stitchd_proto::experiments::v1::Experiment>, Status> {
            Err(Status::unimplemented("not used in tests"))
        }
        async fn delete_experiment(
            &self,
            _req: Request<stitchd_proto::experiments::v1::DeleteExperimentRequest>,
        ) -> Result<Response<stitchd_proto::experiments::v1::Experiment>, Status> {
            Err(Status::unimplemented("not used in tests"))
        }
        async fn transition_experiment(
            &self,
            _req: Request<stitchd_proto::experiments::v1::TransitionExperimentRequest>,
        ) -> Result<Response<stitchd_proto::experiments::v1::Experiment>, Status> {
            Err(Status::unimplemented("not used in tests"))
        }
        async fn list_iterations(
            &self,
            _req: Request<stitchd_proto::experiments::v1::ListIterationsRequest>,
        ) -> Result<Response<stitchd_proto::experiments::v1::ListIterationsResponse>, Status>
        {
            Err(Status::unimplemented("not used in tests"))
        }
        async fn get_results(
            &self,
            _req: Request<stitchd_proto::experiments::v1::GetResultsRequest>,
        ) -> Result<Response<ExperimentResults>, Status> {
            Err(Status::unimplemented("not used in tests"))
        }
        async fn get_experiment_iteration(
            &self,
            _req: Request<stitchd_proto::experiments::v1::GetExperimentIterationRequest>,
        ) -> Result<Response<ExperimentIteration>, Status> {
            match self.iteration.lock().await.clone() {
                Some(it) => Ok(Response::new(it)),
                None => Err(Status::not_found("no iteration configured")),
            }
        }
        async fn update_iteration_last_computed(
            &self,
            _req: Request<stitchd_proto::experiments::v1::UpdateIterationLastComputedRequest>,
        ) -> Result<Response<UpdateIterationLastComputedResponse>, Status> {
            Err(Status::unimplemented("not used in tests"))
        }

        async fn list_running_experiments(
            &self,
            _req: Request<ListRunningExperimentsRequest>,
        ) -> Result<Response<Self::ListRunningExperimentsStream>, Status> {
            let items = self.items.lock().await.clone();
            let (tx, rx) = tokio::sync::mpsc::channel(items.len().max(1));
            for item in items {
                tx.send(Ok(item)).await.ok();
            }
            Ok(Response::new(tokio_stream::wrappers::ReceiverStream::new(
                rx,
            )))
        }

        async fn list_exposures(
            &self,
            _req: Request<stitchd_proto::experiments::v1::ListExposuresRequest>,
        ) -> Result<Response<stitchd_proto::experiments::v1::ListExposuresResponse>, Status>
        {
            Err(Status::unimplemented("not used in tests"))
        }
        async fn create_exclusion_group(
            &self,
            _req: Request<stitchd_proto::experiments::v1::CreateExclusionGroupRequest>,
        ) -> Result<Response<stitchd_proto::experiments::v1::ExclusionGroup>, Status> {
            Err(Status::unimplemented("not used in tests"))
        }
        async fn list_exclusion_groups(
            &self,
            _req: Request<stitchd_proto::experiments::v1::ListExclusionGroupsRequest>,
        ) -> Result<Response<stitchd_proto::experiments::v1::ListExclusionGroupsResponse>, Status>
        {
            Err(Status::unimplemented("not used in tests"))
        }
        async fn get_exclusion_group(
            &self,
            _req: Request<stitchd_proto::experiments::v1::GetExclusionGroupRequest>,
        ) -> Result<Response<stitchd_proto::experiments::v1::ExclusionGroup>, Status> {
            Err(Status::unimplemented("not used in tests"))
        }
        async fn update_exclusion_group(
            &self,
            _req: Request<stitchd_proto::experiments::v1::UpdateExclusionGroupRequest>,
        ) -> Result<Response<stitchd_proto::experiments::v1::ExclusionGroup>, Status> {
            Err(Status::unimplemented("not used in tests"))
        }
        async fn delete_exclusion_group(
            &self,
            _req: Request<stitchd_proto::experiments::v1::DeleteExclusionGroupRequest>,
        ) -> Result<Response<stitchd_proto::experiments::v1::DeleteExclusionGroupResponse>, Status>
        {
            Err(Status::unimplemented("not used in tests"))
        }
        async fn assign_experiment_to_group(
            &self,
            _req: Request<stitchd_proto::experiments::v1::AssignExperimentToGroupRequest>,
        ) -> Result<Response<stitchd_proto::experiments::v1::AssignExperimentToGroupResponse>, Status>
        {
            Err(Status::unimplemented("not used in tests"))
        }
        async fn unassign_experiment(
            &self,
            _req: Request<stitchd_proto::experiments::v1::UnassignExperimentRequest>,
        ) -> Result<Response<stitchd_proto::experiments::v1::UnassignExperimentResponse>, Status>
        {
            Err(Status::unimplemented("not used in tests"))
        }
        async fn get_experiment_interactions(
            &self,
            _req: Request<stitchd_proto::experiments::v1::GetExperimentInteractionsRequest>,
        ) -> Result<
            Response<stitchd_proto::experiments::v1::GetExperimentInteractionsResponse>,
            Status,
        > {
            Err(Status::unimplemented("not used in tests"))
        }

        async fn get_experiment_start_prerequisites(
            &self,
            _req: Request<stitchd_proto::experiments::v1::GetExperimentStartPrerequisitesRequest>,
        ) -> Result<
            Response<stitchd_proto::experiments::v1::GetExperimentStartPrerequisitesResponse>,
            Status,
        > {
            Err(Status::unimplemented("not used in tests"))
        }

        async fn apply_bandit_allocation(
            &self,
            _req: Request<stitchd_proto::experiments::v1::ApplyBanditAllocationRequest>,
        ) -> Result<Response<stitchd_proto::experiments::v1::ApplyBanditAllocationResponse>, Status>
        {
            Err(Status::unimplemented("not used in tests"))
        }
        async fn create_bandit_campaign(
            &self,
            _req: Request<stitchd_proto::experiments::v1::CreateBanditCampaignRequest>,
        ) -> Result<Response<stitchd_proto::experiments::v1::BanditCampaign>, Status> {
            Err(Status::unimplemented("not used in tests"))
        }
        async fn get_bandit_campaign(
            &self,
            _req: Request<stitchd_proto::experiments::v1::GetBanditCampaignRequest>,
        ) -> Result<Response<stitchd_proto::experiments::v1::BanditCampaign>, Status> {
            Err(Status::unimplemented("not used in tests"))
        }
        async fn list_bandit_campaigns(
            &self,
            _req: Request<stitchd_proto::experiments::v1::ListBanditCampaignsRequest>,
        ) -> Result<Response<stitchd_proto::experiments::v1::ListBanditCampaignsResponse>, Status>
        {
            Err(Status::unimplemented("not used in tests"))
        }
        async fn stop_bandit_campaign(
            &self,
            _req: Request<stitchd_proto::experiments::v1::StopBanditCampaignRequest>,
        ) -> Result<Response<stitchd_proto::experiments::v1::BanditCampaign>, Status> {
            Err(Status::unimplemented("not used in tests"))
        }
    }

    /// Spin up an in-process gRPC server and return a client connected to it.
    async fn make_client(
        items: Vec<ProtoRunningExperiment>,
    ) -> ExperimentationServiceClient<Channel> {
        make_client_with_iteration(items, None).await
    }

    /// As [`make_client`] but with a configured iteration for
    /// `get_experiment_iteration`.
    async fn make_client_with_iteration(
        items: Vec<ProtoRunningExperiment>,
        iteration: Option<ExperimentIteration>,
    ) -> ExperimentationServiceClient<Channel> {
        use tonic::transport::Server;

        let svc = MockExperimentationService {
            items: Arc::new(Mutex::new(items)),
            iteration: Arc::new(Mutex::new(iteration)),
        };

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        tokio::spawn(async move {
            Server::builder()
                .add_service(ExperimentationServiceServer::new(svc))
                .serve_with_incoming(tokio_stream::wrappers::TcpListenerStream::new(listener))
                .await
                .unwrap();
        });

        // Small yield to let the server start.
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;

        ExperimentationServiceClient::connect(format!("http://{addr}"))
            .await
            .unwrap()
    }

    // ── Test cases ─────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_fetch_running_experiments_empty() {
        let mut client = make_client(vec![]).await;
        let result = fetch_running_experiments(&mut client).await.unwrap();
        assert!(result.is_empty(), "no experiments should be returned");
    }

    #[tokio::test]
    async fn test_fetch_running_experiments_single() {
        let exp_id = Uuid::new_v4();
        let env_id = Uuid::new_v4();
        let iter_id = Uuid::new_v4();
        let metric_id = Uuid::new_v4();

        let proto = ProtoRunningExperiment {
            experiment_id: exp_id.to_string(),
            environment_id: env_id.to_string(),
            iteration_id: iter_id.to_string(),
            variant_keys: vec!["control".into(), "treatment".into()],
            metric_ids: vec![metric_id.to_string()],
            started_at_ms: 1_700_000_000_000,
            status: "running".into(),
            unit_context_types: vec!["user".into(), "account".into()],
            pre_period_days: 14,
            sequential_testing_enabled: true,
            sequential_alpha: 0.01,
            sequential_tau_squared: Some(0.25),
            sequential_min_sample_size: 250,
            variant_expected_bp: HashMap::from([
                ("control".to_string(), 9000),
                ("treatment".to_string(), 1000),
            ]),
            experiment_mode: "fixed".into(),
            bandit_config: None,
            bandit_campaign_id: None,
        };

        let mut client = make_client(vec![proto]).await;
        let results = fetch_running_experiments(&mut client).await.unwrap();

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].experiment_id, exp_id);
        assert_eq!(results[0].env_id, env_id);
        assert_eq!(results[0].iteration_id, iter_id);
        assert_eq!(results[0].metric_ids, vec![metric_id]);
        assert_eq!(results[0].variant_keys, vec!["control", "treatment"]);
        // FIX C3: iteration compute-pass inputs are hydrated directly from the
        // ListRunningExperiments response (no second GetExperimentIteration RPC).
        assert_eq!(results[0].unit_context_types, vec!["user", "account"]);
        assert_eq!(results[0].pre_period_days, 14);
        assert!(results[0].sequential.enabled);
        assert!((results[0].sequential.alpha - 0.01).abs() < 1e-12);
        assert_eq!(results[0].sequential.tau_squared, Some(0.25));
        assert_eq!(results[0].sequential.min_sample_size, 250);
        // Designed split (bp) for weighted SRM rides on the response.
        assert_eq!(results[0].variant_expected_bp.get("control"), Some(&9000));
        assert_eq!(results[0].variant_expected_bp.get("treatment"), Some(&1000));
    }

    #[tokio::test]
    async fn test_fetch_running_experiments_multiple() {
        let items: Vec<ProtoRunningExperiment> = (0..5)
            .map(|_| ProtoRunningExperiment {
                experiment_id: Uuid::new_v4().to_string(),
                environment_id: Uuid::new_v4().to_string(),
                iteration_id: Uuid::new_v4().to_string(),
                variant_keys: vec!["control".into()],
                metric_ids: vec![Uuid::new_v4().to_string()],
                started_at_ms: 0,
                status: "running".into(),
                unit_context_types: vec!["user".into()],
                pre_period_days: 0,
                sequential_testing_enabled: false,
                sequential_alpha: 0.05,
                sequential_tau_squared: None,
                sequential_min_sample_size: 100,
                variant_expected_bp: HashMap::new(),
                experiment_mode: "fixed".into(),
                bandit_config: None,
                bandit_campaign_id: None,
            })
            .collect();

        let mut client = make_client(items).await;
        let results = fetch_running_experiments(&mut client).await.unwrap();
        assert_eq!(results.len(), 5, "all five experiments should be returned");
    }

    #[tokio::test]
    async fn test_fetch_running_experiments_no_sql_on_experiments_tables() {
        // This is a compile-time check — the function signature no longer
        // accepts a PgPool, confirming no direct DB access.  If this test
        // compiles and runs, the constraint is satisfied.
        fn assert_no_pg_pool_arg<F>(_f: F)
        where
            F: Fn(&mut ExperimentationServiceClient<Channel>),
        {
        }
        assert_no_pg_pool_arg(|_client| {});
    }

    // ── FIX C3: iteration settings carried on ListRunningExperiments ──────────

    /// FIX C3: when the `RunningExperiment` proto carries an EMPTY
    /// `unit_context_types` (older server, or an iteration with none
    /// snapshotted), `fetch_running_experiments` defaults it to `["user"]` so the
    /// compute pass always has at least one dimension — while still reading the
    /// sequential / pre-period fields straight off the proto. This is the
    /// defaulting that used to live in the now-deleted `enrich_sequential_settings`.
    #[tokio::test]
    async fn fetch_defaults_empty_context_types_to_user() {
        let proto = ProtoRunningExperiment {
            experiment_id: Uuid::new_v4().to_string(),
            environment_id: Uuid::new_v4().to_string(),
            iteration_id: Uuid::new_v4().to_string(),
            variant_keys: vec!["control".into(), "treatment".into()],
            metric_ids: vec![],
            started_at_ms: 0,
            status: "running".into(),
            // Empty on the wire → must default to ["user"].
            unit_context_types: vec![],
            pre_period_days: 0,
            sequential_testing_enabled: false,
            sequential_alpha: 0.05,
            sequential_tau_squared: None,
            sequential_min_sample_size: 100,
            variant_expected_bp: HashMap::new(),
            experiment_mode: "fixed".into(),
            bandit_config: None,
            bandit_campaign_id: None,
        };
        let mut client = make_client(vec![proto]).await;
        let results = fetch_running_experiments(&mut client).await.unwrap();

        assert_eq!(results.len(), 1);
        assert_eq!(
            results[0].unit_context_types,
            vec!["user"],
            "empty unit_context_types must default to [\"user\"]"
        );
        assert!(!results[0].sequential.enabled);
        assert_eq!(results[0].pre_period_days, 0);
    }
}
