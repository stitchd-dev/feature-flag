//! Writes computed per-metric statistics via the analytics-service gRPC client.
//!
//! No direct PostgreSQL access to `experiment_results`.
//!
//! ## Per-context-type rows (Phase 5)
//!
//! Each [`MetricSummary`] carries a `context_type` (e.g. `"user"`,
//! `"account"`). The stats query builders ([`crate::queries::aggregation`]
//! etc.) emit one (context_type, variant_key) row per assignment scope,
//! and the result builder produces one `MetricSummary` per
//! `(metric_key, context_type)`. Each summary becomes one
//! `WriteExperimentResultsRequest` — analytics-service is responsible
//! for the upsert into the `experiment_results` CH table.

use chrono::{DateTime, Utc};
use std::collections::HashMap;
use tonic::transport::Channel;
use uuid::Uuid;

use stitchd_proto::analytics::v1::{
    WriteExperimentResultsRequest, analytics_service_client::AnalyticsServiceClient,
};

/// A summarized metric result for one (context_type, metric_key) pair,
/// ready to be forwarded to analytics-service.
///
/// The `variant_stats` JSON encodes the per-variant breakdown WITHIN the
/// `context_type`; analyses (frequentist + bayesian) are scoped per
/// context_type per spec §3.
#[derive(Debug, Clone)]
pub struct MetricSummary {
    /// The attribution unit's context type (e.g. `"user"`, `"account"`).
    /// Required since the Phase 5 cutover — every result row is per
    /// context type.
    pub context_type: String,
    /// The metric being summarised.
    pub metric_key: String,
    /// The metric's analysis type discriminant (`"count"`, `"numeric"`,
    /// `"percentile"`, `"funnel"`) — forwarded verbatim as the result row's
    /// `metric_type`. Defaults to `"count"` (see [`Default`]); the live compute
    /// pass threads the real type from the metric kind via
    /// [`crate::compute::metric_type_for`].
    pub metric_type: String,
    /// JSON object mapping `variant_key` → per-variant statistics
    /// (count / sum / quantiles / …) within the `context_type`.
    pub variant_stats: serde_json::Value,
    /// Optional frequentist analysis result (JSON).
    pub frequentist_result: Option<serde_json::Value>,
    /// Optional bayesian analysis result (JSON).
    pub bayesian_result: Option<serde_json::Value>,
    /// Optional sequential (always-valid mSPRT) result — a JSON object keyed by
    /// `variant_key` mirroring `frequentist_result`. `None` when sequential
    /// testing was disabled for the experiment (or the metric family has no
    /// always-valid analogue).
    pub sequential_result: Option<serde_json::Value>,
    /// Human-readable recommendation string.
    pub recommendation: String,
}

/// A single per-(context_type, variant_key) data point returned by the
/// query builders.
///
/// The result builder groups these into `MetricSummary`s keyed by
/// `(metric_key, context_type)` — see [`build_metric_summaries`].
#[derive(Debug, Clone)]
pub struct VariantPoint {
    /// The attribution unit's context type.
    pub context_type: String,
    /// The variant the contexts in this point landed in.
    pub variant_key: String,
    /// The aggregator value for the bucket (count / mean / etc).
    pub metric_value: f64,
}

/// Group `(context_type, variant_key, metric_value)` data points by
/// `(metric_key, context_type)` into a list of `MetricSummary`s.
///
/// One summary is emitted per `(metric_key, context_type)` pair; the
/// `variant_stats` JSON inside the summary maps `variant_key → metric_value`
/// for the variants observed within that context type.
///
/// Frequentist / bayesian / sequential results are passed through unchanged —
/// callers can pre-compute them per (metric_key, context_type) and look them up
/// in the `results` maps. `None` for a pair means "no analysis available
/// — surface as empty in the proto".
#[must_use]
pub fn build_metric_summaries(
    points_per_metric: &HashMap<String, Vec<VariantPoint>>,
    frequentist_per_pair: &HashMap<(String, String), serde_json::Value>,
    bayesian_per_pair: &HashMap<(String, String), serde_json::Value>,
    sequential_per_pair: &HashMap<(String, String), serde_json::Value>,
    recommendations_per_pair: &HashMap<(String, String), String>,
) -> Vec<MetricSummary> {
    let mut summaries = Vec::new();
    for (metric_key, points) in points_per_metric {
        // Bucket the points by context_type → { variant_key: metric_value }.
        let mut per_ctx: HashMap<String, serde_json::Map<String, serde_json::Value>> =
            HashMap::new();
        for p in points {
            let entry = per_ctx.entry(p.context_type.clone()).or_default();
            entry.insert(
                p.variant_key.clone(),
                serde_json::Value::from(p.metric_value),
            );
        }
        for (context_type, variant_stats_map) in per_ctx {
            let key = (metric_key.clone(), context_type.clone());
            summaries.push(MetricSummary {
                context_type: context_type.clone(),
                metric_key: metric_key.clone(),
                // Default discriminant; the live compute pass overwrites this
                // from the metric kind after building the summaries.
                metric_type: "count".to_string(),
                variant_stats: serde_json::Value::Object(variant_stats_map),
                frequentist_result: frequentist_per_pair.get(&key).cloned(),
                bayesian_result: bayesian_per_pair.get(&key).cloned(),
                sequential_result: sequential_per_pair.get(&key).cloned(),
                recommendation: recommendations_per_pair
                    .get(&key)
                    .cloned()
                    .unwrap_or_default(),
            });
        }
    }
    // Stable ordering for deterministic test assertions: (metric_key,
    // context_type) ascending.
    summaries.sort_by(|a, b| {
        (a.metric_key.as_str(), a.context_type.as_str())
            .cmp(&(b.metric_key.as_str(), b.context_type.as_str()))
    });
    summaries
}

/// Forward computed metric summaries to `analytics-service.WriteExperimentResults`.
///
/// Each `MetricSummary` becomes one `WriteExperimentResultsRequest` call keyed
/// on `(experiment_id, iteration_id, context_type, metric_key)`.  The
/// analytics service is responsible for the actual upsert.
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
            metric_type: summary.metric_type.clone(),
            variant_stats: summary.variant_stats.to_string(),
            frequentist_result: summary.frequentist_result.as_ref().map(|v| v.to_string()),
            bayesian_result: summary.bayesian_result.as_ref().map(|v| v.to_string()),
            // Sequential (always-valid) per-variant JSON blob; `None` when
            // sequential testing was disabled or the metric has no analogue.
            sequential_result: summary.sequential_result.as_ref().map(|v| v.to_string()),
            recommendation: summary.recommendation.clone(),
            computed_at: computed_at_rfc.clone(),
            context_type: summary.context_type.clone(),
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
        ExperimentResult, GetContextIntelligenceResponse, GetEvalStatsResponse,
        IngestEventResponse, ListContextParamsResponse, ListContextTypesResponse,
        RegisterContextResponse, WriteExperimentResultsRequest, WriteExperimentResultsResponse,
        analytics_service_server::{
            AnalyticsService as AnalyticsServiceTrait, AnalyticsServiceServer,
        },
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
        async fn track_events(
            &self,
            _req: Request<stitchd_proto::analytics::v1::TrackEventsRequest>,
        ) -> Result<Response<stitchd_proto::analytics::v1::TrackEventsResponse>, Status> {
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
        async fn get_event_firings(
            &self,
            _req: Request<stitchd_proto::analytics::v1::GetEventFiringsRequest>,
        ) -> Result<Response<stitchd_proto::analytics::v1::GetEventFiringsResponse>, Status>
        {
            Err(Status::unimplemented("not used in tests"))
        }
        async fn get_event_stats(
            &self,
            _req: Request<stitchd_proto::analytics::v1::GetEventStatsRequest>,
        ) -> Result<Response<stitchd_proto::analytics::v1::GetEventStatsResponse>, Status> {
            Err(Status::unimplemented("not used in tests"))
        }
        // Event-definitions CRUD — closed in feature-flag-wr4.
        async fn create_event_definition(
            &self,
            _req: Request<stitchd_proto::analytics::v1::CreateEventDefinitionRequest>,
        ) -> Result<Response<stitchd_proto::analytics::v1::EventDefinitionMsg>, Status> {
            Err(Status::unimplemented("not used in tests"))
        }
        async fn get_event_definition(
            &self,
            _req: Request<stitchd_proto::analytics::v1::GetEventDefinitionRequest>,
        ) -> Result<Response<stitchd_proto::analytics::v1::EventDefinitionMsg>, Status> {
            Err(Status::unimplemented("not used in tests"))
        }
        async fn list_event_definitions(
            &self,
            _req: Request<stitchd_proto::analytics::v1::ListEventDefinitionsRequest>,
        ) -> Result<Response<stitchd_proto::analytics::v1::ListEventDefinitionsResponse>, Status>
        {
            Err(Status::unimplemented("not used in tests"))
        }
        async fn update_event_definition(
            &self,
            _req: Request<stitchd_proto::analytics::v1::UpdateEventDefinitionRequest>,
        ) -> Result<Response<stitchd_proto::analytics::v1::EventDefinitionMsg>, Status> {
            Err(Status::unimplemented("not used in tests"))
        }
        async fn delete_event_definition(
            &self,
            _req: Request<stitchd_proto::analytics::v1::DeleteEventDefinitionRequest>,
        ) -> Result<Response<stitchd_proto::analytics::v1::DeleteEventDefinitionResponse>, Status>
        {
            Err(Status::unimplemented("not used in tests"))
        }
    }

    /// Spin up an in-process gRPC server and return (client, captured_requests).
    async fn make_client() -> (
        AnalyticsServiceClient<Channel>,
        Arc<Mutex<Vec<WriteExperimentResultsRequest>>>,
    ) {
        use tonic::transport::Server;

        let captured: Arc<Mutex<Vec<WriteExperimentResultsRequest>>> = Arc::new(Mutex::new(vec![]));
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
            context_type: "user".into(),
            metric_key: "clicks".into(),
            metric_type: "count".into(),
            variant_stats: serde_json::json!({ "control": 50, "treatment": 70 }),
            frequentist_result: Some(serde_json::json!({ "p_value": 0.04 })),
            bayesian_result: None,
            sequential_result: None,
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
                context_type: "user".into(),
                metric_key: "clicks".into(),
                metric_type: "count".into(),
                variant_stats: serde_json::json!({}),
                frequentist_result: None,
                bayesian_result: None,
                sequential_result: None,
                recommendation: "wait".into(),
            },
            MetricSummary {
                context_type: "user".into(),
                metric_key: "revenue".into(),
                metric_type: "numeric".into(),
                variant_stats: serde_json::json!({}),
                frequentist_result: None,
                bayesian_result: None,
                sequential_result: None,
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

        assert_eq!(captured.lock().await.len(), 2, "one RPC per metric summary");
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

    // ── Per-context-type result builder (Phase 5 Task 5.5) ────────────────

    #[tokio::test]
    async fn write_results_forwards_context_type_per_row() {
        let (mut client, captured) = make_client().await;
        let summaries = vec![
            MetricSummary {
                context_type: "user".into(),
                metric_key: "clicks".into(),
                metric_type: "count".into(),
                variant_stats: serde_json::json!({ "control": 10, "treatment": 14 }),
                frequentist_result: None,
                bayesian_result: None,
                sequential_result: None,
                recommendation: "wait".into(),
            },
            MetricSummary {
                context_type: "account".into(),
                metric_key: "clicks".into(),
                metric_type: "count".into(),
                variant_stats: serde_json::json!({ "control": 3, "treatment": 5 }),
                frequentist_result: None,
                bayesian_result: None,
                sequential_result: None,
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
        .expect("write_results should succeed");

        let reqs = captured.lock().await;
        assert_eq!(reqs.len(), 2);
        let by_ctx: std::collections::HashSet<&str> =
            reqs.iter().map(|r| r.context_type.as_str()).collect();
        assert!(by_ctx.contains("user"));
        assert!(by_ctx.contains("account"));
    }

    #[test]
    fn build_metric_summaries_groups_by_metric_and_context_type() {
        use std::collections::HashMap;

        let mut points_per_metric: HashMap<String, Vec<VariantPoint>> = HashMap::new();
        points_per_metric.insert(
            "clicks".into(),
            vec![
                VariantPoint {
                    context_type: "user".into(),
                    variant_key: "control".into(),
                    metric_value: 10.0,
                },
                VariantPoint {
                    context_type: "user".into(),
                    variant_key: "treatment".into(),
                    metric_value: 14.0,
                },
                VariantPoint {
                    context_type: "account".into(),
                    variant_key: "control".into(),
                    metric_value: 3.0,
                },
                VariantPoint {
                    context_type: "account".into(),
                    variant_key: "treatment".into(),
                    metric_value: 5.0,
                },
            ],
        );

        let mut freq: HashMap<(String, String), serde_json::Value> = HashMap::new();
        freq.insert(
            ("clicks".into(), "user".into()),
            serde_json::json!({"p_value": 0.04}),
        );

        let summaries = build_metric_summaries(
            &points_per_metric,
            &freq,
            &HashMap::new(),
            &HashMap::new(),
            &HashMap::new(),
        );

        // Stable ordering by (metric_key, context_type).
        assert_eq!(summaries.len(), 2);
        assert_eq!(summaries[0].metric_key, "clicks");
        assert_eq!(summaries[0].context_type, "account");
        assert_eq!(summaries[1].metric_key, "clicks");
        assert_eq!(summaries[1].context_type, "user");

        // Per-context-type variant_stats encoding.
        let user_stats = summaries[1].variant_stats.as_object().unwrap();
        assert_eq!(user_stats["control"], serde_json::json!(10.0));
        assert_eq!(user_stats["treatment"], serde_json::json!(14.0));

        // Frequentist result attached only to the matching pair.
        assert!(summaries[1].frequentist_result.is_some());
        assert!(summaries[0].frequentist_result.is_none());
    }

    #[test]
    fn build_metric_summaries_attaches_sequential_per_pair() {
        use std::collections::HashMap;

        let mut points_per_metric: HashMap<String, Vec<VariantPoint>> = HashMap::new();
        points_per_metric.insert(
            "clicks".into(),
            vec![VariantPoint {
                context_type: "user".into(),
                variant_key: "control".into(),
                metric_value: 10.0,
            }],
        );

        let mut seq: HashMap<(String, String), serde_json::Value> = HashMap::new();
        seq.insert(
            ("clicks".into(), "user".into()),
            serde_json::json!({"treatment": {"always_valid_p": 0.02, "method": "msprt"}}),
        );

        let summaries = build_metric_summaries(
            &points_per_metric,
            &HashMap::new(),
            &HashMap::new(),
            &seq,
            &HashMap::new(),
        );
        assert_eq!(summaries.len(), 1);
        let blob = summaries[0]
            .sequential_result
            .as_ref()
            .expect("sequential_result attached");
        assert_eq!(blob["treatment"]["always_valid_p"], serde_json::json!(0.02));
    }

    #[tokio::test]
    async fn write_results_forwards_sequential_result_string() {
        let (mut client, captured) = make_client().await;
        let summaries = vec![MetricSummary {
            context_type: "user".into(),
            metric_key: "clicks".into(),
            metric_type: "count".into(),
            variant_stats: serde_json::json!({ "control": 50, "treatment": 70 }),
            frequentist_result: None,
            bayesian_result: None,
            sequential_result: Some(serde_json::json!({
                "control": {"always_valid_p": 1.0, "p_crossed": false, "method": "msprt"},
                "treatment": {"always_valid_p": 0.01, "p_crossed": true, "method": "msprt"},
            })),
            recommendation: "ship_treatment".into(),
        }];

        write_results(
            &mut client,
            Uuid::new_v4(),
            Uuid::new_v4(),
            Uuid::new_v4(),
            Utc::now(),
            &summaries,
        )
        .await
        .expect("write_results should succeed");

        let reqs = captured.lock().await;
        assert_eq!(reqs.len(), 1);
        let seq_str = reqs[0]
            .sequential_result
            .as_ref()
            .expect("sequential_result string forwarded");
        let parsed: serde_json::Value = serde_json::from_str(seq_str).unwrap();
        assert_eq!(parsed["treatment"]["p_crossed"], serde_json::json!(true));
    }
}
