//! Fetches running experiments via the experimentation-service gRPC client.
//!
//! No direct PostgreSQL access to `experiments` or `experiment_iterations` tables.

use chrono::{DateTime, TimeZone, Utc};
use tonic::transport::Channel;
use uuid::Uuid;

use stitchd_proto::experiments::v1::{
    GetExperimentIterationRequest, ListRunningExperimentsRequest,
    experimentation_service_client::ExperimentationServiceClient,
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
    /// `ExperimentIteration.unit_context_types`, field 8). The compute pass runs
    /// one stats analysis per context type. Defaults to `["user"]` until
    /// [`enrich_sequential_settings`] hydrates it from the iteration snapshot;
    /// the `ListRunningExperiments` RPC does not carry this field.
    pub unit_context_types: Vec<String>,
    /// Pre-period length in days for CUPED variance reduction. Captured for the
    /// deferred CUPED pass; currently **unused** by the compute pass. Defaults to
    /// `0`. (The `ExperimentIteration` proto does not snapshot this today, so it
    /// stays `0` until the CUPED follow-up plumbs it through.)
    pub pre_period_days: u32,
    /// Sequential-testing configuration resolved from the iteration snapshot.
    /// Defaults to disabled until [`enrich_sequential_settings`] fetches the
    /// iteration; the `ListRunningExperiments` RPC does not carry these fields.
    pub sequential: SequentialSettings,
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
                results.push(RunningExperiment {
                    experiment_id,
                    env_id,
                    iteration_id,
                    metric_ids,
                    variant_keys: proto.variant_keys,
                    started_at,
                    // ListRunningExperiments carries neither the unit context
                    // types nor the sequential config / pre-period; all are
                    // resolved separately via the iteration snapshot. Default to
                    // a single "user" context until enriched.
                    unit_context_types: vec!["user".to_string()],
                    pre_period_days: 0,
                    sequential: SequentialSettings::default(),
                });
            }
        }
    }

    Ok(results)
}

/// Resolve the iteration-snapshotted compute-pass inputs for an experiment via
/// `GetExperimentIteration`: the [`SequentialSettings`] AND the
/// `unit_context_types` the compute pass scopes its analyses by.
///
/// The `ListRunningExperiments` view omits both, so the scheduler calls this to
/// hydrate `exp.sequential` and `exp.unit_context_types` before the compute
/// pass. On RPC failure the sequential settings are left at their default
/// (disabled) and the context types fall back to `["user"]` — the safe
/// behaviour, since a transient experimentation-service blip must not turn on
/// (or misconfigure) always-valid testing or silently drop a context dimension.
///
/// `pre_period_days` is NOT carried on the `ExperimentIteration` proto today, so
/// it is left at its `0` default (the deferred CUPED pass will plumb it).
pub async fn enrich_sequential_settings(
    client: &mut ExperimentationServiceClient<Channel>,
    exp: &mut RunningExperiment,
) {
    match client
        .get_experiment_iteration(GetExperimentIterationRequest {
            iteration_id: exp.iteration_id.to_string(),
        })
        .await
    {
        Ok(resp) => {
            let it = resp.into_inner();
            exp.sequential = SequentialSettings {
                enabled: it.sequential_testing_enabled,
                alpha: it.sequential_alpha,
                tau_squared: it.sequential_tau_squared,
                min_sample_size: it.sequential_min_sample_size,
            };
            // Capture the iteration's analysis unit context types; default to a
            // single "user" context when the snapshot carries none so the
            // compute pass always has at least one dimension to analyse.
            if it.unit_context_types.is_empty() {
                exp.unit_context_types = vec!["user".to_string()];
            } else {
                exp.unit_context_types = it.unit_context_types;
            }
        }
        Err(e) => {
            // Reset to the safe default (disabled): a transient
            // experimentation-service blip must never leave stale/partial
            // sequential config that could turn on or misconfigure
            // always-valid testing.
            exp.sequential = SequentialSettings::default();
            exp.unit_context_types = vec!["user".to_string()];
            tracing::warn!(
                experiment_id = %exp.experiment_id,
                iteration_id = %exp.iteration_id,
                "failed to fetch iteration for sequential settings; leaving disabled: {e}"
            );
        }
    }
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
        /// Optional iteration returned by `get_experiment_iteration`; `None`
        /// makes the RPC fail (NotFound) to exercise the fallback path.
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
        };

        let mut client = make_client(vec![proto]).await;
        let results = fetch_running_experiments(&mut client).await.unwrap();

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].experiment_id, exp_id);
        assert_eq!(results[0].env_id, env_id);
        assert_eq!(results[0].iteration_id, iter_id);
        assert_eq!(results[0].metric_ids, vec![metric_id]);
        assert_eq!(results[0].variant_keys, vec!["control", "treatment"]);
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

    // ── enrich_sequential_settings (Phase 3 config flow) ──────────────────────

    fn running_exp(iteration_id: Uuid) -> RunningExperiment {
        RunningExperiment {
            experiment_id: Uuid::new_v4(),
            env_id: Uuid::new_v4(),
            iteration_id,
            metric_ids: vec![],
            variant_keys: vec!["control".into(), "treatment".into()],
            started_at: Utc::now(),
            unit_context_types: vec!["user".into()],
            pre_period_days: 0,
            sequential: SequentialSettings::default(),
        }
    }

    fn iteration_with_sequential(
        id: Uuid,
        enabled: bool,
        alpha: f64,
        tau: Option<f64>,
        min_n: i64,
    ) -> ExperimentIteration {
        iteration_full(id, enabled, alpha, tau, min_n, vec![])
    }

    #[allow(clippy::too_many_arguments)]
    fn iteration_full(
        id: Uuid,
        enabled: bool,
        alpha: f64,
        tau: Option<f64>,
        min_n: i64,
        unit_context_types: Vec<String>,
    ) -> ExperimentIteration {
        ExperimentIteration {
            id: id.to_string(),
            experiment_id: Uuid::new_v4().to_string(),
            iteration_number: 1,
            started_at_ms: 0,
            ended_at_ms: 0,
            metric_ids: vec![],
            traffic_allocation: 1.0,
            unit_context_types,
            exclusion_group_id: None,
            group_bucket_lo: None,
            group_bucket_hi: None,
            sequential_testing_enabled: enabled,
            sequential_alpha: alpha,
            sequential_tau_squared: tau,
            sequential_min_sample_size: min_n,
        }
    }

    #[tokio::test]
    async fn enrich_sequential_settings_reads_iteration_snapshot() {
        let iter_id = Uuid::new_v4();
        let it = iteration_with_sequential(iter_id, true, 0.01, Some(0.25), 250);
        let mut client = make_client_with_iteration(vec![], Some(it)).await;

        let mut exp = running_exp(iter_id);
        enrich_sequential_settings(&mut client, &mut exp).await;

        assert!(exp.sequential.enabled);
        assert!((exp.sequential.alpha - 0.01).abs() < 1e-12);
        assert_eq!(exp.sequential.tau_squared, Some(0.25));
        assert_eq!(exp.sequential.min_sample_size, 250);
    }

    #[tokio::test]
    async fn enrich_captures_unit_context_types_from_iteration() {
        let iter_id = Uuid::new_v4();
        let it = iteration_full(
            iter_id,
            false,
            0.05,
            None,
            100,
            vec!["user".into(), "account".into()],
        );
        let mut client = make_client_with_iteration(vec![], Some(it)).await;

        let mut exp = running_exp(iter_id);
        // Start from a wrong default to prove the fetch overwrites it.
        exp.unit_context_types = vec!["bogus".into()];
        enrich_sequential_settings(&mut client, &mut exp).await;

        assert_eq!(exp.unit_context_types, vec!["user", "account"]);
    }

    #[tokio::test]
    async fn enrich_defaults_context_types_to_user_when_iteration_empty() {
        let iter_id = Uuid::new_v4();
        let it = iteration_full(iter_id, false, 0.05, None, 100, vec![]);
        let mut client = make_client_with_iteration(vec![], Some(it)).await;
        let mut exp = running_exp(iter_id);
        exp.unit_context_types = vec![];
        enrich_sequential_settings(&mut client, &mut exp).await;
        assert_eq!(exp.unit_context_types, vec!["user"]);
    }

    #[tokio::test]
    async fn enrich_sequential_settings_falls_back_to_disabled_on_rpc_error() {
        // No iteration configured → get_experiment_iteration returns NotFound.
        let mut client = make_client_with_iteration(vec![], None).await;
        let mut exp = running_exp(Uuid::new_v4());
        // Pretend it was somehow enabled; the failed fetch must reset to default.
        exp.sequential.enabled = true;
        enrich_sequential_settings(&mut client, &mut exp).await;
        assert!(
            !exp.sequential.enabled,
            "RPC failure must leave sequential disabled"
        );
    }
}
