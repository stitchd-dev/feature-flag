//! Context Intelligence routes — surfaced context types and parameter registry.
//!
//! GET /v1/environments/{env_id}/context-types
//! GET /v1/environments/{env_id}/context-types/{context_type}/params

use axum::{
    Json,
    extract::{Path, State},
    response::IntoResponse,
};
use chrono::{Duration, Utc};
use serde::Serialize;
use std::sync::Arc;
use uuid::Uuid;

use stitchd_core::id::EnvironmentId;

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

/// One item in the context-types list response.
#[derive(Debug, Serialize)]
pub struct ContextTypeJson {
    /// Context type name (e.g. `"user"`, `"device"`).
    pub context_type: String,
    /// When this type was last observed (ISO 8601 UTC).
    pub last_seen_at: String,
}

/// One item in the context-type params list response.
#[derive(Debug, Serialize)]
pub struct ContextParamJson {
    /// Parameter key (e.g. `"plan"`, `"email"`).
    pub param_key: String,
    /// Inferred value type.
    pub inferred_type: String,
    /// Whether the parameter was observed with a private (masked) value.
    pub is_private: bool,
    /// When this param was last observed (ISO 8601 UTC).
    pub last_seen_at: String,
}

/// `GET /v1/environments/{env_id}/context-types`
pub async fn list_context_types(
    State(state): State<Arc<GatewayState>>,
    Path(env_id): Path<Uuid>,
    req: axum::extract::Request,
) -> Result<impl IntoResponse, GatewayError> {
    require_permission(&req, "flag:read")?;

    let eid = EnvironmentId::from_uuid(env_id);
    let records = state
        .context_registry
        .list_types(eid)
        .await
        .map_err(|e| GatewayError::Upstream(format!("context registry error: {e}")))?;

    let body: Vec<ContextTypeJson> = records
        .into_iter()
        .map(|r| ContextTypeJson {
            context_type: r.context_type,
            last_seen_at: r.last_seen_at.to_rfc3339(),
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

    let eid = EnvironmentId::from_uuid(env_id);
    let cutoff = Utc::now() - Duration::days(90);

    let records = state
        .context_registry
        .list_params(eid, &context_type)
        .await
        .map_err(|e| GatewayError::Upstream(format!("context registry error: {e}")))?;

    // API-level 90-day safety filter — the repo applies this too, but enforce here defensively.
    let body: Vec<ContextParamJson> = records
        .into_iter()
        .filter(|r| r.last_seen_at >= cutoff)
        .map(|r| ContextParamJson {
            param_key: r.param_key,
            inferred_type: r.inferred_type.to_string(),
            is_private: r.is_private,
            last_seen_at: r.last_seen_at.to_rfc3339(),
        })
        .collect();

    Ok(Json(body))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use stitchd_core::context::{ContextParamRecord, ContextTypeRecord, InferredType};
    use stitchd_core::id::EnvironmentId;
    use uuid::Uuid;

    fn make_env() -> EnvironmentId {
        EnvironmentId::from_uuid(Uuid::new_v4())
    }

    #[test]
    fn context_type_json_fields() {
        let now = Utc::now();
        let eid = make_env();
        let rec = ContextTypeRecord {
            env_id: eid,
            context_type: "user".to_string(),
            first_seen_at: now,
            last_seen_at: now,
        };
        let j = ContextTypeJson {
            context_type: rec.context_type.clone(),
            last_seen_at: rec.last_seen_at.to_rfc3339(),
        };
        assert_eq!(j.context_type, "user");
        assert!(!j.last_seen_at.is_empty());
    }

    #[test]
    fn context_param_json_fields() {
        let now = Utc::now();
        let eid = make_env();
        let rec = ContextParamRecord {
            env_id: eid,
            context_type: "user".to_string(),
            param_key: "plan".to_string(),
            inferred_type: InferredType::Str,
            is_private: false,
            first_seen_at: now,
            last_seen_at: now,
        };
        let j = ContextParamJson {
            param_key: rec.param_key.clone(),
            inferred_type: rec.inferred_type.to_string(),
            is_private: rec.is_private,
            last_seen_at: rec.last_seen_at.to_rfc3339(),
        };
        assert_eq!(j.param_key, "plan");
        assert_eq!(j.inferred_type, "str");
        assert!(!j.is_private);
    }

    #[test]
    fn api_90_day_filter_removes_stale() {
        let now = Utc::now();
        let eid = make_env();
        let stale = now - Duration::days(91);
        let records = vec![
            ContextParamRecord {
                env_id: eid,
                context_type: "user".to_string(),
                param_key: "plan".to_string(),
                inferred_type: InferredType::Str,
                is_private: false,
                first_seen_at: stale,
                last_seen_at: stale,
            },
            ContextParamRecord {
                env_id: eid,
                context_type: "user".to_string(),
                param_key: "email".to_string(),
                inferred_type: InferredType::Str,
                is_private: true,
                first_seen_at: now,
                last_seen_at: now,
            },
        ];
        let cutoff = now - Duration::days(90);
        let fresh: Vec<_> = records.into_iter().filter(|r| r.last_seen_at >= cutoff).collect();
        assert_eq!(fresh.len(), 1);
        assert_eq!(fresh[0].param_key, "email");
    }
}
