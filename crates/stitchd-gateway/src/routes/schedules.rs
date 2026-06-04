//! Scheduled-change route handlers (flag_lifecycle_20260604, Phase 5 Task 4).
//!
//! Entity-scoped CRUD over the schedule-service `ScheduleService` RPCs. A
//! scheduled change targets `(entity_type, entity_id, env_id)` plus a mutation
//! payload and a schedule spec (one-shot instant OR recurring RRULE+IANA-tz);
//! the schedule-service's interval loop applies due changes via the owning
//! service's canonical mutation RPC (flag → MutateFlag, experiment →
//! TransitionExperiment, segment → UpdateAdminSegment).
//!
//! ## URL shape
//! Schedules are environment-scoped (a change carries an `env_id`) and grouped
//! under the targeted entity, mirroring the env-scoped experiment/segment read
//! surface:
//!   * `POST /v1/environments/{environment_id}/{flags|segments|experiments}/{entity_id}/schedules`
//!     — create a scheduled change for that entity.
//!   * `GET  /v1/environments/{environment_id}/{flags|segments|experiments}/{entity_id}/schedules`
//!     — list this entity's scheduled changes.
//!   * `GET  /v1/environments/{environment_id}/schedules/{schedule_id}` — get one
//!     (with run history).
//!   * `POST /v1/environments/{environment_id}/schedules/{schedule_id}/cancel`
//!     `|/pause|/resume` — lifecycle transitions (optimistic `version`).

use std::sync::Arc;

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use stitchd_proto::schedule::v1::{
    CancelScheduledChangeRequest, CreateScheduledChangeRequest, GetScheduledChangeRequest,
    ListScheduledChangesRequest, PauseScheduledChangeRequest, ResumeScheduledChangeRequest,
    ScheduleEntityType, ScheduleKind, ScheduleRunOutcome, ScheduleStatus, ScheduledChange,
    ScheduledChangeRun,
};

use crate::error::GatewayError;
use crate::state::GatewayState;

// ─── REST <-> proto enum mapping ────────────────────────────────────────────

/// Map a URL entity-kind segment (`flags`/`segments`/`experiments`) to the proto
/// [`ScheduleEntityType`]. Returns a 400 for an unrecognized segment.
fn entity_type_from_path(kind: &str) -> Result<ScheduleEntityType, GatewayError> {
    match kind {
        "flags" => Ok(ScheduleEntityType::Flag),
        "segments" => Ok(ScheduleEntityType::Segment),
        "experiments" => Ok(ScheduleEntityType::Experiment),
        other => Err(GatewayError::BadRequest(format!(
            "unknown schedule entity kind '{other}' (expected flags|segments|experiments)"
        ))),
    }
}

fn entity_type_to_str(t: ScheduleEntityType) -> &'static str {
    match t {
        ScheduleEntityType::Unspecified => "unspecified",
        ScheduleEntityType::Flag => "flag",
        ScheduleEntityType::Segment => "segment",
        ScheduleEntityType::Experiment => "experiment",
    }
}

fn schedule_kind_from_str(s: &str) -> Result<ScheduleKind, GatewayError> {
    match s {
        "one_shot" => Ok(ScheduleKind::OneShot),
        "recurring" => Ok(ScheduleKind::Recurring),
        other => Err(GatewayError::BadRequest(format!(
            "unknown schedule_kind '{other}' (expected one_shot|recurring)"
        ))),
    }
}

fn schedule_kind_to_str(k: ScheduleKind) -> &'static str {
    match k {
        ScheduleKind::Unspecified => "unspecified",
        ScheduleKind::OneShot => "one_shot",
        ScheduleKind::Recurring => "recurring",
    }
}

fn status_to_str(s: ScheduleStatus) -> &'static str {
    match s {
        ScheduleStatus::Unspecified => "unspecified",
        ScheduleStatus::Pending => "pending",
        ScheduleStatus::Active => "active",
        ScheduleStatus::Paused => "paused",
        ScheduleStatus::Applied => "applied",
        ScheduleStatus::Failed => "failed",
        ScheduleStatus::Cancelled => "cancelled",
    }
}

fn run_outcome_to_str(o: ScheduleRunOutcome) -> &'static str {
    match o {
        ScheduleRunOutcome::Unspecified => "unspecified",
        ScheduleRunOutcome::Applied => "applied",
        ScheduleRunOutcome::Skipped => "skipped",
        ScheduleRunOutcome::Failed => "failed",
    }
}

// ─── REST types ─────────────────────────────────────────────────────────────

/// Request body to create a scheduled change.
#[derive(Debug, Deserialize, ToSchema)]
pub struct CreateScheduleBody {
    /// JSON-encoded mutation the owning service's RPC consumes at fire time.
    pub mutation_payload: serde_json::Value,
    /// `one_shot` | `recurring`.
    pub schedule_kind: String,
    /// One-shot: epoch-ms instant to fire (UTC). Required for `one_shot`.
    #[serde(default)]
    pub scheduled_at_ms: Option<i64>,
    /// Recurring: RFC-5545 RRULE string. Required for `recurring`.
    #[serde(default)]
    pub rrule: Option<String>,
    /// Recurring: IANA timezone the RRULE is evaluated in.
    #[serde(default)]
    pub tz: Option<String>,
}

/// A single firing of a scheduled change.
#[derive(Debug, Serialize, ToSchema)]
pub struct ScheduleRunJson {
    pub id: String,
    pub fired_at_ms: i64,
    /// `applied` | `skipped` | `failed`.
    pub outcome: String,
    /// Free-text reason / error detail.
    pub detail: String,
}

/// JSON representation of a scheduled change.
#[derive(Debug, Serialize, ToSchema)]
pub struct ScheduleJson {
    pub id: String,
    /// `flag` | `segment` | `experiment`.
    pub entity_type: String,
    pub entity_id: String,
    pub env_id: String,
    pub mutation_payload: serde_json::Value,
    /// `one_shot` | `recurring`.
    pub schedule_kind: String,
    pub scheduled_at_ms: i64,
    pub rrule: String,
    pub tz: String,
    /// `pending` | `active` | `paused` | `applied` | `failed` | `cancelled`.
    pub status: String,
    pub next_run_at_ms: i64,
    pub last_run_at_ms: i64,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
    pub version: u64,
    /// Run history (most-recent-first); populated on get, empty on list.
    pub runs: Vec<ScheduleRunJson>,
}

/// Request body for a lifecycle transition (cancel/pause/resume).
#[derive(Debug, Deserialize, ToSchema)]
pub struct ScheduleVersionBody {
    /// Optimistic-concurrency version the caller last observed.
    #[serde(default)]
    pub version: u64,
}

fn run_to_json(r: &ScheduledChangeRun) -> ScheduleRunJson {
    ScheduleRunJson {
        id: r.id.clone(),
        fired_at_ms: r.fired_at_ms,
        outcome: run_outcome_to_str(r.outcome()).to_string(),
        detail: r.detail.clone(),
    }
}

fn change_to_json(c: &ScheduledChange) -> ScheduleJson {
    ScheduleJson {
        id: c.id.clone(),
        entity_type: entity_type_to_str(c.entity_type()).to_string(),
        entity_id: c.entity_id.clone(),
        env_id: c.env_id.clone(),
        mutation_payload: serde_json::from_str(&c.mutation_payload_json)
            .unwrap_or(serde_json::Value::Null),
        schedule_kind: schedule_kind_to_str(c.schedule_kind()).to_string(),
        scheduled_at_ms: c.scheduled_at_ms,
        rrule: c.rrule.clone(),
        tz: c.tz.clone(),
        status: status_to_str(c.status()).to_string(),
        next_run_at_ms: c.next_run_at_ms,
        last_run_at_ms: c.last_run_at_ms,
        created_at_ms: c.created_at_ms,
        updated_at_ms: c.updated_at_ms,
        version: c.version,
        runs: c.runs.iter().map(run_to_json).collect(),
    }
}

// ─── Handlers ───────────────────────────────────────────────────────────────

/// `POST /v1/environments/{environment_id}/{entity_kind}/{entity_id}/schedules`
/// — create a scheduled change for the targeted entity.
#[utoipa::path(
    post,
    path = "/v1/environments/{environment_id}/{entity_kind}/{entity_id}/schedules",
    tag = "schedules",
    params(
        ("environment_id" = String, Path, description = "Environment ID"),
        ("entity_kind" = String, Path, description = "flags | segments | experiments"),
        ("entity_id" = String, Path, description = "Targeted entity ID"),
    ),
    request_body = CreateScheduleBody,
    responses(
        (status = 201, description = "Scheduled change created", body = ScheduleJson),
        (status = 400, description = "Bad entity kind / schedule kind / missing spec"),
        (status = 401, description = "Unauthorized"),
        (status = 502, description = "Schedule service unavailable"),
    ),
    security(("bearer_jwt" = []))
)]
pub async fn create_schedule(
    State(state): State<Arc<GatewayState>>,
    Path((environment_id, entity_kind, entity_id)): Path<(String, String, String)>,
    Json(body): Json<CreateScheduleBody>,
) -> Result<impl IntoResponse, GatewayError> {
    let entity_type = entity_type_from_path(&entity_kind)?;
    let schedule_kind = schedule_kind_from_str(&body.schedule_kind)?;

    let rpc = tonic::Request::new(CreateScheduledChangeRequest {
        entity_type: entity_type as i32,
        entity_id,
        env_id: environment_id,
        mutation_payload_json: body.mutation_payload.to_string(),
        schedule_kind: schedule_kind as i32,
        scheduled_at_ms: body.scheduled_at_ms.unwrap_or(0),
        rrule: body.rrule.unwrap_or_default(),
        tz: body.tz.unwrap_or_default(),
    });

    let mut client = state.schedule_client.lock().await;
    let change = client
        .create_scheduled_change(rpc)
        .await
        .map_err(GatewayError::from)?
        .into_inner();
    Ok((StatusCode::CREATED, Json(change_to_json(&change))))
}

/// `GET /v1/environments/{environment_id}/{entity_kind}/{entity_id}/schedules`
/// — list scheduled changes for the targeted entity.
#[utoipa::path(
    get,
    path = "/v1/environments/{environment_id}/{entity_kind}/{entity_id}/schedules",
    tag = "schedules",
    params(
        ("environment_id" = String, Path, description = "Environment ID"),
        ("entity_kind" = String, Path, description = "flags | segments | experiments"),
        ("entity_id" = String, Path, description = "Targeted entity ID"),
    ),
    responses(
        (status = 200, description = "List of scheduled changes", body = Vec<ScheduleJson>),
        (status = 400, description = "Bad entity kind"),
        (status = 401, description = "Unauthorized"),
        (status = 502, description = "Schedule service unavailable"),
    ),
    security(("bearer_jwt" = []))
)]
pub async fn list_schedules(
    State(state): State<Arc<GatewayState>>,
    Path((environment_id, entity_kind, entity_id)): Path<(String, String, String)>,
) -> Result<impl IntoResponse, GatewayError> {
    let entity_type = entity_type_from_path(&entity_kind)?;
    let rpc = tonic::Request::new(ListScheduledChangesRequest {
        env_id: environment_id,
        entity_type: entity_type as i32,
        entity_id,
        status: ScheduleStatus::Unspecified as i32,
    });
    let mut client = state.schedule_client.lock().await;
    let resp = client
        .list_scheduled_changes(rpc)
        .await
        .map_err(GatewayError::from)?
        .into_inner();
    let items: Vec<ScheduleJson> = resp.changes.iter().map(change_to_json).collect();
    Ok(Json(items))
}

/// `GET /v1/environments/{environment_id}/schedules/{schedule_id}` — get one
/// scheduled change (with run history).
#[utoipa::path(
    get,
    path = "/v1/environments/{environment_id}/schedules/{schedule_id}",
    tag = "schedules",
    params(
        ("environment_id" = String, Path, description = "Environment ID"),
        ("schedule_id" = String, Path, description = "Scheduled-change ID"),
    ),
    responses(
        (status = 200, description = "The scheduled change", body = ScheduleJson),
        (status = 404, description = "Not found"),
        (status = 401, description = "Unauthorized"),
        (status = 502, description = "Schedule service unavailable"),
    ),
    security(("bearer_jwt" = []))
)]
pub async fn get_schedule(
    State(state): State<Arc<GatewayState>>,
    Path((_environment_id, schedule_id)): Path<(String, String)>,
) -> Result<impl IntoResponse, GatewayError> {
    let rpc = tonic::Request::new(GetScheduledChangeRequest { id: schedule_id });
    let mut client = state.schedule_client.lock().await;
    let change = client
        .get_scheduled_change(rpc)
        .await
        .map_err(GatewayError::from)?
        .into_inner();
    Ok(Json(change_to_json(&change)))
}

/// `POST /v1/environments/{environment_id}/schedules/{schedule_id}/cancel`
/// — cancel a pending one-shot change.
#[utoipa::path(
    post,
    path = "/v1/environments/{environment_id}/schedules/{schedule_id}/cancel",
    tag = "schedules",
    params(
        ("environment_id" = String, Path, description = "Environment ID"),
        ("schedule_id" = String, Path, description = "Scheduled-change ID"),
    ),
    request_body = ScheduleVersionBody,
    responses(
        (status = 200, description = "Cancelled", body = ScheduleJson),
        (status = 404, description = "Not found"),
        (status = 409, description = "Version conflict / invalid state"),
        (status = 401, description = "Unauthorized"),
        (status = 502, description = "Schedule service unavailable"),
    ),
    security(("bearer_jwt" = []))
)]
pub async fn cancel_schedule(
    State(state): State<Arc<GatewayState>>,
    Path((_environment_id, schedule_id)): Path<(String, String)>,
    Json(body): Json<ScheduleVersionBody>,
) -> Result<impl IntoResponse, GatewayError> {
    let rpc = tonic::Request::new(CancelScheduledChangeRequest {
        id: schedule_id,
        version: body.version,
    });
    let mut client = state.schedule_client.lock().await;
    let change = client
        .cancel_scheduled_change(rpc)
        .await
        .map_err(GatewayError::from)?
        .into_inner();
    Ok(Json(change_to_json(&change)))
}

/// `POST /v1/environments/{environment_id}/schedules/{schedule_id}/pause`
/// — pause a recurring change.
#[utoipa::path(
    post,
    path = "/v1/environments/{environment_id}/schedules/{schedule_id}/pause",
    tag = "schedules",
    params(
        ("environment_id" = String, Path, description = "Environment ID"),
        ("schedule_id" = String, Path, description = "Scheduled-change ID"),
    ),
    request_body = ScheduleVersionBody,
    responses(
        (status = 200, description = "Paused", body = ScheduleJson),
        (status = 404, description = "Not found"),
        (status = 409, description = "Version conflict / invalid state"),
        (status = 401, description = "Unauthorized"),
        (status = 502, description = "Schedule service unavailable"),
    ),
    security(("bearer_jwt" = []))
)]
pub async fn pause_schedule(
    State(state): State<Arc<GatewayState>>,
    Path((_environment_id, schedule_id)): Path<(String, String)>,
    Json(body): Json<ScheduleVersionBody>,
) -> Result<impl IntoResponse, GatewayError> {
    let rpc = tonic::Request::new(PauseScheduledChangeRequest {
        id: schedule_id,
        version: body.version,
    });
    let mut client = state.schedule_client.lock().await;
    let change = client
        .pause_scheduled_change(rpc)
        .await
        .map_err(GatewayError::from)?
        .into_inner();
    Ok(Json(change_to_json(&change)))
}

/// `POST /v1/environments/{environment_id}/schedules/{schedule_id}/resume`
/// — resume a paused recurring change.
#[utoipa::path(
    post,
    path = "/v1/environments/{environment_id}/schedules/{schedule_id}/resume",
    tag = "schedules",
    params(
        ("environment_id" = String, Path, description = "Environment ID"),
        ("schedule_id" = String, Path, description = "Scheduled-change ID"),
    ),
    request_body = ScheduleVersionBody,
    responses(
        (status = 200, description = "Resumed", body = ScheduleJson),
        (status = 404, description = "Not found"),
        (status = 409, description = "Version conflict / invalid state"),
        (status = 401, description = "Unauthorized"),
        (status = 502, description = "Schedule service unavailable"),
    ),
    security(("bearer_jwt" = []))
)]
pub async fn resume_schedule(
    State(state): State<Arc<GatewayState>>,
    Path((_environment_id, schedule_id)): Path<(String, String)>,
    Json(body): Json<ScheduleVersionBody>,
) -> Result<impl IntoResponse, GatewayError> {
    let rpc = tonic::Request::new(ResumeScheduledChangeRequest {
        id: schedule_id,
        version: body.version,
    });
    let mut client = state.schedule_client.lock().await;
    let change = client
        .resume_scheduled_change(rpc)
        .await
        .map_err(GatewayError::from)?
        .into_inner();
    Ok(Json(change_to_json(&change)))
}
