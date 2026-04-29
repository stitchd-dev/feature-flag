//! Gateway error type and `IntoResponse` conversion.

use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde_json::json;
use thiserror::Error;

/// Top-level gateway error.
#[derive(Debug, Error)]
pub enum GatewayError {
    /// Authentication failed — invalid or missing credentials.
    #[error("unauthorized: {0}")]
    Unauthorized(String),

    /// The downstream gRPC service returned NOT_FOUND.
    #[error("not found: {0}")]
    NotFound(String),

    /// The downstream gRPC service returned INVALID_ARGUMENT.
    #[error("bad request: {0}")]
    BadRequest(String),

    /// A gRPC transport or internal error.
    #[error("upstream error: {0}")]
    Upstream(String),

    /// Request body could not be deserialized.
    #[error("invalid body: {0}")]
    InvalidBody(String),
}

impl From<tonic::Status> for GatewayError {
    fn from(s: tonic::Status) -> Self {
        match s.code() {
            tonic::Code::Unauthenticated => GatewayError::Unauthorized(s.message().to_string()),
            tonic::Code::PermissionDenied => GatewayError::Unauthorized(s.message().to_string()),
            tonic::Code::NotFound => GatewayError::NotFound(s.message().to_string()),
            tonic::Code::InvalidArgument => GatewayError::BadRequest(s.message().to_string()),
            _ => GatewayError::Upstream(s.message().to_string()),
        }
    }
}

impl IntoResponse for GatewayError {
    fn into_response(self) -> Response {
        let (status, msg) = match &self {
            GatewayError::Unauthorized(m) => (StatusCode::UNAUTHORIZED, m.clone()),
            GatewayError::NotFound(m) => (StatusCode::NOT_FOUND, m.clone()),
            GatewayError::BadRequest(m) => (StatusCode::BAD_REQUEST, m.clone()),
            GatewayError::Upstream(m) => (StatusCode::BAD_GATEWAY, m.clone()),
            GatewayError::InvalidBody(m) => (StatusCode::UNPROCESSABLE_ENTITY, m.clone()),
        };
        let body = Json(json!({ "error": msg }));
        (status, body).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::StatusCode;
    use axum::response::IntoResponse;

    #[test]
    fn unauthorized_maps_to_401() {
        let err = GatewayError::Unauthorized("bad token".to_string());
        let resp = err.into_response();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[test]
    fn not_found_maps_to_404() {
        let err = GatewayError::NotFound("missing".to_string());
        let resp = err.into_response();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[test]
    fn bad_request_maps_to_400() {
        let err = GatewayError::BadRequest("invalid arg".to_string());
        let resp = err.into_response();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[test]
    fn upstream_maps_to_502() {
        let err = GatewayError::Upstream("grpc broke".to_string());
        let resp = err.into_response();
        assert_eq!(resp.status(), StatusCode::BAD_GATEWAY);
    }

    #[test]
    fn invalid_body_maps_to_422() {
        let err = GatewayError::InvalidBody("bad json".to_string());
        let resp = err.into_response();
        assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[test]
    fn tonic_unauthenticated_converts() {
        let s = tonic::Status::unauthenticated("token expired");
        let err = GatewayError::from(s);
        assert!(matches!(err, GatewayError::Unauthorized(_)));
    }

    #[test]
    fn tonic_not_found_converts() {
        let s = tonic::Status::not_found("flag missing");
        let err = GatewayError::from(s);
        assert!(matches!(err, GatewayError::NotFound(_)));
    }

    #[test]
    fn tonic_invalid_argument_converts() {
        let s = tonic::Status::invalid_argument("bad field");
        let err = GatewayError::from(s);
        assert!(matches!(err, GatewayError::BadRequest(_)));
    }

    #[test]
    fn tonic_internal_converts_to_upstream() {
        let s = tonic::Status::internal("server exploded");
        let err = GatewayError::from(s);
        assert!(matches!(err, GatewayError::Upstream(_)));
    }
}
