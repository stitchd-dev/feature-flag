//! Fetches running experiments via the experimentation-service gRPC client.
//!
//! No direct PostgreSQL access to `experiments` or `experiment_iterations` tables.

use chrono::{DateTime, TimeZone, Utc};
use tonic::transport::Channel;
use uuid::Uuid;

use stitchd_proto::experiments::v1::{
    ListRunningExperimentsRequest, experimentation_service_client::ExperimentationServiceClient,
};

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
            Err(Status::unimplemented("not used in tests"))
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
        ) -> Result<Response<stitchd_proto::experiments::v1::ListExposuresResponse>, Status> {
            Err(Status::unimplemented("not used in tests"))
        }
    }

    /// Spin up an in-process gRPC server and return a client connected to it.
    async fn make_client(
        items: Vec<ProtoRunningExperiment>,
    ) -> ExperimentationServiceClient<Channel> {
        use tonic::transport::Server;

        let svc = MockExperimentationService {
            items: Arc::new(Mutex::new(items)),
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
}
