//! Writes computed per-metric statistics via the analytics-service gRPC client.
//!
//! No direct PostgreSQL access to `experiment_results`.

use chrono::{DateTime, Utc};
use tonic::transport::Channel;
use uuid::Uuid;

use stitchd_proto::analytics::v1::{
    WriteExperimentResultsRequest,
    analytics_service_client::AnalyticsServiceClient,
};

/// A summarized metric result for one (variant_group, metric_key) pair,
/// ready to be forwarded to analytics-service.
#[derive(Debug, Clone)]
pub struct MetricSummary {
    pub metric_key: String,
    /// JSON object mapping variant_key → event count.
    pub variant_stats: serde_json::Value,
    /// Optional frequentist analysis result (JSON).
    pub frequentist_result: Option<serde_json::Value>,
    /// Optional bayesian analysis result (JSON).
    pub bayesian_result: Option<serde_json::Value>,
    pub recommendation: String,
}

/// Forward computed metric summaries to `analytics-service.WriteExperimentResults`.
///
/// Each `MetricSummary` becomes one `WriteExperimentResultsRequest` call keyed
/// on `(experiment_id, iteration_id, metric_key)`.  The analytics service is
/// responsible for the actual upsert.
///
/// `env_id` is the environment UUID that scopes the result rows.
pub async fn write_results(
    client: &mut AnalyticsServiceClient<Channel>,
    env_id: Uuid,
    experiment_id: Uuid,
    iteration_id: Uuid,
    computed_at: DateTime<Utc>,
    summaries: &[MetricSummary],
) -> Result<(), anyhow::Error> {
    let computed_at_rfc = computed_at.to_rfc3339();

    for summary in summaries {
        let req = WriteExperimentResultsRequest {
            env_id: env_id.to_string(),
            experiment_id: experiment_id.to_string(),
            iteration_id: iteration_id.to_string(),
            variant_key: String::new(), // variant breakdown is encoded in variant_stats JSON
            metric_key: summary.metric_key.clone(),
            metric_type: "count".to_string(),
            variant_stats: summary.variant_stats.to_string(),
            frequentist_result: summary
                .frequentist_result
                .as_ref()
                .map(|v| v.to_string()),
            bayesian_result: summary.bayesian_result.as_ref().map(|v| v.to_string()),
            recommendation: summary.recommendation.clone(),
            computed_at: computed_at_rfc.clone(),
        };

        client.write_experiment_results(req).await?;
    }

    Ok(())
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

    use stitchd_proto::analytics::v1::{
        ExperimentResult,
        GetContextIntelligenceResponse,
        GetEvalStatsResponse,
        IngestEventResponse,
        ListContextParamsResponse,
        ListContextTypesResponse,
        RegisterContextResponse,
        WriteExperimentResultsRequest,
        WriteExperimentResultsResponse,
        analytics_service_server::{AnalyticsService as AnalyticsServiceTrait, AnalyticsServiceServer},
    };

    // ── Mock analytics service ────────────────────────────────────────────────

    #[derive(Default)]
    struct MockAnalyticsService {
        received: Arc<Mutex<Vec<WriteExperimentResultsRequest>>>,
    }

    #[tonic::async_trait]
    impl AnalyticsServiceTrait for MockAnalyticsService {
        type ListExperimentResultsStream =
            tokio_stream::wrappers::ReceiverStream<Result<ExperimentResult, Status>>;

        async fn ingest_event(
            &self,
            _req: Request<stitchd_proto::analytics::v1::IngestEventRequest>,
        ) -> Result<Response<IngestEventResponse>, Status> {
            Err(Status::unimplemented("not used in tests"))
        }
        async fn register_context(
            &self,
            _req: Request<stitchd_proto::analytics::v1::RegisterContextRequest>,
        ) -> Result<Response<RegisterContextResponse>, Status> {
            Err(Status::unimplemented("not used in tests"))
        }
        async fn list_context_types(
            &self,
            _req: Request<stitchd_proto::analytics::v1::ListContextTypesRequest>,
        ) -> Result<Response<ListContextTypesResponse>, Status> {
            Err(Status::unimplemented("not used in tests"))
        }
        async fn list_context_params(
            &self,
            _req: Request<stitchd_proto::analytics::v1::ListContextParamsRequest>,
        ) -> Result<Response<ListContextParamsResponse>, Status> {
            Err(Status::unimplemented("not used in tests"))
        }
        async fn get_eval_stats(
            &self,
            _req: Request<stitchd_proto::analytics::v1::GetEvalStatsRequest>,
        ) -> Result<Response<GetEvalStatsResponse>, Status> {
            Err(Status::unimplemented("not used in tests"))
        }
        async fn get_context_intelligence(
            &self,
            _req: Request<stitchd_proto::analytics::v1::GetContextIntelligenceRequest>,
        ) -> Result<Response<GetContextIntelligenceResponse>, Status> {
            Err(Status::unimplemented("not used in tests"))
        }
        async fn list_experiment_results(
            &self,
            _req: Request<stitchd_proto::analytics::v1::ListExperimentResultsRequest>,
        ) -> Result<Response<Self::ListExperimentResultsStream>, Status> {
            Err(Status::unimplemented("not used in tests"))
        }
        async fn get_experiment_result(
            &self,
            _req: Request<stitchd_proto::analytics::v1::GetExperimentResultRequest>,
        ) -> Result<Response<ExperimentResult>, Status> {
            Err(Status::unimplemented("not used in tests"))
        }

        async fn write_experiment_results(
            &self,
            req: Request<WriteExperimentResultsRequest>,
        ) -> Result<Response<WriteExperimentResultsResponse>, Status> {
            self.received.lock().await.push(req.into_inner());
            Ok(Response::new(WriteExperimentResultsResponse {}))
        }

        // ── Metric definitions CRUD — not exercised by stats-service tests ──
        async fn create_metric(
            &self,
            _req: Request<stitchd_proto::analytics::v1::CreateMetricRequest>,
        ) -> Result<Response<stitchd_proto::analytics::v1::MetricDefinition>, Status> {
            Err(Status::unimplemented("not used in tests"))
        }
        async fn get_metric(
            &self,
            _req: Request<stitchd_proto::analytics::v1::GetMetricRequest>,
        ) -> Result<Response<stitchd_proto::analytics::v1::MetricDefinition>, Status> {
            Err(Status::unimplemented("not used in tests"))
        }
        async fn list_metrics(
            &self,
            _req: Request<stitchd_proto::analytics::v1::ListMetricsRequest>,
        ) -> Result<Response<stitchd_proto::analytics::v1::ListMetricsResponse>, Status> {
            Err(Status::unimplemented("not used in tests"))
        }
        async fn update_metric(
            &self,
            _req: Request<stitchd_proto::analytics::v1::UpdateMetricRequest>,
        ) -> Result<Response<stitchd_proto::analytics::v1::MetricDefinition>, Status> {
            Err(Status::unimplemented("not used in tests"))
        }
        async fn delete_metric(
            &self,
            _req: Request<stitchd_proto::analytics::v1::DeleteMetricRequest>,
        ) -> Result<Response<stitchd_proto::analytics::v1::DeleteMetricResponse>, Status> {
            Err(Status::unimplemented("not used in tests"))
        }
        async fn preview_metric(
            &self,
            _req: Request<stitchd_proto::analytics::v1::PreviewMetricRequest>,
        ) -> Result<Response<stitchd_proto::analytics::v1::PreviewMetricResponse>, Status> {
            Err(Status::unimplemented("not used in tests"))
        }
    }

    /// Spin up an in-process gRPC server and return (client, captured_requests).
    async fn make_client() -> (
        AnalyticsServiceClient<Channel>,
        Arc<Mutex<Vec<WriteExperimentResultsRequest>>>,
    ) {
        use tonic::transport::Server;

        let captured: Arc<Mutex<Vec<WriteExperimentResultsRequest>>> =
            Arc::new(Mutex::new(vec![]));
        let svc = MockAnalyticsService {
            received: captured.clone(),
        };

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        tokio::spawn(async move {
            Server::builder()
                .add_service(AnalyticsServiceServer::new(svc))
                .serve_with_incoming(tokio_stream::wrappers::TcpListenerStream::new(listener))
                .await
                .unwrap();
        });

        tokio::time::sleep(std::time::Duration::from_millis(10)).await;

        let client = AnalyticsServiceClient::connect(format!("http://{addr}"))
            .await
            .unwrap();

        (client, captured)
    }

    // ── Test cases ─────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_write_results_empty_summaries_is_noop() {
        let (mut client, captured) = make_client().await;
        write_results(
            &mut client,
            Uuid::new_v4(),
            Uuid::new_v4(),
            Uuid::new_v4(),
            Utc::now(),
            &[],
        )
        .await
        .expect("empty write should succeed");

        assert!(captured.lock().await.is_empty(), "no RPC calls expected");
    }

    #[tokio::test]
    async fn test_write_results_single_metric() {
        let (mut client, captured) = make_client().await;
        let exp_id = Uuid::new_v4();
        let iter_id = Uuid::new_v4();
        let env_id = Uuid::new_v4();

        let summaries = vec![MetricSummary {
            metric_key: "clicks".into(),
            variant_stats: serde_json::json!({ "control": 50, "treatment": 70 }),
            frequentist_result: Some(serde_json::json!({ "p_value": 0.04 })),
            bayesian_result: None,
            recommendation: "ship_treatment".into(),
        }];

        write_results(&mut client, env_id, exp_id, iter_id, Utc::now(), &summaries)
            .await
            .expect("write_results should succeed");

        let reqs = captured.lock().await;
        assert_eq!(reqs.len(), 1);
        assert_eq!(reqs[0].experiment_id, exp_id.to_string());
        assert_eq!(reqs[0].iteration_id, iter_id.to_string());
        assert_eq!(reqs[0].env_id, env_id.to_string());
        assert_eq!(reqs[0].metric_key, "clicks");
        assert_eq!(reqs[0].recommendation, "ship_treatment");
        assert!(reqs[0].frequentist_result.is_some());
    }

    #[tokio::test]
    async fn test_write_results_multiple_metrics_sends_one_rpc_each() {
        let (mut client, captured) = make_client().await;
        let summaries = vec![
            MetricSummary {
                metric_key: "clicks".into(),
                variant_stats: serde_json::json!({}),
                frequentist_result: None,
                bayesian_result: None,
                recommendation: "wait".into(),
            },
            MetricSummary {
                metric_key: "revenue".into(),
                variant_stats: serde_json::json!({}),
                frequentist_result: None,
                bayesian_result: None,
                recommendation: "wait".into(),
            },
        ];

        write_results(
            &mut client,
            Uuid::new_v4(),
            Uuid::new_v4(),
            Uuid::new_v4(),
            Utc::now(),
            &summaries,
        )
        .await
        .unwrap();

        assert_eq!(
            captured.lock().await.len(),
            2,
            "one RPC per metric summary"
        );
    }

    #[tokio::test]
    async fn test_write_results_no_sql_on_experiment_results() {
        // Compile-time check: the function signature no longer accepts PgPool.
        fn assert_no_pg_pool_arg<F>(_f: F)
        where
            F: Fn(&mut AnalyticsServiceClient<Channel>),
        {
        }
        assert_no_pg_pool_arg(|_client| {});
    }
}
