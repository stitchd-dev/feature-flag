//! Exclusion-group route handlers — proxy REST requests to the
//! Experimentation Service via gRPC.
//!
//! An exclusion group partitions an environment's traffic into disjoint
//! basis-point slices so member experiments never co-assign the same context.
//! `allocated_bp` + `free_bp` always sum to the group's total budget; the
//! gateway exposes both plus the `version` for optimistic concurrency.
//!
//! The gateway carries ZERO domain logic — these handlers are pure
//! translation + auth + canonical error mapping. The `flag_locked_by_experiment`
//! sentinel returned by assign/unassign is decoded centrally in
//! `GatewayError::from(tonic::Status)` into the structured 409 body.

use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use utoipa::ToSchema;

use stitchd_proto::experiments::v1::{
    AssignExperimentToGroupRequest, CreateExclusionGroupRequest, DeleteExclusionGroupRequest,
    ExclusionGroup, GetExclusionGroupRequest, ListExclusionGroupsRequest,
    UnassignExperimentRequest, UpdateExclusionGroupRequest,
};

use crate::error::GatewayError;
use crate::pagination::{CursorPage, CursorParams};
use crate::routes::experiments::ExperimentJson;
use crate::state::GatewayState;

// ─── REST types ───────────────────────────────────────────────────────────────

/// Query parameters for listing exclusion groups.
#[derive(Debug, Deserialize, ToSchema)]
pub struct ListExclusionGroupsQuery {
    #[serde(flatten)]
    pub cursor: CursorParams,
}

/// Request body for `POST /exclusion-groups`.
#[derive(Debug, Deserialize, ToSchema)]
pub struct CreateExclusionGroupBody {
    pub name: Option<String>,
    pub description: Option<String>,
    /// The group's diversion (randomization) unit context type, e.g. "user".
    /// All member experiments must randomize on this unit for mutual exclusion
    /// to hold. Defaults to "user" when omitted. Immutable after creation.
    pub unit_context_type: Option<String>,
}

/// Request body for `PATCH /exclusion-groups/{group_id}`.
#[derive(Debug, Deserialize, ToSchema)]
pub struct UpdateExclusionGroupBody {
    pub name: Option<String>,
    pub description: Option<String>,
    /// Optimistic-concurrency version; the service rejects stale writes.
    pub version: Option<i64>,
}

/// Request body for assigning an experiment to a group.
#[derive(Debug, Deserialize, ToSchema)]
pub struct AssignExperimentBody {
    pub group_id: String,
    /// Basis points to carve out of the group's free budget for this
    /// experiment.
    pub requested_bp: u32,
}

/// JSON view of an exclusion group, exposing allocated/free capacity.
#[derive(Debug, Serialize, ToSchema)]
pub struct ExclusionGroupJson {
    pub id: String,
    pub env_id: String,
    pub name: String,
    pub description: String,
    /// Basis points currently allocated to member experiments.
    pub allocated_bp: u32,
    /// Basis points still available for new assignments.
    pub free_bp: u32,
    /// Optimistic-concurrency version, bumped on every mutation.
    pub version: i64,
    /// The group's diversion (randomization) unit context type, e.g. "user".
    /// All member experiments must randomize on this unit. Immutable.
    pub unit_context_type: String,
}

/// Response for assign / unassign — the updated experiment alongside the
/// group reflecting the new allocated/free split.
#[derive(Debug, Serialize, ToSchema)]
pub struct AssignmentResultJson {
    pub experiment: ExperimentJson,
    pub group: ExclusionGroupJson,
}

fn exclusion_group_to_json(g: &ExclusionGroup) -> ExclusionGroupJson {
    ExclusionGroupJson {
        id: g.id.clone(),
        env_id: g.env_id.clone(),
        name: g.name.clone(),
        description: g.description.clone(),
        allocated_bp: g.allocated_bp,
        free_bp: g.free_bp,
        version: g.version,
        unit_context_type: g.unit_context_type.clone(),
    }
}

// ─── Handlers ────────────────────────────────────────────────────────────────

/// `GET /v1/environments/{environment_id}/exclusion-groups`
#[utoipa::path(
    get,
    path = "/v1/environments/{environment_id}/exclusion-groups",
    tag = "exclusion-groups",
    params(
        ("environment_id" = String, Path, description = "Environment ID"),
        ("cursor" = Option<String>, Query, description = "Opaque pagination cursor from a previous response"),
        ("limit" = Option<u32>, Query, description = "Page size (default 50, max 200)"),
    ),
    responses(
        (status = 200, description = "Cursor-paginated list of exclusion groups"),
        (status = 401, description = "Unauthorized"),
        (status = 502, description = "Experimentation service unavailable"),
    ),
    security(("bearer_jwt" = []))
)]
pub async fn list_exclusion_groups(
    State(state): State<Arc<GatewayState>>,
    Path(environment_id): Path<String>,
    Query(query): Query<ListExclusionGroupsQuery>,
) -> Result<impl IntoResponse, GatewayError> {
    let offset = query
        .cursor
        .offset()
        .map_err(|_| GatewayError::BadRequest("invalid cursor".into()))?;
    let limit = query.cursor.effective_limit();
    let req = tonic::Request::new(ListExclusionGroupsRequest {
        env_id: environment_id,
        page: (offset / u64::from(limit)) as u32 + 1,
        per_page: limit,
    });
    let mut client = state.experimentation_client.lock().await;
    let resp = client
        .list_exclusion_groups(req)
        .await
        .map_err(GatewayError::from)?;
    let inner = resp.into_inner();
    let groups: Vec<ExclusionGroupJson> =
        inner.groups.iter().map(exclusion_group_to_json).collect();
    Ok(Json(CursorPage::from_offset(groups, inner.total, offset)))
}

/// `POST /v1/environments/{environment_id}/exclusion-groups`
#[utoipa::path(
    post,
    path = "/v1/environments/{environment_id}/exclusion-groups",
    tag = "exclusion-groups",
    params(("environment_id" = String, Path, description = "Environment ID")),
    request_body = CreateExclusionGroupBody,
    responses(
        (status = 201, description = "Exclusion group created", body = ExclusionGroupJson),
        (status = 401, description = "Unauthorized"),
        (status = 502, description = "Experimentation service unavailable"),
    ),
    security(("bearer_jwt" = []))
)]
pub async fn create_exclusion_group(
    State(state): State<Arc<GatewayState>>,
    Path(environment_id): Path<String>,
    Json(body): Json<CreateExclusionGroupBody>,
) -> Result<impl IntoResponse, GatewayError> {
    let group = ExclusionGroup {
        env_id: environment_id,
        name: body.name.unwrap_or_default(),
        description: body.description.unwrap_or_default(),
        unit_context_type: body
            .unit_context_type
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| "user".to_string()),
        ..Default::default()
    };
    let req = tonic::Request::new(CreateExclusionGroupRequest { group: Some(group) });
    let mut client = state.experimentation_client.lock().await;
    let resp = client
        .create_exclusion_group(req)
        .await
        .map_err(GatewayError::from)?;
    Ok((
        StatusCode::CREATED,
        Json(exclusion_group_to_json(&resp.into_inner())),
    ))
}

/// `GET /v1/environments/{environment_id}/exclusion-groups/{group_id}`
///
/// Fetches a single group via the experimentation-service `GetExclusionGroup`
/// RPC, surfacing the upstream NOT_FOUND as a 404.
#[utoipa::path(
    get,
    path = "/v1/environments/{environment_id}/exclusion-groups/{group_id}",
    tag = "exclusion-groups",
    params(
        ("environment_id" = String, Path, description = "Environment ID"),
        ("group_id" = String, Path, description = "Exclusion group ID"),
    ),
    responses(
        (status = 200, description = "Exclusion group", body = ExclusionGroupJson),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "Group not found"),
        (status = 502, description = "Experimentation service unavailable"),
    ),
    security(("bearer_jwt" = []))
)]
pub async fn get_exclusion_group(
    State(state): State<Arc<GatewayState>>,
    Path((environment_id, group_id)): Path<(String, String)>,
) -> Result<impl IntoResponse, GatewayError> {
    let req = tonic::Request::new(GetExclusionGroupRequest {
        env_id: environment_id,
        group_id,
    });
    let mut client = state.experimentation_client.lock().await;
    let resp = client
        .get_exclusion_group(req)
        .await
        .map_err(GatewayError::from)?;
    Ok(Json(exclusion_group_to_json(&resp.into_inner())))
}

/// `PATCH /v1/environments/{environment_id}/exclusion-groups/{group_id}`
#[utoipa::path(
    patch,
    path = "/v1/environments/{environment_id}/exclusion-groups/{group_id}",
    tag = "exclusion-groups",
    params(
        ("environment_id" = String, Path, description = "Environment ID"),
        ("group_id" = String, Path, description = "Exclusion group ID"),
    ),
    request_body = UpdateExclusionGroupBody,
    responses(
        (status = 200, description = "Updated exclusion group", body = ExclusionGroupJson),
        (status = 401, description = "Unauthorized"),
        (status = 502, description = "Experimentation service unavailable"),
    ),
    security(("bearer_jwt" = []))
)]
pub async fn update_exclusion_group(
    State(state): State<Arc<GatewayState>>,
    Path((environment_id, group_id)): Path<(String, String)>,
    Json(body): Json<UpdateExclusionGroupBody>,
) -> Result<impl IntoResponse, GatewayError> {
    let group = ExclusionGroup {
        id: group_id,
        env_id: environment_id,
        name: body.name.unwrap_or_default(),
        description: body.description.unwrap_or_default(),
        version: body.version.unwrap_or(0),
        ..Default::default()
    };
    let req = tonic::Request::new(UpdateExclusionGroupRequest { group: Some(group) });
    let mut client = state.experimentation_client.lock().await;
    let resp = client
        .update_exclusion_group(req)
        .await
        .map_err(GatewayError::from)?;
    Ok(Json(exclusion_group_to_json(&resp.into_inner())))
}

/// `DELETE /v1/environments/{environment_id}/exclusion-groups/{group_id}`
#[utoipa::path(
    delete,
    path = "/v1/environments/{environment_id}/exclusion-groups/{group_id}",
    tag = "exclusion-groups",
    params(
        ("environment_id" = String, Path, description = "Environment ID"),
        ("group_id" = String, Path, description = "Exclusion group ID"),
    ),
    responses(
        (status = 204, description = "Exclusion group deleted"),
        (status = 401, description = "Unauthorized"),
        (status = 502, description = "Experimentation service unavailable"),
    ),
    security(("bearer_jwt" = []))
)]
pub async fn delete_exclusion_group(
    State(state): State<Arc<GatewayState>>,
    Path((environment_id, group_id)): Path<(String, String)>,
) -> Result<impl IntoResponse, GatewayError> {
    let req = tonic::Request::new(DeleteExclusionGroupRequest {
        env_id: environment_id,
        group_id,
    });
    let mut client = state.experimentation_client.lock().await;
    client
        .delete_exclusion_group(req)
        .await
        .map_err(GatewayError::from)?;
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /v1/environments/{environment_id}/experiments/{experiment_id}/exclusion-group`
///
/// Assigns an experiment to an exclusion group, carving `requested_bp` basis
/// points out of the group's free budget. If the parent flag is locked by a
/// running/paused experiment the service returns the
/// `flag_locked_by_experiment` sentinel, which the gateway surfaces as the
/// structured 409.
#[utoipa::path(
    post,
    path = "/v1/environments/{environment_id}/experiments/{experiment_id}/exclusion-group",
    tag = "exclusion-groups",
    params(
        ("environment_id" = String, Path, description = "Environment ID"),
        ("experiment_id" = String, Path, description = "Experiment ID"),
    ),
    request_body = AssignExperimentBody,
    responses(
        (status = 200, description = "Experiment assigned to group", body = AssignmentResultJson),
        (status = 401, description = "Unauthorized"),
        (status = 409, description = "Capacity exceeded or flag locked by experiment"),
        (status = 502, description = "Experimentation service unavailable"),
    ),
    security(("bearer_jwt" = []))
)]
pub async fn assign_experiment_to_group(
    State(state): State<Arc<GatewayState>>,
    Path((environment_id, experiment_id)): Path<(String, String)>,
    Json(body): Json<AssignExperimentBody>,
) -> Result<impl IntoResponse, GatewayError> {
    let req = tonic::Request::new(AssignExperimentToGroupRequest {
        env_id: environment_id,
        group_id: body.group_id,
        experiment_id,
        requested_bp: body.requested_bp,
    });
    let mut client = state.experimentation_client.lock().await;
    let resp = client
        .assign_experiment_to_group(req)
        .await
        .map_err(GatewayError::from)?;
    let inner = resp.into_inner();
    let group = inner
        .group
        .as_ref()
        .map(exclusion_group_to_json)
        .ok_or_else(|| GatewayError::Upstream("assign response missing group".to_string()))?;
    let experiment = inner
        .experiment
        .as_ref()
        .map(crate::routes::experiments::experiment_to_json)
        .ok_or_else(|| GatewayError::Upstream("assign response missing experiment".to_string()))?;
    Ok(Json(AssignmentResultJson { experiment, group }))
}

/// `DELETE /v1/environments/{environment_id}/experiments/{experiment_id}/exclusion-group`
///
/// Removes an experiment from its exclusion group, returning the freed
/// capacity to the group's budget.
#[utoipa::path(
    delete,
    path = "/v1/environments/{environment_id}/experiments/{experiment_id}/exclusion-group",
    tag = "exclusion-groups",
    params(
        ("environment_id" = String, Path, description = "Environment ID"),
        ("experiment_id" = String, Path, description = "Experiment ID"),
    ),
    responses(
        (status = 200, description = "Experiment unassigned from group", body = AssignmentResultJson),
        (status = 401, description = "Unauthorized"),
        (status = 409, description = "Flag locked by experiment"),
        (status = 502, description = "Experimentation service unavailable"),
    ),
    security(("bearer_jwt" = []))
)]
pub async fn unassign_experiment(
    State(state): State<Arc<GatewayState>>,
    Path((environment_id, experiment_id)): Path<(String, String)>,
) -> Result<impl IntoResponse, GatewayError> {
    let req = tonic::Request::new(UnassignExperimentRequest {
        env_id: environment_id,
        experiment_id,
    });
    let mut client = state.experimentation_client.lock().await;
    let resp = client
        .unassign_experiment(req)
        .await
        .map_err(GatewayError::from)?;
    let inner = resp.into_inner();
    let group = inner
        .group
        .as_ref()
        .map(exclusion_group_to_json)
        .ok_or_else(|| GatewayError::Upstream("unassign response missing group".to_string()))?;
    let experiment = inner
        .experiment
        .as_ref()
        .map(crate::routes::experiments::experiment_to_json)
        .ok_or_else(|| {
            GatewayError::Upstream("unassign response missing experiment".to_string())
        })?;
    Ok(Json(AssignmentResultJson { experiment, group }))
}

#[cfg(test)]
pub fn test_router(state: Arc<GatewayState>) -> axum::Router {
    use axum::routing::{get, post};
    axum::Router::new()
        .route(
            "/v1/environments/{environment_id}/exclusion-groups",
            get(list_exclusion_groups).post(create_exclusion_group),
        )
        .route(
            "/v1/environments/{environment_id}/exclusion-groups/{group_id}",
            get(get_exclusion_group)
                .patch(update_exclusion_group)
                .delete(delete_exclusion_group),
        )
        .route(
            "/v1/environments/{environment_id}/experiments/{experiment_id}/exclusion-group",
            post(assign_experiment_to_group).delete(unassign_experiment),
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

    #[test]
    fn exclusion_group_to_json_maps_fields() {
        let g = ExclusionGroup {
            id: "grp-1".to_string(),
            env_id: "env-1".to_string(),
            name: "Checkout".to_string(),
            description: "checkout funnel".to_string(),
            allocated_bp: 3000,
            free_bp: 7000,
            version: 4,
            unit_context_type: "account".to_string(),
        };
        let j = exclusion_group_to_json(&g);
        assert_eq!(j.id, "grp-1");
        assert_eq!(j.env_id, "env-1");
        assert_eq!(j.name, "Checkout");
        assert_eq!(j.allocated_bp, 3000);
        assert_eq!(j.free_bp, 7000);
        assert_eq!(j.version, 4);
        assert_eq!(j.unit_context_type, "account");
    }

    #[tokio::test]
    async fn list_groups_returns_200_or_502() {
        let state = make_stub_state();
        let app = test_router(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/v1/environments/env-1/exclusion-groups")
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
    async fn create_group_returns_201_or_502() {
        let state = make_stub_state();
        let app = test_router(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/environments/env-1/exclusion-groups")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"name":"Checkout","description":"funnel"}"#))
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
    async fn update_group_returns_200_or_502() {
        let state = make_stub_state();
        let app = test_router(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .method("PATCH")
                    .uri("/v1/environments/env-1/exclusion-groups/grp-1")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"name":"Renamed","version":1}"#))
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
    async fn delete_group_returns_204_or_502() {
        let state = make_stub_state();
        let app = test_router(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri("/v1/environments/env-1/exclusion-groups/grp-1")
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

    #[tokio::test]
    async fn assign_returns_200_or_502() {
        let state = make_stub_state();
        let app = test_router(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/environments/env-1/experiments/exp-1/exclusion-group")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"group_id":"grp-1","requested_bp":2500}"#))
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
    async fn unassign_returns_200_or_502() {
        let state = make_stub_state();
        let app = test_router(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri("/v1/environments/env-1/experiments/exp-1/exclusion-group")
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
}
