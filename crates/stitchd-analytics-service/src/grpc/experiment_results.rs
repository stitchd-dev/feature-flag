//! gRPC handlers for ExperimentResults RPCs.
//!
//! # Trait boundary
//! Handlers depend on [`ExperimentResultsRepository`] from
//! `crate::repo::experiment_results` (Worker 3's canonical ClickHouse-backed
//! implementation).
//!
//! # Proto mapping
//! `ExperimentResultRow` (from Worker 3's repo) ↔ `ExperimentResult` (proto):
//! - JSON columns (`variant_stats`, `frequentist_result`, `bayesian_result`)
//!   are stored as raw JSON strings; empty string signals absent optional.
//! - `computed_at` / `created_at` are `i64` Unix-milliseconds in the repo;
//!   the proto carries RFC 3339 UTC strings.

use std::pin::Pin;
use std::sync::Arc;

use chrono::TimeZone as _;
use tokio_stream::{Stream, iter as stream_iter};
use tonic::{Request, Response, Status};

use stitchd_proto::analytics::v1::{
    ExperimentResult, GetExperimentResultRequest, ListExperimentResultsRequest,
    WriteExperimentResultsRequest, WriteExperimentResultsResponse,
};

use crate::repo::experiment_results::{
    ExperimentResultRow, ExperimentResultsRepository, RepoError, ResultsFilter, WriteResultRow,
};

// ---------------------------------------------------------------------------
// Proto conversion helpers
// ---------------------------------------------------------------------------

/// Convert a `chrono::DateTime<Utc>` from Unix milliseconds to RFC 3339.
fn ms_to_rfc3339(ms: i64) -> String {
    chrono::Utc
        .timestamp_millis_opt(ms)
        .single()
        .unwrap_or_else(chrono::Utc::now)
        .to_rfc3339()
}

fn row_to_proto(row: ExperimentResultRow) -> ExperimentResult {
    ExperimentResult {
        env_id: row.env_id.to_string(),
        experiment_id: row.experiment_id.to_string(),
        iteration_id: row.iteration_id.to_string(),
        variant_key: row.variant_key,
        metric_key: row.metric_key,
        metric_type: row.metric_type,
        variant_stats: row.variant_stats,
        frequentist_result: if row.frequentist_result.is_empty() {
            None
        } else {
            Some(row.frequentist_result)
        },
        bayesian_result: if row.bayesian_result.is_empty() {
            None
        } else {
            Some(row.bayesian_result)
        },
        sequential_result: if row.sequential_result.is_empty() {
            None
        } else {
            Some(row.sequential_result)
        },
        recommendation: row.recommendation,
        computed_at: ms_to_rfc3339(row.computed_at),
        created_at: ms_to_rfc3339(row.created_at),
        context_type: row.context_type,
    }
}

#[allow(clippy::result_large_err)]
fn parse_uuid(s: &str, field: &str) -> Result<uuid::Uuid, Status> {
    s.parse::<uuid::Uuid>()
        .map_err(|_| Status::invalid_argument(format!("invalid {field}: {s}")))
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// Handle `WriteExperimentResults` — writes one result row to ClickHouse.
#[allow(clippy::result_large_err)]
pub async fn handle_write_experiment_results(
    repo: &Arc<dyn ExperimentResultsRepository>,
    request: Request<WriteExperimentResultsRequest>,
) -> Result<Response<WriteExperimentResultsResponse>, Status> {
    let req = request.into_inner();

    let env_id = parse_uuid(&req.env_id, "env_id")?;
    let experiment_id = parse_uuid(&req.experiment_id, "experiment_id")?;
    let iteration_id = parse_uuid(&req.iteration_id, "iteration_id")?;

    if req.variant_key.is_empty() {
        return Err(Status::invalid_argument("variant_key must not be empty"));
    }
    if req.metric_key.is_empty() {
        return Err(Status::invalid_argument("metric_key must not be empty"));
    }
    if req.metric_type.is_empty() {
        return Err(Status::invalid_argument("metric_type must not be empty"));
    }
    if req.variant_stats.is_empty() {
        return Err(Status::invalid_argument("variant_stats must not be empty"));
    }
    if req.recommendation.is_empty() {
        return Err(Status::invalid_argument("recommendation must not be empty"));
    }

    let variant_stats: serde_json::Value = serde_json::from_str(&req.variant_stats)
        .map_err(|e| Status::invalid_argument(format!("variant_stats is not valid JSON: {e}")))?;

    let frequentist_result = req
        .frequentist_result
        .as_deref()
        .filter(|s| !s.is_empty())
        .map(|s| {
            serde_json::from_str::<serde_json::Value>(s).map_err(|e| {
                Status::invalid_argument(format!("frequentist_result not valid JSON: {e}"))
            })
        })
        .transpose()?;

    let bayesian_result = req
        .bayesian_result
        .as_deref()
        .filter(|s| !s.is_empty())
        .map(|s| {
            serde_json::from_str::<serde_json::Value>(s).map_err(|e| {
                Status::invalid_argument(format!("bayesian_result not valid JSON: {e}"))
            })
        })
        .transpose()?;

    let sequential_result = req
        .sequential_result
        .as_deref()
        .filter(|s| !s.is_empty())
        .map(|s| {
            serde_json::from_str::<serde_json::Value>(s).map_err(|e| {
                Status::invalid_argument(format!("sequential_result not valid JSON: {e}"))
            })
        })
        .transpose()?;

    let computed_at = if req.computed_at.is_empty() {
        chrono::Utc::now()
    } else {
        req.computed_at
            .parse::<chrono::DateTime<chrono::Utc>>()
            .map_err(|e| {
                Status::invalid_argument(format!("computed_at must be RFC 3339 UTC: {e}"))
            })?
    };

    let row = WriteResultRow {
        env_id,
        experiment_id,
        iteration_id,
        variant_key: req.variant_key,
        metric_key: req.metric_key,
        metric_type: req.metric_type,
        variant_stats,
        frequentist_result,
        bayesian_result,
        // Sequential testing result (per-variant JSON blob) is pass-through from
        // the request; absent when sequential testing was not run for the row.
        sequential_result,
        recommendation: req.recommendation,
        computed_at,
        context_type: req.context_type,
    };

    repo.write(vec![row])
        .await
        .map_err(|e| Status::internal(format!("write failed: {e}")))?;

    Ok(Response::new(WriteExperimentResultsResponse {}))
}

/// Handle `ListExperimentResults` — streams results matching the filter.
pub type ResultStream = Pin<Box<dyn Stream<Item = Result<ExperimentResult, Status>> + Send>>;

#[allow(clippy::result_large_err)]
pub async fn handle_list_experiment_results(
    repo: &Arc<dyn ExperimentResultsRepository>,
    request: Request<ListExperimentResultsRequest>,
) -> Result<Response<ResultStream>, Status> {
    let req = request.into_inner();

    let env_id = parse_uuid(&req.env_id, "env_id")?;
    let experiment_id = parse_uuid(&req.experiment_id, "experiment_id")?;
    let iteration_id = req
        .iteration_id
        .as_deref()
        .map(|s| parse_uuid(s, "iteration_id"))
        .transpose()?;

    let filter = ResultsFilter {
        env_id,
        experiment_id,
        iteration_id,
    };

    let rows = repo
        .list(&filter)
        .await
        .map_err(|e| Status::internal(format!("list failed: {e}")))?;

    let stream: ResultStream = Box::pin(stream_iter(rows.into_iter().map(|r| Ok(row_to_proto(r)))));

    Ok(Response::new(stream))
}

/// Handle `GetExperimentResult` — fetches one result by (variant_key, metric_key).
#[allow(clippy::result_large_err)]
pub async fn handle_get_experiment_result(
    repo: &Arc<dyn ExperimentResultsRepository>,
    request: Request<GetExperimentResultRequest>,
) -> Result<Response<ExperimentResult>, Status> {
    let req = request.into_inner();

    let env_id = parse_uuid(&req.env_id, "env_id")?;
    let experiment_id = parse_uuid(&req.experiment_id, "experiment_id")?;

    if req.variant_key.is_empty() {
        return Err(Status::invalid_argument("variant_key must not be empty"));
    }
    if req.metric_key.is_empty() {
        return Err(Status::invalid_argument("metric_key must not be empty"));
    }

    let iteration_id = req
        .iteration_id
        .as_deref()
        .map(|s| parse_uuid(s, "iteration_id"))
        .transpose()?;

    let row = repo
        .get(
            env_id,
            experiment_id,
            &req.variant_key,
            &req.metric_key,
            iteration_id,
        )
        .await
        .map_err(|e| match e {
            RepoError::Clickhouse(ref inner) => Status::internal(format!("get failed: {inner}")),
            RepoError::Json(ref inner) => Status::internal(format!("get failed (json): {inner}")),
        })?
        .ok_or_else(|| {
            Status::not_found(format!(
                "no result for variant_key={} metric_key={}",
                req.variant_key, req.metric_key
            ))
        })?;

    Ok(Response::new(row_to_proto(row)))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use async_trait::async_trait;
    use tokio_stream::StreamExt as _;
    use uuid::Uuid;

    use super::*;
    use crate::repo::experiment_results::{
        ExperimentResultRow, ExperimentResultsRepository, RepoError, ResultsFilter, WriteResultRow,
    };

    // -----------------------------------------------------------------------
    // Mock repository (implements Worker 3's trait)
    // -----------------------------------------------------------------------

    struct MockRepo {
        rows: Mutex<Vec<ExperimentResultRow>>,
        fail_write: bool,
        fail_list: bool,
        fail_get: bool,
    }

    impl MockRepo {
        fn empty() -> Arc<dyn ExperimentResultsRepository> {
            Arc::new(Self {
                rows: Mutex::new(vec![]),
                fail_write: false,
                fail_list: false,
                fail_get: false,
            })
        }

        fn with_row(row: ExperimentResultRow) -> Arc<dyn ExperimentResultsRepository> {
            Arc::new(Self {
                rows: Mutex::new(vec![row]),
                fail_write: false,
                fail_list: false,
                fail_get: false,
            })
        }

        fn always_errors() -> Arc<dyn ExperimentResultsRepository> {
            Arc::new(Self {
                rows: Mutex::new(vec![]),
                fail_write: true,
                fail_list: true,
                fail_get: true,
            })
        }
    }

    #[async_trait]
    impl ExperimentResultsRepository for MockRepo {
        async fn write(&self, rows: Vec<WriteResultRow>) -> Result<(), RepoError> {
            if self.fail_write {
                return Err(RepoError::Json(
                    serde_json::from_str::<serde_json::Value>("invalid").unwrap_err(),
                ));
            }
            let mut stored = self.rows.lock().unwrap();
            for r in rows {
                stored.push(ExperimentResultRow {
                    env_id: r.env_id,
                    experiment_id: r.experiment_id,
                    iteration_id: r.iteration_id,
                    variant_key: r.variant_key,
                    metric_key: r.metric_key,
                    metric_type: r.metric_type,
                    variant_stats: serde_json::to_string(&r.variant_stats).unwrap(),
                    frequentist_result: r
                        .frequentist_result
                        .map(|v| serde_json::to_string(&v).unwrap())
                        .unwrap_or_default(),
                    bayesian_result: r
                        .bayesian_result
                        .map(|v| serde_json::to_string(&v).unwrap())
                        .unwrap_or_default(),
                    sequential_result: r
                        .sequential_result
                        .map(|v| serde_json::to_string(&v).unwrap())
                        .unwrap_or_default(),
                    recommendation: r.recommendation,
                    computed_at: r.computed_at.timestamp_millis(),
                    created_at: chrono::Utc::now().timestamp_millis(),
                    context_type: r.context_type,
                });
            }
            Ok(())
        }

        async fn list(
            &self,
            filter: &ResultsFilter,
        ) -> Result<Vec<ExperimentResultRow>, RepoError> {
            if self.fail_list {
                return Err(RepoError::Json(
                    serde_json::from_str::<serde_json::Value>("invalid").unwrap_err(),
                ));
            }
            let rows = self.rows.lock().unwrap();
            Ok(rows
                .iter()
                .filter(|r| {
                    r.env_id == filter.env_id
                        && r.experiment_id == filter.experiment_id
                        && filter.iteration_id.is_none_or(|it| r.iteration_id == it)
                })
                .cloned()
                .collect())
        }

        async fn get(
            &self,
            env_id: Uuid,
            experiment_id: Uuid,
            variant_key: &str,
            metric_key: &str,
            iteration_id: Option<Uuid>,
        ) -> Result<Option<ExperimentResultRow>, RepoError> {
            if self.fail_get {
                return Err(RepoError::Json(
                    serde_json::from_str::<serde_json::Value>("invalid").unwrap_err(),
                ));
            }
            let rows = self.rows.lock().unwrap();
            Ok(rows
                .iter()
                .find(|r| {
                    r.env_id == env_id
                        && r.experiment_id == experiment_id
                        && r.variant_key == variant_key
                        && r.metric_key == metric_key
                        && iteration_id.is_none_or(|it| r.iteration_id == it)
                })
                .cloned())
        }
    }

    // -----------------------------------------------------------------------
    // Helpers / constants
    // -----------------------------------------------------------------------

    const ENV_ID: &str = "550e8400-e29b-41d4-a716-446655440099";
    const EXP_ID: &str = "550e8400-e29b-41d4-a716-446655440000";
    const ITER_ID: &str = "550e8400-e29b-41d4-a716-446655440001";

    fn sample_row() -> ExperimentResultRow {
        ExperimentResultRow {
            env_id: ENV_ID.parse().unwrap(),
            experiment_id: EXP_ID.parse().unwrap(),
            iteration_id: ITER_ID.parse().unwrap(),
            variant_key: "control".to_string(),
            metric_key: "checkout".to_string(),
            metric_type: "count".to_string(),
            variant_stats: r#"{"control":100,"treatment":120}"#.to_string(),
            frequentist_result: r#"{"p_value":0.03}"#.to_string(),
            bayesian_result: String::new(),
            sequential_result: String::new(),
            recommendation: "ship_treatment".to_string(),
            computed_at: 1_746_057_600_000, // 2026-05-01T00:00:00Z
            created_at: 1_746_057_600_000,
            context_type: "user".to_string(),
        }
    }

    fn write_request() -> Request<WriteExperimentResultsRequest> {
        Request::new(WriteExperimentResultsRequest {
            env_id: ENV_ID.to_string(),
            experiment_id: EXP_ID.to_string(),
            iteration_id: ITER_ID.to_string(),
            variant_key: "control".to_string(),
            metric_key: "checkout".to_string(),
            metric_type: "count".to_string(),
            variant_stats: r#"{"control":100,"treatment":120}"#.to_string(),
            frequentist_result: Some(r#"{"p_value":0.03}"#.to_string()),
            bayesian_result: None,
            sequential_result: None,
            recommendation: "ship_treatment".to_string(),
            computed_at: "2026-05-01T00:00:00Z".to_string(),
            context_type: "user".to_string(),
        })
    }

    // -----------------------------------------------------------------------
    // WriteExperimentResults
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn write_returns_empty_response_on_success() {
        let repo = MockRepo::empty();
        let resp = handle_write_experiment_results(&repo, write_request())
            .await
            .expect("should succeed");
        // Response is empty — just confirm it exists
        let _ = resp.into_inner();
    }

    #[tokio::test]
    async fn write_rejects_invalid_env_uuid() {
        let repo = MockRepo::empty();
        let mut req = write_request().into_inner();
        req.env_id = "not-a-uuid".to_string();
        let err = handle_write_experiment_results(&repo, Request::new(req))
            .await
            .unwrap_err();
        assert_eq!(err.code(), tonic::Code::InvalidArgument);
        assert!(err.message().contains("env_id"));
    }

    #[tokio::test]
    async fn write_rejects_invalid_experiment_uuid() {
        let repo = MockRepo::empty();
        let mut req = write_request().into_inner();
        req.experiment_id = "not-a-uuid".to_string();
        let err = handle_write_experiment_results(&repo, Request::new(req))
            .await
            .unwrap_err();
        assert_eq!(err.code(), tonic::Code::InvalidArgument);
        assert!(err.message().contains("experiment_id"));
    }

    #[tokio::test]
    async fn write_rejects_invalid_iteration_uuid() {
        let repo = MockRepo::empty();
        let mut req = write_request().into_inner();
        req.iteration_id = "bad".to_string();
        let err = handle_write_experiment_results(&repo, Request::new(req))
            .await
            .unwrap_err();
        assert_eq!(err.code(), tonic::Code::InvalidArgument);
        assert!(err.message().contains("iteration_id"));
    }

    #[tokio::test]
    async fn write_rejects_empty_variant_key() {
        let repo = MockRepo::empty();
        let mut req = write_request().into_inner();
        req.variant_key = String::new();
        let err = handle_write_experiment_results(&repo, Request::new(req))
            .await
            .unwrap_err();
        assert_eq!(err.code(), tonic::Code::InvalidArgument);
        assert!(err.message().contains("variant_key"));
    }

    #[tokio::test]
    async fn write_rejects_empty_metric_key() {
        let repo = MockRepo::empty();
        let mut req = write_request().into_inner();
        req.metric_key = String::new();
        let err = handle_write_experiment_results(&repo, Request::new(req))
            .await
            .unwrap_err();
        assert_eq!(err.code(), tonic::Code::InvalidArgument);
        assert!(err.message().contains("metric_key"));
    }

    #[tokio::test]
    async fn write_rejects_empty_metric_type() {
        let repo = MockRepo::empty();
        let mut req = write_request().into_inner();
        req.metric_type = String::new();
        let err = handle_write_experiment_results(&repo, Request::new(req))
            .await
            .unwrap_err();
        assert_eq!(err.code(), tonic::Code::InvalidArgument);
        assert!(err.message().contains("metric_type"));
    }

    #[tokio::test]
    async fn write_rejects_empty_variant_stats() {
        let repo = MockRepo::empty();
        let mut req = write_request().into_inner();
        req.variant_stats = String::new();
        let err = handle_write_experiment_results(&repo, Request::new(req))
            .await
            .unwrap_err();
        assert_eq!(err.code(), tonic::Code::InvalidArgument);
        assert!(err.message().contains("variant_stats"));
    }

    #[tokio::test]
    async fn write_rejects_invalid_variant_stats_json() {
        let repo = MockRepo::empty();
        let mut req = write_request().into_inner();
        req.variant_stats = "not-json".to_string();
        let err = handle_write_experiment_results(&repo, Request::new(req))
            .await
            .unwrap_err();
        assert_eq!(err.code(), tonic::Code::InvalidArgument);
        assert!(err.message().contains("variant_stats"));
    }

    #[tokio::test]
    async fn write_rejects_empty_recommendation() {
        let repo = MockRepo::empty();
        let mut req = write_request().into_inner();
        req.recommendation = String::new();
        let err = handle_write_experiment_results(&repo, Request::new(req))
            .await
            .unwrap_err();
        assert_eq!(err.code(), tonic::Code::InvalidArgument);
        assert!(err.message().contains("recommendation"));
    }

    #[tokio::test]
    async fn write_propagates_internal_repo_error() {
        let repo = MockRepo::always_errors();
        let err = handle_write_experiment_results(&repo, write_request())
            .await
            .unwrap_err();
        assert_eq!(err.code(), tonic::Code::Internal);
        assert!(err.message().contains("write failed"));
    }

    // -----------------------------------------------------------------------
    // ListExperimentResults
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn list_streams_matching_rows() {
        let repo = MockRepo::with_row(sample_row());
        let req = Request::new(ListExperimentResultsRequest {
            env_id: ENV_ID.to_string(),
            experiment_id: EXP_ID.to_string(),
            iteration_id: Some(ITER_ID.to_string()),
        });
        let mut stream = handle_list_experiment_results(&repo, req)
            .await
            .expect("should succeed")
            .into_inner();

        let item = stream.next().await.expect("one item").expect("no error");
        assert_eq!(item.metric_key, "checkout");
        assert_eq!(item.variant_key, "control");
        assert!(stream.next().await.is_none());
    }

    #[tokio::test]
    async fn list_streams_empty_when_no_rows() {
        let repo = MockRepo::empty();
        let req = Request::new(ListExperimentResultsRequest {
            env_id: ENV_ID.to_string(),
            experiment_id: EXP_ID.to_string(),
            iteration_id: None,
        });
        let mut stream = handle_list_experiment_results(&repo, req)
            .await
            .expect("should succeed")
            .into_inner();
        assert!(stream.next().await.is_none());
    }

    #[tokio::test]
    async fn list_rejects_invalid_env_uuid() {
        let repo = MockRepo::empty();
        let req = Request::new(ListExperimentResultsRequest {
            env_id: "bad-uuid".to_string(),
            experiment_id: EXP_ID.to_string(),
            iteration_id: None,
        });
        let err = handle_list_experiment_results(&repo, req)
            .await
            .err()
            .expect("should be an error");
        assert_eq!(err.code(), tonic::Code::InvalidArgument);
        assert!(err.message().contains("env_id"));
    }

    #[tokio::test]
    async fn list_rejects_invalid_experiment_uuid() {
        let repo = MockRepo::empty();
        let req = Request::new(ListExperimentResultsRequest {
            env_id: ENV_ID.to_string(),
            experiment_id: "bad-uuid".to_string(),
            iteration_id: None,
        });
        let err = handle_list_experiment_results(&repo, req)
            .await
            .err()
            .expect("should be an error");
        assert_eq!(err.code(), tonic::Code::InvalidArgument);
        assert!(err.message().contains("experiment_id"));
    }

    #[tokio::test]
    async fn list_rejects_invalid_optional_iteration_uuid() {
        let repo = MockRepo::empty();
        let req = Request::new(ListExperimentResultsRequest {
            env_id: ENV_ID.to_string(),
            experiment_id: EXP_ID.to_string(),
            iteration_id: Some("bad".to_string()),
        });
        let err = handle_list_experiment_results(&repo, req)
            .await
            .err()
            .expect("should be an error");
        assert_eq!(err.code(), tonic::Code::InvalidArgument);
        assert!(err.message().contains("iteration_id"));
    }

    #[tokio::test]
    async fn list_propagates_internal_repo_error() {
        let repo = MockRepo::always_errors();
        let req = Request::new(ListExperimentResultsRequest {
            env_id: ENV_ID.to_string(),
            experiment_id: EXP_ID.to_string(),
            iteration_id: None,
        });
        let err = handle_list_experiment_results(&repo, req)
            .await
            .err()
            .expect("should be an error");
        assert_eq!(err.code(), tonic::Code::Internal);
    }

    // -----------------------------------------------------------------------
    // GetExperimentResult
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn get_returns_row_on_success() {
        let repo = MockRepo::with_row(sample_row());
        let req = Request::new(GetExperimentResultRequest {
            env_id: ENV_ID.to_string(),
            experiment_id: EXP_ID.to_string(),
            variant_key: "control".to_string(),
            metric_key: "checkout".to_string(),
            iteration_id: Some(ITER_ID.to_string()),
        });
        let result = handle_get_experiment_result(&repo, req)
            .await
            .expect("should succeed")
            .into_inner();
        assert_eq!(result.metric_key, "checkout");
        assert_eq!(result.variant_key, "control");
        assert_eq!(result.recommendation, "ship_treatment");
        // frequentist_result non-empty string → Some
        assert!(result.frequentist_result.is_some());
        // bayesian_result empty string → None
        assert!(result.bayesian_result.is_none());
    }

    #[tokio::test]
    async fn get_rejects_invalid_env_uuid() {
        let repo = MockRepo::empty();
        let req = Request::new(GetExperimentResultRequest {
            env_id: "not-uuid".to_string(),
            experiment_id: EXP_ID.to_string(),
            variant_key: "control".to_string(),
            metric_key: "checkout".to_string(),
            iteration_id: None,
        });
        let err = handle_get_experiment_result(&repo, req).await.unwrap_err();
        assert_eq!(err.code(), tonic::Code::InvalidArgument);
        assert!(err.message().contains("env_id"));
    }

    #[tokio::test]
    async fn get_rejects_invalid_experiment_uuid() {
        let repo = MockRepo::empty();
        let req = Request::new(GetExperimentResultRequest {
            env_id: ENV_ID.to_string(),
            experiment_id: "not-uuid".to_string(),
            variant_key: "control".to_string(),
            metric_key: "checkout".to_string(),
            iteration_id: None,
        });
        let err = handle_get_experiment_result(&repo, req).await.unwrap_err();
        assert_eq!(err.code(), tonic::Code::InvalidArgument);
        assert!(err.message().contains("experiment_id"));
    }

    #[tokio::test]
    async fn get_rejects_empty_variant_key() {
        let repo = MockRepo::empty();
        let req = Request::new(GetExperimentResultRequest {
            env_id: ENV_ID.to_string(),
            experiment_id: EXP_ID.to_string(),
            variant_key: String::new(),
            metric_key: "checkout".to_string(),
            iteration_id: None,
        });
        let err = handle_get_experiment_result(&repo, req).await.unwrap_err();
        assert_eq!(err.code(), tonic::Code::InvalidArgument);
        assert!(err.message().contains("variant_key"));
    }

    #[tokio::test]
    async fn get_rejects_empty_metric_key() {
        let repo = MockRepo::empty();
        let req = Request::new(GetExperimentResultRequest {
            env_id: ENV_ID.to_string(),
            experiment_id: EXP_ID.to_string(),
            variant_key: "control".to_string(),
            metric_key: String::new(),
            iteration_id: None,
        });
        let err = handle_get_experiment_result(&repo, req).await.unwrap_err();
        assert_eq!(err.code(), tonic::Code::InvalidArgument);
        assert!(err.message().contains("metric_key"));
    }

    #[tokio::test]
    async fn get_returns_not_found_for_missing_row() {
        let repo = MockRepo::empty();
        let req = Request::new(GetExperimentResultRequest {
            env_id: ENV_ID.to_string(),
            experiment_id: EXP_ID.to_string(),
            variant_key: "control".to_string(),
            metric_key: "checkout".to_string(),
            iteration_id: None,
        });
        let err = handle_get_experiment_result(&repo, req).await.unwrap_err();
        assert_eq!(err.code(), tonic::Code::NotFound);
        assert!(err.message().contains("checkout"));
    }

    #[tokio::test]
    async fn get_propagates_internal_repo_error() {
        let repo = MockRepo::always_errors();
        let req = Request::new(GetExperimentResultRequest {
            env_id: ENV_ID.to_string(),
            experiment_id: EXP_ID.to_string(),
            variant_key: "control".to_string(),
            metric_key: "checkout".to_string(),
            iteration_id: None,
        });
        let err = handle_get_experiment_result(&repo, req).await.unwrap_err();
        assert_eq!(err.code(), tonic::Code::Internal);
    }
}
