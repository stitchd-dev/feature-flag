//! Segment route handlers — proxy REST requests to the Segmentation Service.

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use utoipa::ToSchema;

use stitchd_proto::segments::v1::{
    GetSegmentRequest, ListSegmentsRequest, MutateSegmentRequest, SegmentMutationKind,
};

use crate::error::GatewayError;
use crate::state::GatewayState;

// ─── REST types ───────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize, ToSchema)]
pub struct SegmentCreateRequest {
    pub key: Option<String>,
    pub context_type: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct SegmentJson {
    pub key: String,
    pub context_type: String,
    #[schema(value_type = String)]
    pub kind: &'static str,
}

// ─── Handlers ─────────────────────────────────────────────────────────────────

/// `GET /v1/environments/{env_id}/segments`
#[utoipa::path(
    get,
    path = "/v1/environments/{env_id}/segments",
    tag = "segments",
    params(("env_id" = String, Path, description = "Environment ID")),
    responses(
        (status = 200, description = "List of segments", body = Vec<SegmentJson>),
        (status = 401, description = "Unauthorized"),
        (status = 502, description = "Segmentation service unavailable"),
    ),
    security(("bearer_jwt" = []))
)]
pub async fn list_segments(
    State(state): State<Arc<GatewayState>>,
    Path(env_id): Path<String>,
) -> Result<impl IntoResponse, GatewayError> {
    let req = tonic::Request::new(ListSegmentsRequest {
        environment_id: env_id,
    });
    let mut client = state.segmentation_client.lock().await;
    let resp = client
        .list_segments(req)
        .await
        .map_err(GatewayError::from)?;
    let inner = resp.into_inner();
    let mut segments: Vec<SegmentJson> = inner
        .rule_segments
        .iter()
        .map(|s| SegmentJson {
            key: s.key.clone(),
            context_type: s.context_type.clone(),
            kind: "rule",
        })
        .collect();
    for ls in &inner.list_segments {
        segments.push(SegmentJson {
            key: ls.key.clone(),
            context_type: ls.context_type.clone(),
            kind: "list",
        });
    }
    Ok(Json(segments))
}

/// `POST /v1/environments/{env_id}/segments`
#[utoipa::path(
    post,
    path = "/v1/environments/{env_id}/segments",
    tag = "segments",
    params(("env_id" = String, Path, description = "Environment ID")),
    request_body = SegmentCreateRequest,
    responses(
        (status = 201, description = "Segment created", body = SegmentJson),
        (status = 401, description = "Unauthorized"),
        (status = 502, description = "Segmentation service unavailable"),
    ),
    security(("bearer_jwt" = []))
)]
pub async fn create_segment(
    State(state): State<Arc<GatewayState>>,
    Path(env_id): Path<String>,
    Json(body): Json<SegmentCreateRequest>,
) -> Result<impl IntoResponse, GatewayError> {
    use stitchd_proto::segments::v1::mutate_segment_request::Segment;
    use stitchd_proto::segments::v1::{MutateSegmentRequest, RuleSegment};

    let rule_seg = RuleSegment {
        key: body.key.unwrap_or_default(),
        context_type: body.context_type.unwrap_or_default(),
        ..Default::default()
    };
    let req = tonic::Request::new(MutateSegmentRequest {
        environment_id: env_id,
        kind: SegmentMutationKind::Create as i32,
        segment: Some(Segment::RuleSegment(rule_seg)),
        version: 0,
    });
    let mut client = state.segmentation_client.lock().await;
    let resp = client
        .mutate_segment(req)
        .await
        .map_err(GatewayError::from)?;
    let inner = resp.into_inner();
    let seg_json = match inner.segment {
        Some(stitchd_proto::segments::v1::mutate_segment_response::Segment::RuleSegment(s)) => {
            SegmentJson {
                key: s.key,
                context_type: s.context_type,
                kind: "rule",
            }
        }
        Some(stitchd_proto::segments::v1::mutate_segment_response::Segment::ListSegment(s)) => {
            SegmentJson {
                key: s.key,
                context_type: s.context_type,
                kind: "list",
            }
        }
        None => SegmentJson {
            key: String::new(),
            context_type: String::new(),
            kind: "unknown",
        },
    };
    Ok((StatusCode::CREATED, Json(seg_json)))
}

/// `GET /v1/environments/{env_id}/segments/{segment_key}`
#[utoipa::path(
    get,
    path = "/v1/environments/{env_id}/segments/{segment_id}",
    tag = "segments",
    params(
        ("env_id" = String, Path, description = "Environment ID"),
        ("segment_id" = String, Path, description = "Segment key"),
    ),
    responses(
        (status = 200, description = "Segment", body = SegmentJson),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "Segment not found"),
        (status = 502, description = "Segmentation service unavailable"),
    ),
    security(("bearer_jwt" = []))
)]
pub async fn get_segment(
    State(state): State<Arc<GatewayState>>,
    Path((env_id, segment_key)): Path<(String, String)>,
) -> Result<impl IntoResponse, GatewayError> {
    let req = tonic::Request::new(GetSegmentRequest {
        environment_id: env_id,
        segment_key,
    });
    let mut client = state.segmentation_client.lock().await;
    let resp = client.get_segment(req).await.map_err(GatewayError::from)?;
    let bundle = resp.into_inner();
    // Return first rule or list segment found
    let result = if let Some(s) = bundle.rule_segments.first() {
        SegmentJson {
            key: s.key.clone(),
            context_type: s.context_type.clone(),
            kind: "rule",
        }
    } else if let Some(s) = bundle.list_segments.first() {
        SegmentJson {
            key: s.key.clone(),
            context_type: s.context_type.clone(),
            kind: "list",
        }
    } else {
        return Err(GatewayError::NotFound("segment not found".to_string()));
    };
    Ok(Json(result))
}

/// `PUT /v1/environments/{env_id}/segments/{segment_key}`
#[utoipa::path(
    put,
    path = "/v1/environments/{env_id}/segments/{segment_id}",
    tag = "segments",
    params(
        ("env_id" = String, Path, description = "Environment ID"),
        ("segment_id" = String, Path, description = "Segment key"),
    ),
    request_body = SegmentCreateRequest,
    responses(
        (status = 200, description = "Updated segment", body = SegmentJson),
        (status = 401, description = "Unauthorized"),
        (status = 502, description = "Segmentation service unavailable"),
    ),
    security(("bearer_jwt" = []))
)]
pub async fn update_segment(
    State(state): State<Arc<GatewayState>>,
    Path((env_id, segment_key)): Path<(String, String)>,
    Json(body): Json<SegmentCreateRequest>,
) -> Result<impl IntoResponse, GatewayError> {
    use stitchd_proto::segments::v1::RuleSegment;
    use stitchd_proto::segments::v1::mutate_segment_request::Segment;

    let rule_seg = RuleSegment {
        key: segment_key,
        context_type: body.context_type.unwrap_or_default(),
        ..Default::default()
    };
    let req = tonic::Request::new(MutateSegmentRequest {
        environment_id: env_id,
        kind: SegmentMutationKind::Update as i32,
        segment: Some(Segment::RuleSegment(rule_seg)),
        version: 0,
    });
    let mut client = state.segmentation_client.lock().await;
    let resp = client
        .mutate_segment(req)
        .await
        .map_err(GatewayError::from)?;
    let inner = resp.into_inner();
    let seg_json = match inner.segment {
        Some(stitchd_proto::segments::v1::mutate_segment_response::Segment::RuleSegment(s)) => {
            SegmentJson {
                key: s.key,
                context_type: s.context_type,
                kind: "rule",
            }
        }
        Some(stitchd_proto::segments::v1::mutate_segment_response::Segment::ListSegment(s)) => {
            SegmentJson {
                key: s.key,
                context_type: s.context_type,
                kind: "list",
            }
        }
        None => SegmentJson {
            key: String::new(),
            context_type: String::new(),
            kind: "unknown",
        },
    };
    Ok(Json(seg_json))
}

/// `DELETE /v1/environments/{env_id}/segments/{segment_key}`
#[utoipa::path(
    delete,
    path = "/v1/environments/{env_id}/segments/{segment_id}",
    tag = "segments",
    params(
        ("env_id" = String, Path, description = "Environment ID"),
        ("segment_id" = String, Path, description = "Segment key"),
    ),
    responses(
        (status = 204, description = "Segment deleted"),
        (status = 401, description = "Unauthorized"),
        (status = 502, description = "Segmentation service unavailable"),
    ),
    security(("bearer_jwt" = []))
)]
pub async fn delete_segment(
    State(state): State<Arc<GatewayState>>,
    Path((env_id, segment_key)): Path<(String, String)>,
) -> Result<impl IntoResponse, GatewayError> {
    use stitchd_proto::segments::v1::RuleSegment;
    use stitchd_proto::segments::v1::mutate_segment_request::Segment;

    let rule_seg = RuleSegment {
        key: segment_key,
        ..Default::default()
    };
    let req = tonic::Request::new(MutateSegmentRequest {
        environment_id: env_id,
        kind: SegmentMutationKind::Delete as i32,
        segment: Some(Segment::RuleSegment(rule_seg)),
        version: 0,
    });
    let mut client = state.segmentation_client.lock().await;
    client
        .mutate_segment(req)
        .await
        .map_err(GatewayError::from)?;
    Ok(StatusCode::NO_CONTENT)
}

// ─── Test helpers ─────────────────────────────────────────────────────────────

#[cfg(test)]
pub fn test_router(state: Arc<GatewayState>) -> axum::Router {
    #[allow(unused_imports)]
    use axum::routing::{delete, get, post, put};
    axum::Router::new()
        .route(
            "/v1/environments/{env_id}/segments",
            get(list_segments).post(create_segment),
        )
        .route(
            "/v1/environments/{env_id}/segments/{segment_key}",
            get(get_segment).put(update_segment).delete(delete_segment),
        )
        .with_state(state)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::Body,
        http::{Request, StatusCode},
    };
    use tower::ServiceExt as _;

    use crate::tests::helpers::make_stub_state;

    #[tokio::test]
    async fn list_segments_returns_200_or_502() {
        let state = make_stub_state();
        let app = test_router(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/v1/environments/env-1/segments")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(
            resp.status() == StatusCode::OK || resp.status() == StatusCode::BAD_GATEWAY,
            "status: {}",
            resp.status()
        );
    }

    #[tokio::test]
    async fn create_segment_returns_201_or_502() {
        let state = make_stub_state();
        let app = test_router(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/environments/env-1/segments")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"key":"s1","context_type":"user"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(
            resp.status() == StatusCode::CREATED || resp.status() == StatusCode::BAD_GATEWAY,
            "status: {}",
            resp.status()
        );
    }

    #[tokio::test]
    async fn get_segment_returns_200_404_or_502() {
        let state = make_stub_state();
        let app = test_router(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/v1/environments/env-1/segments/my-seg")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(
            resp.status() == StatusCode::OK
                || resp.status() == StatusCode::NOT_FOUND
                || resp.status() == StatusCode::BAD_GATEWAY,
            "status: {}",
            resp.status()
        );
    }

    #[tokio::test]
    async fn update_segment_returns_200_or_502() {
        let state = make_stub_state();
        let app = test_router(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/v1/environments/env-1/segments/my-seg")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"context_type":"user"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(
            resp.status() == StatusCode::OK || resp.status() == StatusCode::BAD_GATEWAY,
            "status: {}",
            resp.status()
        );
    }

    #[tokio::test]
    async fn delete_segment_returns_204_or_502() {
        let state = make_stub_state();
        let app = test_router(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri("/v1/environments/env-1/segments/my-seg")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(
            resp.status() == StatusCode::NO_CONTENT || resp.status() == StatusCode::BAD_GATEWAY,
            "status: {}",
            resp.status()
        );
    }
}
