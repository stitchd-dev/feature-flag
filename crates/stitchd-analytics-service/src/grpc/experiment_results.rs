//! gRPC handlers for ExperimentResults RPCs.
//!
//! # Trait boundary
//! Handlers depend on [`ExperimentResultsRepository`], an async trait that
//! abstracts the storage backend.  Worker 3 owns the ClickHouse-backed
//! implementation at `crates/stitchd-analytics-service/src/repo/experiment_results.rs`.
//!
//! # Coordination note (merge)
//! The trait name here is `ExperimentResultsRepository`.  If Worker 3 chose a
//! different name, prefer renaming this module's trait to converge — the
//! handler functions accept `&dyn ExperimentResultsRepository` and are not
//! coupled to any concrete type.
//!
//! # Proto mapping
//! `ExperimentResultRow` (from `stitchd-db`) ↔ `ExperimentResult` (proto):
//! - JSONB fields (`variant_stats`, `frequentist_result`, `bayesian_result`)
//!   are serialised as JSON strings.
//! - `computed_at` / `created_at` are formatted as RFC 3339 UTC.

use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use tokio_stream::{Stream, iter as stream_iter};
use tonic::{Request, Response, Status};

use stitchd_proto::analytics::v1::{
    ExperimentResult, GetExperimentResultRequest, ListExperimentResultsRequest,
    WriteExperimentResultsRequest, WriteExperimentResultsResponse,
};

// ---------------------------------------------------------------------------
// Repository trait
// ---------------------------------------------------------------------------

/// Write input for one experiment result.
#[derive(Debug, Clone)]
pub struct WriteResultInput {
    pub experiment_id: uuid::Uuid,
    pub iteration_id: uuid::Uuid,
    pub metric_key: String,
    pub metric_type: String,
    /// Per-variant sample statistics — serialised JSON string.
    pub variant_stats: String,
    pub frequentist_result: Option<String>,
    pub bayesian_result: Option<String>,
    pub recommendation: String,
    /// RFC 3339 UTC string when the analysis was computed.
    pub computed_at: String,
}

/// A result row as returned by the repository.
#[derive(Debug, Clone)]
pub struct ResultRow {
    pub id: uuid::Uuid,
    pub experiment_id: uuid::Uuid,
    pub iteration_id: uuid::Uuid,
    pub metric_key: String,
    pub metric_type: String,
    pub variant_stats: String,
    pub frequentist_result: Option<String>,
    pub bayesian_result: Option<String>,
    pub recommendation: String,
    pub computed_at: String,  // RFC 3339 UTC
    pub created_at: String,   // RFC 3339 UTC
}

/// Error type for repository operations.
#[derive(Debug, thiserror::Error)]
pub enum RepoError {
    #[error("not found")]
    NotFound,
    #[error("internal: {0}")]
    Internal(String),
}

// TODO(merge): Worker 3 owns the ClickHouse impl at
// `crates/stitchd-analytics-service/src/repo/experiment_results.rs`.
// The trait name `ExperimentResultsRepository` was chosen here; if Worker 3
// picks a different name, converge by renaming one side.
/// Data-access interface for experiment results backed by an analytics store.
#[async_trait]
pub trait ExperimentResultsRepository: Send + Sync {
    /// Upsert one experiment result.
    ///
    /// On conflict on `(experiment_id, iteration_id, metric_key)` the row
    /// is updated in-place.
    async fn write(&self, input: &WriteResultInput) -> Result<ResultRow, RepoError>;

    /// List all results for the latest iteration of an experiment.
    ///
    /// If `iteration_id` is `Some`, return results for that specific iteration
    /// instead.
    async fn list(
        &self,
        experiment_id: uuid::Uuid,
        iteration_id: Option<uuid::Uuid>,
    ) -> Result<Vec<ResultRow>, RepoError>;

    /// Fetch a single result by `(experiment_id, iteration_id, metric_key)`.
    async fn get(
        &self,
        experiment_id: uuid::Uuid,
        iteration_id: uuid::Uuid,
        metric_key: &str,
    ) -> Result<ResultRow, RepoError>;
}

// ---------------------------------------------------------------------------
// Proto conversion helpers
// ---------------------------------------------------------------------------

fn row_to_proto(row: ResultRow) -> ExperimentResult {
    ExperimentResult {
        id: row.id.to_string(),
        experiment_id: row.experiment_id.to_string(),
        iteration_id: row.iteration_id.to_string(),
        metric_key: row.metric_key,
        metric_type: row.metric_type,
        variant_stats: row.variant_stats,
        frequentist_result: row.frequentist_result,
        bayesian_result: row.bayesian_result,
        recommendation: row.recommendation,
        computed_at: row.computed_at,
        created_at: row.created_at,
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

/// Handle `WriteExperimentResults` — upserts one result row.
pub async fn handle_write_experiment_results(
    repo: &Arc<dyn ExperimentResultsRepository>,
    request: Request<WriteExperimentResultsRequest>,
) -> Result<Response<WriteExperimentResultsResponse>, Status> {
    let req = request.into_inner();

    let experiment_id = parse_uuid(&req.experiment_id, "experiment_id")?;
    let iteration_id = parse_uuid(&req.iteration_id, "iteration_id")?;

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

    let input = WriteResultInput {
        experiment_id,
        iteration_id,
        metric_key: req.metric_key,
        metric_type: req.metric_type,
        variant_stats: req.variant_stats,
        frequentist_result: req.frequentist_result,
        bayesian_result: req.bayesian_result,
        recommendation: req.recommendation,
        computed_at: req.computed_at,
    };

    let row = repo
        .write(&input)
        .await
        .map_err(|e| Status::internal(format!("write failed: {e}")))?;

    Ok(Response::new(WriteExperimentResultsResponse {
        result: Some(row_to_proto(row)),
    }))
}

/// Handle `ListExperimentResults` — streams results for the latest (or
/// specified) iteration.
pub type ResultStream = Pin<Box<dyn Stream<Item = Result<ExperimentResult, Status>> + Send>>;

#[allow(clippy::result_large_err)]
pub async fn handle_list_experiment_results(
    repo: &Arc<dyn ExperimentResultsRepository>,
    request: Request<ListExperimentResultsRequest>,
) -> Result<Response<ResultStream>, Status> {
    let req = request.into_inner();

    let experiment_id = parse_uuid(&req.experiment_id, "experiment_id")?;
    let iteration_id = req
        .iteration_id
        .as_deref()
        .map(|s| parse_uuid(s, "iteration_id"))
        .transpose()?;

    let rows = repo
        .list(experiment_id, iteration_id)
        .await
        .map_err(|e| Status::internal(format!("list failed: {e}")))?;

    let stream: ResultStream = Box::pin(
        stream_iter(rows.into_iter().map(|r| Ok(row_to_proto(r))))
    );

    Ok(Response::new(stream))
}

/// Handle `GetExperimentResult` — fetches one result by metric key.
pub async fn handle_get_experiment_result(
    repo: &Arc<dyn ExperimentResultsRepository>,
    request: Request<GetExperimentResultRequest>,
) -> Result<Response<ExperimentResult>, Status> {
    let req = request.into_inner();

    let experiment_id = parse_uuid(&req.experiment_id, "experiment_id")?;
    let iteration_id = parse_uuid(&req.iteration_id, "iteration_id")?;

    if req.metric_key.is_empty() {
        return Err(Status::invalid_argument("metric_key must not be empty"));
    }

    let row = repo
        .get(experiment_id, iteration_id, &req.metric_key)
        .await
        .map_err(|e| match e {
            RepoError::NotFound => Status::not_found(format!(
                "no result for metric_key={}",
                req.metric_key
            )),
            RepoError::Internal(msg) => Status::internal(msg),
        })?;

    Ok(Response::new(row_to_proto(row)))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use tokio_stream::StreamExt as _;

    use super::*;

    // -----------------------------------------------------------------------
    // Mock repository
    // -----------------------------------------------------------------------

    struct MockRepo {
        rows: Mutex<Vec<ResultRow>>,
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

        fn with_row(row: ResultRow) -> Arc<dyn ExperimentResultsRepository> {
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
        async fn write(&self, input: &WriteResultInput) -> Result<ResultRow, RepoError> {
            if self.fail_write {
                return Err(RepoError::Internal("injected write failure".into()));
            }
            let row = ResultRow {
                id: uuid::Uuid::new_v4(),
                experiment_id: input.experiment_id,
                iteration_id: input.iteration_id,
                metric_key: input.metric_key.clone(),
                metric_type: input.metric_type.clone(),
                variant_stats: input.variant_stats.clone(),
                frequentist_result: input.frequentist_result.clone(),
                bayesian_result: input.bayesian_result.clone(),
                recommendation: input.recommendation.clone(),
                computed_at: input.computed_at.clone(),
                created_at: "2026-01-01T00:00:00Z".to_string(),
            };
            self.rows.lock().unwrap().push(row.clone());
            Ok(row)
        }

        async fn list(
            &self,
            experiment_id: uuid::Uuid,
            iteration_id: Option<uuid::Uuid>,
        ) -> Result<Vec<ResultRow>, RepoError> {
            if self.fail_list {
                return Err(RepoError::Internal("injected list failure".into()));
            }
            let rows = self.rows.lock().unwrap();
            Ok(rows
                .iter()
                .filter(|r| {
                    r.experiment_id == experiment_id
                        && iteration_id.is_none_or(|it| r.iteration_id == it)
                })
                .cloned()
                .collect())
        }

        async fn get(
            &self,
            experiment_id: uuid::Uuid,
            iteration_id: uuid::Uuid,
            metric_key: &str,
        ) -> Result<ResultRow, RepoError> {
            if self.fail_get {
                return Err(RepoError::Internal("injected get failure".into()));
            }
            let rows = self.rows.lock().unwrap();
            rows.iter()
                .find(|r| {
                    r.experiment_id == experiment_id
                        && r.iteration_id == iteration_id
                        && r.metric_key == metric_key
                })
                .cloned()
                .ok_or(RepoError::NotFound)
        }
    }

    // -----------------------------------------------------------------------
    // Helpers
    // -----------------------------------------------------------------------

    const EXP_ID: &str = "550e8400-e29b-41d4-a716-446655440000";
    const ITER_ID: &str = "550e8400-e29b-41d4-a716-446655440001";

    fn sample_row() -> ResultRow {
        ResultRow {
            id: uuid::Uuid::new_v4(),
            experiment_id: EXP_ID.parse().unwrap(),
            iteration_id: ITER_ID.parse().unwrap(),
            metric_key: "checkout".to_string(),
            metric_type: "count".to_string(),
            variant_stats: r#"{"control":100,"treatment":120}"#.to_string(),
            frequentist_result: Some(r#"{"p_value":0.03}"#.to_string()),
            bayesian_result: None,
            recommendation: "ship_treatment".to_string(),
            computed_at: "2026-05-01T00:00:00Z".to_string(),
            created_at: "2026-05-01T00:00:00Z".to_string(),
        }
    }

    fn write_request() -> Request<WriteExperimentResultsRequest> {
        Request::new(WriteExperimentResultsRequest {
            experiment_id: EXP_ID.to_string(),
            iteration_id: ITER_ID.to_string(),
            metric_key: "checkout".to_string(),
            metric_type: "count".to_string(),
            variant_stats: r#"{"control":100,"treatment":120}"#.to_string(),
            frequentist_result: Some(r#"{"p_value":0.03}"#.to_string()),
            bayesian_result: None,
            recommendation: "ship_treatment".to_string(),
            computed_at: "2026-05-01T00:00:00Z".to_string(),
        })
    }

    // -----------------------------------------------------------------------
    // WriteExperimentResults
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn write_returns_result_on_success() {
        let repo = MockRepo::empty();
        let resp = handle_write_experiment_results(&repo, write_request())
            .await
            .expect("should succeed");
        let result = resp.into_inner().result.expect("result present");
        assert_eq!(result.experiment_id, EXP_ID);
        assert_eq!(result.iteration_id, ITER_ID);
        assert_eq!(result.metric_key, "checkout");
        assert_eq!(result.recommendation, "ship_treatment");
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
            experiment_id: EXP_ID.to_string(),
            iteration_id: Some(ITER_ID.to_string()),
        });
        let mut stream = handle_list_experiment_results(&repo, req)
            .await
            .expect("should succeed")
            .into_inner();

        let item = stream.next().await.expect("one item").expect("no error");
        assert_eq!(item.metric_key, "checkout");
        assert!(stream.next().await.is_none());
    }

    #[tokio::test]
    async fn list_streams_empty_when_no_rows() {
        let repo = MockRepo::empty();
        let req = Request::new(ListExperimentResultsRequest {
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
    async fn list_rejects_invalid_experiment_uuid() {
        let repo = MockRepo::empty();
        let req = Request::new(ListExperimentResultsRequest {
            experiment_id: "bad-uuid".to_string(),
            iteration_id: None,
        });
        let result = handle_list_experiment_results(&repo, req).await;
        let err = result.err().expect("should be an error");
        assert_eq!(err.code(), tonic::Code::InvalidArgument);
        assert!(err.message().contains("experiment_id"));
    }

    #[tokio::test]
    async fn list_rejects_invalid_optional_iteration_uuid() {
        let repo = MockRepo::empty();
        let req = Request::new(ListExperimentResultsRequest {
            experiment_id: EXP_ID.to_string(),
            iteration_id: Some("bad".to_string()),
        });
        let result = handle_list_experiment_results(&repo, req).await;
        let err = result.err().expect("should be an error");
        assert_eq!(err.code(), tonic::Code::InvalidArgument);
        assert!(err.message().contains("iteration_id"));
    }

    #[tokio::test]
    async fn list_propagates_internal_repo_error() {
        let repo = MockRepo::always_errors();
        let req = Request::new(ListExperimentResultsRequest {
            experiment_id: EXP_ID.to_string(),
            iteration_id: None,
        });
        let result = handle_list_experiment_results(&repo, req).await;
        let err = result.err().expect("should be an error");
        assert_eq!(err.code(), tonic::Code::Internal);
    }

    // -----------------------------------------------------------------------
    // GetExperimentResult
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn get_returns_row_on_success() {
        let repo = MockRepo::with_row(sample_row());
        let req = Request::new(GetExperimentResultRequest {
            experiment_id: EXP_ID.to_string(),
            iteration_id: ITER_ID.to_string(),
            metric_key: "checkout".to_string(),
        });
        let result = handle_get_experiment_result(&repo, req)
            .await
            .expect("should succeed")
            .into_inner();
        assert_eq!(result.metric_key, "checkout");
        assert_eq!(result.recommendation, "ship_treatment");
    }

    #[tokio::test]
    async fn get_rejects_invalid_experiment_uuid() {
        let repo = MockRepo::empty();
        let req = Request::new(GetExperimentResultRequest {
            experiment_id: "not-uuid".to_string(),
            iteration_id: ITER_ID.to_string(),
            metric_key: "checkout".to_string(),
        });
        let err = handle_get_experiment_result(&repo, req)
            .await
            .unwrap_err();
        assert_eq!(err.code(), tonic::Code::InvalidArgument);
        assert!(err.message().contains("experiment_id"));
    }

    #[tokio::test]
    async fn get_rejects_invalid_iteration_uuid() {
        let repo = MockRepo::empty();
        let req = Request::new(GetExperimentResultRequest {
            experiment_id: EXP_ID.to_string(),
            iteration_id: "oops".to_string(),
            metric_key: "checkout".to_string(),
        });
        let err = handle_get_experiment_result(&repo, req)
            .await
            .unwrap_err();
        assert_eq!(err.code(), tonic::Code::InvalidArgument);
        assert!(err.message().contains("iteration_id"));
    }

    #[tokio::test]
    async fn get_rejects_empty_metric_key() {
        let repo = MockRepo::empty();
        let req = Request::new(GetExperimentResultRequest {
            experiment_id: EXP_ID.to_string(),
            iteration_id: ITER_ID.to_string(),
            metric_key: String::new(),
        });
        let err = handle_get_experiment_result(&repo, req)
            .await
            .unwrap_err();
        assert_eq!(err.code(), tonic::Code::InvalidArgument);
        assert!(err.message().contains("metric_key"));
    }

    #[tokio::test]
    async fn get_returns_not_found_for_missing_row() {
        let repo = MockRepo::empty();
        let req = Request::new(GetExperimentResultRequest {
            experiment_id: EXP_ID.to_string(),
            iteration_id: ITER_ID.to_string(),
            metric_key: "checkout".to_string(),
        });
        let err = handle_get_experiment_result(&repo, req)
            .await
            .unwrap_err();
        assert_eq!(err.code(), tonic::Code::NotFound);
        assert!(err.message().contains("checkout"));
    }

    #[tokio::test]
    async fn get_propagates_internal_repo_error() {
        let repo = MockRepo::always_errors();
        let req = Request::new(GetExperimentResultRequest {
            experiment_id: EXP_ID.to_string(),
            iteration_id: ITER_ID.to_string(),
            metric_key: "checkout".to_string(),
        });
        let err = handle_get_experiment_result(&repo, req)
            .await
            .unwrap_err();
        assert_eq!(err.code(), tonic::Code::Internal);
    }
}
