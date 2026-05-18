//! Context Intelligence routes — delegates to analytics-service via gRPC.
//!
//! GET /v1/environments/{env_id}/context-types
//! GET /v1/environments/{env_id}/context-types/{context_type}/params

use axum::{
    Json,
    extract::{Path, State},
    response::IntoResponse,
};
use serde::Serialize;
use std::sync::Arc;
use uuid::Uuid;

use stitchd_proto::analytics::v1::{ListContextParamsRequest, ListContextTypesRequest};

use crate::error::GatewayError;
use crate::state::GatewayState;

fn require_permission(req: &axum::extract::Request, permission: &str) -> Result<(), GatewayError> {
    match req
        .extensions()
        .get::<stitchd_proto::auth::v1::RbacContext>()
    {
        Some(ctx) if ctx.permissions.iter().any(|p| p == permission) => Ok(()),
        Some(_) => Err(GatewayError::Unauthorized(format!(
            "missing permission: {permission}"
        ))),
        None => Err(GatewayError::Unauthorized(
            "missing credentials".to_string(),
        )),
    }
}

#[derive(Debug, Serialize)]
pub struct ContextTypeJson {
    pub context_type: String,
    pub last_seen_at: String,
}

#[derive(Debug, Serialize)]
pub struct ContextParamJson {
    pub param_key: String,
    pub inferred_type: String,
    pub is_private: bool,
    pub last_seen_at: String,
}

/// `GET /v1/environments/{env_id}/context-types`
pub async fn list_context_types(
    State(state): State<Arc<GatewayState>>,
    Path(env_id): Path<Uuid>,
    req: axum::extract::Request,
) -> Result<impl IntoResponse, GatewayError> {
    require_permission(&req, "flag:read")?;

    let grpc_req = tonic::Request::new(ListContextTypesRequest {
        environment_id: env_id.to_string(),
    });

    let resp = state
        .analytics_client
        .lock()
        .await
        .list_context_types(grpc_req)
        .await
        .map_err(GatewayError::from)?;

    let body: Vec<ContextTypeJson> = resp
        .into_inner()
        .types
        .into_iter()
        .map(|r| ContextTypeJson {
            context_type: r.context_type,
            last_seen_at: r.last_seen_at,
        })
        .collect();

    Ok(Json(body))
}

/// `GET /v1/environments/{env_id}/context-types/{context_type}/params`
pub async fn list_context_params(
    State(state): State<Arc<GatewayState>>,
    Path((env_id, context_type)): Path<(Uuid, String)>,
    req: axum::extract::Request,
) -> Result<impl IntoResponse, GatewayError> {
    require_permission(&req, "flag:read")?;

    let grpc_req = tonic::Request::new(ListContextParamsRequest {
        environment_id: env_id.to_string(),
        context_type,
    });

    let resp = state
        .analytics_client
        .lock()
        .await
        .list_context_params(grpc_req)
        .await
        .map_err(GatewayError::from)?;

    let body: Vec<ContextParamJson> = resp
        .into_inner()
        .params
        .into_iter()
        .map(|r| ContextParamJson {
            param_key: r.param_key,
            inferred_type: r.inferred_type,
            is_private: r.is_private,
            last_seen_at: r.last_seen_at,
        })
        .collect();

    Ok(Json(body))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn context_type_json_has_expected_fields() {
        let j = ContextTypeJson {
            context_type: "user".into(),
            last_seen_at: "2026-05-01T00:00:00Z".into(),
        };
        assert_eq!(j.context_type, "user");
        assert!(!j.last_seen_at.is_empty());
    }

    #[test]
    fn context_param_json_has_expected_fields() {
        let j = ContextParamJson {
            param_key: "plan".into(),
            inferred_type: "str".into(),
            is_private: false,
            last_seen_at: "2026-05-01T00:00:00Z".into(),
        };
        assert_eq!(j.param_key, "plan");
        assert!(!j.is_private);
    }
}
