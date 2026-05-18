//! Evaluation analytics route — delegates to analytics-service via gRPC.
//!
//! GET /v1/projects/{project_id}/flags/{flag_id}/eval-stats
//!
//! Query parameters:
//! - `from` — RFC 3339 start timestamp
//! - `to`   — RFC 3339 end timestamp
//! - `granularity` — `"hour"` | `"day"` (auto-overridden to `"day"` when range > 24 h)

use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;

use stitchd_proto::analytics::v1::GetEvalStatsRequest;

use crate::error::GatewayError;
use crate::state::GatewayState;

/// Query parameters for eval-stats.
#[derive(Debug, Deserialize)]
pub struct EvalStatsQuery {
    pub from: DateTime<Utc>,
    pub to: DateTime<Utc>,
    #[serde(default = "default_granularity")]
    pub granularity: String,
}

fn default_granularity() -> String {
    "hour".to_string()
}

/// One time bucket in the response.
#[derive(Debug, Serialize)]
pub struct EvalStatsBucket {
    pub ts: String,
    pub total: u64,
    pub by_variant: HashMap<String, u64>,
    pub disabled_count: u64,
    pub unique_context_keys: u64,
}

/// Response body for eval-stats.
#[derive(Debug, Serialize)]
pub struct EvalStatsResponse {
    pub granularity: String,
    pub buckets: Vec<EvalStatsBucket>,
}

/// GET /v1/projects/{project_id}/flags/{flag_id}/eval-stats
pub async fn get_eval_stats(
    State(state): State<Arc<GatewayState>>,
    Path((_project_id, flag_id)): Path<(String, String)>,
    Query(params): Query<EvalStatsQuery>,
) -> Result<impl IntoResponse, GatewayError> {
    let req = tonic::Request::new(GetEvalStatsRequest {
        flag_id,
        project_id: _project_id,
        from: params.from.to_rfc3339(),
        to: params.to.to_rfc3339(),
        granularity: params.granularity,
    });

    let resp = state
        .analytics_client
        .lock()
        .await
        .get_eval_stats(req)
        .await
        .map_err(GatewayError::from)?;

    let inner = resp.into_inner();
    let buckets: Vec<EvalStatsBucket> = inner
        .buckets
        .into_iter()
        .map(|b| EvalStatsBucket {
            ts: b.ts,
            total: b.total,
            by_variant: b.by_variant,
            disabled_count: b.disabled_count,
            unique_context_keys: b.unique_context_keys,
        })
        .collect();

    Ok((
        StatusCode::OK,
        Json(EvalStatsResponse {
            granularity: inner.granularity,
            buckets,
        }),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn default_granularity_is_hour() {
        assert_eq!(default_granularity(), "hour");
    }

    #[test]
    fn eval_stats_query_fields() {
        let from = Utc.with_ymd_and_hms(2026, 5, 1, 0, 0, 0).unwrap();
        let to = Utc.with_ymd_and_hms(2026, 5, 2, 0, 0, 0).unwrap();
        let q = EvalStatsQuery {
            from,
            to,
            granularity: "day".to_string(),
        };
        assert_eq!(q.granularity, "day");
    }
}
