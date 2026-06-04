//! Gateway error type and `IntoResponse` conversion.

use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde_json::json;
use thiserror::Error;

/// Sentinel prefix used by the flag-service to encode the locking
/// experiment ID into a `tonic::Status::failed_precondition` message so
/// this gateway can rebuild the structured 409 body.
const FLAG_LOCKED_STATUS_PREFIX: &str = "flag_locked_by_experiment:";

/// Sentinel prefix the flag-service stamps onto an `INVALID_ARGUMENT` status
/// when a default-rule distribution fails validation, so the gateway can
/// rebuild the structured 422 body without inspecting free-form messages.
/// Mirrors `stitchd_flag_service::error::INVALID_DISTRIBUTION_STATUS_PREFIX`.
const INVALID_DISTRIBUTION_STATUS_PREFIX: &str = "invalid_distribution:";

/// Sentinel prefix the flag-service stamps onto a `FAILED_PRECONDITION` status
/// when an entity delete/archive is blocked because other entities still
/// reference it as a prerequisite/dependency, so the gateway can rebuild the
/// structured `409 DEPENDENCY_EXISTS` body. Mirrors
/// `stitchd_flag_service::prerequisites::DEPENDENCY_EXISTS_STATUS_PREFIX`.
///
/// Format: `"dependency_exists:<comma-separated dependent ids>"`.
const DEPENDENCY_EXISTS_STATUS_PREFIX: &str = "dependency_exists:";

/// Structured error variants returned by gateway handlers. The
/// `*StructuredCode` variants surface a stable `error` discriminator in
/// the JSON body so the admin UI / SDK can branch without parsing the
/// human-readable message.
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

    /// A precondition was not met (e.g. revoke last active SDK key).
    #[error("conflict: {0}")]
    Conflict(String),

    /// The targeted flag is locked because a running/paused experiment is
    /// bound to it. Surfaced as HTTP 409 with body
    /// `{ "error": "flag_locked_by_experiment", "experiment_id": "<uuid>", "message": "..." }`.
    ///
    /// The downstream flag-service produces this via the
    /// `flag_locked_by_experiment:<uuid>` sentinel prefix on its
    /// `tonic::Status::failed_precondition` message; see
    /// `stitchd_flag_service::error::FLAG_LOCKED_STATUS_PREFIX`.
    #[error("flag locked by experiment {experiment_id}")]
    FlagLockedByExperiment { experiment_id: String },

    /// The targeted entity cannot be deleted/archived because other entities
    /// still reference it as a prerequisite/dependency. Surfaced as HTTP 409
    /// with body
    /// `{ "error": "dependency_exists", "dependents": ["<id>", ...], "message": "..." }`.
    ///
    /// The downstream flag-service produces this via the
    /// `dependency_exists:<ids>` sentinel prefix on its
    /// `tonic::Status::failed_precondition` message; see
    /// `stitchd_flag_service::prerequisites::DEPENDENCY_EXISTS_STATUS_PREFIX`.
    #[error("dependency exists: {dependents:?}")]
    DependencyExists { dependents: Vec<String> },

    /// One of the experiment-binding invariants was violated at create /
    /// update time. Mapped to HTTP 422 with body
    /// `{ "error": "<structured_code>", "message": "..." }`.
    ///
    /// `code` is one of:
    /// - `invalid_rule_kind`
    /// - `invalid_default_rule_kind`
    /// - `empty_unit_context_types`
    /// - `unknown_context_type`
    #[error("experiment binding invariant violated: {code}")]
    ExperimentBindingInvalid { code: &'static str, message: String },

    /// A gRPC transport or internal error.
    #[error("upstream error: {0}")]
    Upstream(String),

    /// Request body could not be deserialized.
    #[error("invalid body: {0}")]
    InvalidBody(String),

    /// The required `context_type` query parameter was missing or empty.
    /// Surfaced by Phase 7 endpoints (`/exposures`, `/timeseries`) per spec §5.
    /// Mapped to HTTP 400 with body
    /// `{ "error": "missing_context_type", "message": "..." }`.
    #[error("missing required `context_type` query parameter")]
    MissingContextType,

    /// The required `metric_id` query parameter was missing or empty.
    /// Surfaced by Phase 7 `/timeseries`. Mapped to HTTP 400 with body
    /// `{ "error": "missing_metric_id", "message": "..." }`.
    #[error("missing required `metric_id` query parameter")]
    MissingMetricId,

    /// A `RolloutDistribution` body failed validation.
    /// Mapped to HTTP 422 with body
    /// `{ "error": "invalid_distribution", "message": "..." }`.
    #[error("invalid rollout distribution: {0}")]
    InvalidDistribution(String),
}

impl From<tonic::Status> for GatewayError {
    fn from(s: tonic::Status) -> Self {
        // Decode the flag-lock sentinel before the generic
        // FailedPrecondition → Conflict mapping kicks in.
        if s.code() == tonic::Code::FailedPrecondition
            && let Some(rest) = s.message().strip_prefix(FLAG_LOCKED_STATUS_PREFIX)
        {
            return GatewayError::FlagLockedByExperiment {
                experiment_id: rest.trim().to_string(),
            };
        }
        // Decode the dependency-exists sentinel before the generic
        // FailedPrecondition → Conflict mapping kicks in.
        if s.code() == tonic::Code::FailedPrecondition
            && let Some(rest) = s.message().strip_prefix(DEPENDENCY_EXISTS_STATUS_PREFIX)
        {
            let dependents = rest
                .split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(ToString::to_string)
                .collect();
            return GatewayError::DependencyExists { dependents };
        }
        // Decode the invalid-distribution sentinel before the generic
        // InvalidArgument → BadRequest mapping so it surfaces as a structured 422.
        if s.code() == tonic::Code::InvalidArgument
            && let Some(rest) = s.message().strip_prefix(INVALID_DISTRIBUTION_STATUS_PREFIX)
        {
            return GatewayError::InvalidDistribution(rest.trim().to_string());
        }
        match s.code() {
            tonic::Code::Unauthenticated => GatewayError::Unauthorized(s.message().to_string()),
            tonic::Code::PermissionDenied => GatewayError::Unauthorized(s.message().to_string()),
            tonic::Code::NotFound => GatewayError::NotFound(s.message().to_string()),
            tonic::Code::InvalidArgument => GatewayError::BadRequest(s.message().to_string()),
            tonic::Code::FailedPrecondition => GatewayError::Conflict(s.message().to_string()),
            _ => GatewayError::Upstream(s.message().to_string()),
        }
    }
}

impl IntoResponse for GatewayError {
    fn into_response(self) -> Response {
        match self {
            GatewayError::FlagLockedByExperiment { experiment_id } => {
                let body = Json(json!({
                    "error": "flag_locked_by_experiment",
                    "message": format!(
                        "This flag is locked by experiment {experiment_id} (running or paused). Stop the experiment to modify the flag."
                    ),
                    "experiment_id": experiment_id,
                }));
                (StatusCode::CONFLICT, body).into_response()
            }
            GatewayError::DependencyExists { dependents } => {
                let body = Json(json!({
                    "error": "dependency_exists",
                    "message":
                        "This entity cannot be deleted or archived while other entities reference it as a prerequisite. Remove the references first.",
                    "dependents": dependents,
                }));
                (StatusCode::CONFLICT, body).into_response()
            }
            GatewayError::ExperimentBindingInvalid { code, message } => {
                let body = Json(json!({
                    "error": code,
                    "message": message,
                }));
                (StatusCode::UNPROCESSABLE_ENTITY, body).into_response()
            }
            GatewayError::MissingContextType => {
                let body = Json(json!({
                    "error": "missing_context_type",
                    "message": "the `context_type` query parameter is required and must be one of the experiment's unit_context_types",
                }));
                (StatusCode::BAD_REQUEST, body).into_response()
            }
            GatewayError::MissingMetricId => {
                let body = Json(json!({
                    "error": "missing_metric_id",
                    "message": "the `metric_id` query parameter is required",
                }));
                (StatusCode::BAD_REQUEST, body).into_response()
            }
            GatewayError::InvalidDistribution(msg) => {
                let body = Json(json!({
                    "error": "invalid_distribution",
                    "message": msg,
                }));
                (StatusCode::UNPROCESSABLE_ENTITY, body).into_response()
            }
            other => {
                let (status, msg) = match &other {
                    GatewayError::Unauthorized(m) => (StatusCode::UNAUTHORIZED, m.clone()),
                    GatewayError::NotFound(m) => (StatusCode::NOT_FOUND, m.clone()),
                    GatewayError::BadRequest(m) => (StatusCode::BAD_REQUEST, m.clone()),
                    GatewayError::Conflict(m) => (StatusCode::CONFLICT, m.clone()),
                    GatewayError::Upstream(m) => (StatusCode::BAD_GATEWAY, m.clone()),
                    GatewayError::InvalidBody(m) => (StatusCode::UNPROCESSABLE_ENTITY, m.clone()),
                    GatewayError::FlagLockedByExperiment { .. }
                    | GatewayError::DependencyExists { .. }
                    | GatewayError::ExperimentBindingInvalid { .. }
                    | GatewayError::MissingContextType
                    | GatewayError::MissingMetricId
                    | GatewayError::InvalidDistribution(_) => unreachable!(),
                };
                let body = Json(json!({ "error": msg }));
                (status, body).into_response()
            }
        }
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
    fn invalid_distribution_status_decodes_to_structured_variant() {
        // The flag-service stamps the sentinel onto an InvalidArgument status;
        // the gateway must decode it centrally (not via per-route string checks)
        // into the structured 422 variant.
        let s = tonic::Status::invalid_argument(format!(
            "{INVALID_DISTRIBUTION_STATUS_PREFIX} percentages must sum to 100"
        ));
        let err = GatewayError::from(s);
        match err {
            GatewayError::InvalidDistribution(msg) => {
                assert_eq!(msg, "percentages must sum to 100");
            }
            other => panic!("expected InvalidDistribution, got {other:?}"),
        }
    }

    #[test]
    fn invalid_distribution_response_is_422() {
        let err = GatewayError::InvalidDistribution("percentages must sum to 100".to_string());
        let resp = err.into_response();
        assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[test]
    fn tonic_internal_converts_to_upstream() {
        let s = tonic::Status::internal("server exploded");
        let err = GatewayError::from(s);
        assert!(matches!(err, GatewayError::Upstream(_)));
    }

    #[test]
    fn flag_locked_status_decodes_to_structured_variant() {
        let s = tonic::Status::failed_precondition(format!(
            "{FLAG_LOCKED_STATUS_PREFIX}11111111-1111-1111-1111-111111111111"
        ));
        let err = GatewayError::from(s);
        match err {
            GatewayError::FlagLockedByExperiment { experiment_id } => {
                assert_eq!(experiment_id, "11111111-1111-1111-1111-111111111111");
            }
            other => panic!("expected FlagLockedByExperiment, got {other:?}"),
        }
    }

    #[test]
    fn flag_locked_response_is_409_with_structured_body() {
        let err = GatewayError::FlagLockedByExperiment {
            experiment_id: "exp-123".to_string(),
        };
        let resp = err.into_response();
        assert_eq!(resp.status(), StatusCode::CONFLICT);
    }

    #[test]
    fn experiment_binding_response_is_422() {
        let err = GatewayError::ExperimentBindingInvalid {
            code: "invalid_rule_kind",
            message: "rule must be percentage rollout".to_string(),
        };
        let resp = err.into_response();
        assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[test]
    fn dependency_exists_status_decodes_to_structured_variant() {
        let s = tonic::Status::failed_precondition(format!(
            "{DEPENDENCY_EXISTS_STATUS_PREFIX}11111111-1111-1111-1111-111111111111,22222222-2222-2222-2222-222222222222"
        ));
        let err = GatewayError::from(s);
        match err {
            GatewayError::DependencyExists { dependents } => {
                assert_eq!(dependents.len(), 2);
                assert_eq!(dependents[0], "11111111-1111-1111-1111-111111111111");
                assert_eq!(dependents[1], "22222222-2222-2222-2222-222222222222");
            }
            other => panic!("expected DependencyExists, got {other:?}"),
        }
    }

    #[test]
    fn dependency_exists_response_is_409() {
        let err = GatewayError::DependencyExists {
            dependents: vec!["flag-1".to_string()],
        };
        let resp = err.into_response();
        assert_eq!(resp.status(), StatusCode::CONFLICT);
    }

    #[test]
    fn plain_failed_precondition_still_maps_to_conflict() {
        // A FailedPrecondition without the flag-lock prefix should keep the
        // generic Conflict (409) mapping for back-compat.
        let s = tonic::Status::failed_precondition("revoke last active sdk key");
        let err = GatewayError::from(s);
        assert!(matches!(err, GatewayError::Conflict(_)));
    }
}
