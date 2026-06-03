//! Experiment route handlers — proxy REST requests to the Experimentation Service via gRPC.

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
    BoundTarget, ContextTypeResults, CreateExperimentRequest, DeleteExperimentRequest, Experiment,
    ExperimentInteraction, ExperimentIteration, ExperimentStatus, GetExperimentInteractionsRequest,
    GetExperimentRequest, GetResultsRequest, ListExperimentsRequest, ListExposuresRequest,
    ListIterationsRequest, SrmResult as ProtoSrmResult, TransitionExperimentRequest,
    UpdateExperimentRequest, VariantResult,
};

use crate::error::GatewayError;
use crate::pagination::{PaginatedResponse, PaginationParams};
use crate::state::GatewayState;

// ─── REST types ───────────────────────────────────────────────────────────────

/// Query parameters for listing experiments.
#[derive(Debug, Deserialize, ToSchema)]
pub struct ListExperimentsQuery {
    #[serde(flatten)]
    pub pagination: PaginationParams,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct CreateExperimentBody {
    pub name: Option<String>,
    pub description: Option<String>,
    pub flag_key: Option<String>,
    pub variant_keys: Option<Vec<String>>,
    /// When set, the experiment binds to a specific rule on the flag —
    /// the rule's output MUST be a percentage rollout (Allocation), else
    /// the request is rejected with 422 `INVALID_RULE_KIND`. Mutually
    /// exclusive with `targets_default_rule`.
    pub flag_rule_id: Option<String>,
    /// When `true`, the experiment binds to the flag's default-rule
    /// percentage-distribution fall-through. The parent flag must have
    /// `default_rule_distribution` configured (validated against the
    /// flag's `default_rule_distribution` JSONB column once Phase 7 wires
    /// it through the proto; for Phase 3 we enforce only the XOR
    /// invariant with `flag_rule_id`).
    #[serde(default)]
    pub targets_default_rule: bool,
    /// Analysis unit context types. At least one entry is required and
    /// each entry must be registered in `context_type_registry` for the
    /// environment.
    #[serde(default)]
    pub unit_context_types: Vec<String>,
    /// Optional metric UUIDs to treat as guardrails (direction-violation alerts).
    #[serde(default)]
    pub guardrail_metric_ids: Vec<String>,
    /// CUPED pre-period window in days; `0` disables variance reduction.
    #[serde(default)]
    pub pre_period_days: u32,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct UpdateExperimentBody {
    pub name: Option<String>,
    pub description: Option<String>,
    pub variant_keys: Option<Vec<String>>,
    pub version: Option<u64>,
    pub flag_key: Option<String>,
    pub flag_rule_id: Option<String>,
    #[serde(default)]
    pub targets_default_rule: bool,
    #[serde(default)]
    pub unit_context_types: Vec<String>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct TransitionBody {
    pub new_status: String,
    pub reason: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct IterationJson {
    pub id: String,
    pub experiment_id: String,
    pub iteration_number: i32,
    pub started_at_ms: i64,
    pub ended_at_ms: i64,
    pub traffic_allocation: f64,
    /// Snapshot of `unit_context_types` at iteration start. Used by the admin
    /// Iterations tab to render per-iteration context-type badges.
    pub unit_context_types: Vec<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ExperimentJson {
    pub id: String,
    /// Alias for `id` — experiments are addressed by UUID; kept for UI compat.
    pub key: String,
    pub environment_id: String,
    pub name: String,
    pub description: String,
    pub flag_key: String,
    pub status: String,
    pub model: String,
    pub variant_keys: Vec<String>,
    /// Number of variants (derived from `variant_keys.len()`).
    pub variants: u32,
    /// Metric definition UUIDs attached to this experiment. Empty until the
    /// experimentation-service proto carries metric_ids (Phase 7 gap).
    pub metric_ids: Vec<String>,
    /// ISO-8601 UTC timestamp; derived from the proto `created_at_ms` field.
    pub created_at: String,
    /// ISO-8601 UTC timestamp; derived from the proto `updated_at_ms` field.
    pub updated_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub started_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<String>,
    pub unit_context_types: Vec<String>,
}

/// Per-variant result row — used in both `variants` and `guardrails` buckets
/// of a [`ContextTypeResultsJson`]. The `direction_violation` flag is only
/// load-bearing for guardrail rows.
#[derive(Debug, Serialize, ToSchema)]
pub struct VariantResultJson {
    pub variant_key: String,
    pub participant_count: u64,
    /// Frequentist p-value when present; absent (omitted from JSON) otherwise.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub p_value: Option<f64>,
    /// Multiple-comparison-corrected p-value (Bonferroni); only set when
    /// multiple metrics were analysed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub p_value_corrected: Option<f64>,
    /// Observed `treatment − control` lift. `0.0` for control rows.
    pub lift: f64,
    /// `true` only for guardrail rows that violate the metric's
    /// `goal_direction`.
    pub direction_violation: bool,
    /// Attribution unit context type ("user", "account", …).
    pub context_type: String,
}

/// SRM (Sample Ratio Mismatch) breakdown for one context type.
#[derive(Debug, Serialize, ToSchema)]
pub struct SrmPerVariantJson {
    pub variant_key: String,
    pub observed: u64,
    pub expected: f64,
    pub chi_sq_contribution: f64,
}

/// Aggregate SRM result for one context type's assignments.
#[derive(Debug, Serialize, ToSchema)]
pub struct SrmResultJson {
    pub per_variant: Vec<SrmPerVariantJson>,
    pub overall_chi_sq: f64,
    pub overall_chi_sq_p: f64,
    /// `"green"` | `"yellow"` | `"red"`
    pub health: String,
}

/// One context-type's results bundle: variants, SRM, and guardrails computed
/// independently per spec §3 "Per-context-type stats".
#[derive(Debug, Serialize, ToSchema)]
pub struct ContextTypeResultsJson {
    pub variants: Vec<VariantResultJson>,
    /// Absent if SRM was not computed (e.g. legacy iterations).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub srm: Option<SrmResultJson>,
    pub guardrails: Vec<VariantResultJson>,
}

/// Identifies what flag target the experiment is bound to — either a
/// specific rule or the flag's default-rule fall-through. `rule_id` is
/// `None` (omitted) when `kind == "default_rule"`.
#[derive(Debug, Serialize, ToSchema)]
pub struct BoundTargetJson {
    /// `"rule"` | `"default_rule"`
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rule_id: Option<String>,
    pub label: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ExperimentResultsJson {
    pub experiment_id: String,
    /// Per-context-type results bundles, keyed by `context_type` (e.g.
    /// `"user"`, `"account"`). Each value carries variants + SRM +
    /// guardrails for that context type. Empty `{}` when no rows have
    /// been computed yet.
    pub results_by_context_type: std::collections::HashMap<String, ContextTypeResultsJson>,
    /// What the experiment is bound to (rule vs default-rule).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bound_target: Option<BoundTargetJson>,
    /// CUPED pre-period window in days; `0` disables variance reduction.
    pub pre_period_days: u32,
    pub computed_at_ms: i64,
    pub is_stale: bool,
    pub next_run_at_ms: i64,
    pub computation_status: String,
}

fn experiment_status_str(status: i32) -> String {
    match ExperimentStatus::try_from(status).unwrap_or(ExperimentStatus::Unspecified) {
        ExperimentStatus::Draft => "draft",
        ExperimentStatus::Active => "running",
        ExperimentStatus::Paused => "paused",
        ExperimentStatus::Concluded => "stopped",
        ExperimentStatus::Unspecified => "draft",
    }
    .to_string()
}

fn ms_to_iso(ms: i64) -> String {
    use std::time::{Duration, UNIX_EPOCH};
    let secs = (ms / 1000) as u64;
    let nanos = ((ms % 1000) * 1_000_000) as u32;
    let t = UNIX_EPOCH + Duration::new(secs, nanos);
    let d = chrono::DateTime::<chrono::Utc>::from(t);
    d.to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

pub(crate) fn experiment_to_json(e: &Experiment) -> ExperimentJson {
    let variants = e.variant_keys.len() as u32;
    ExperimentJson {
        id: e.id.clone(),
        key: e.id.clone(),
        environment_id: e.environment_id.clone(),
        name: e.name.clone(),
        description: e.description.clone(),
        flag_key: e.flag_key.clone(),
        status: experiment_status_str(e.status),
        model: "frequentist".to_string(),
        variant_keys: e.variant_keys.clone(),
        variants,
        metric_ids: e.metric_ids.clone(),
        created_at: ms_to_iso(e.created_at_ms),
        updated_at: ms_to_iso(e.updated_at_ms),
        started_at: None,
        ended_at: None,
        unit_context_types: e.unit_context_types.clone(),
    }
}

fn variant_result_to_json(v: &VariantResult) -> VariantResultJson {
    VariantResultJson {
        variant_key: v.variant_key.clone(),
        participant_count: v.participant_count,
        p_value: if v.p_value_present {
            Some(v.p_value)
        } else {
            None
        },
        p_value_corrected: v.p_value_corrected,
        lift: v.lift,
        direction_violation: v.direction_violation,
        context_type: if v.context_type.is_empty() {
            "user".to_string()
        } else {
            v.context_type.clone()
        },
    }
}

fn srm_result_to_json(s: &ProtoSrmResult) -> SrmResultJson {
    SrmResultJson {
        per_variant: s
            .per_variant
            .iter()
            .map(|r| SrmPerVariantJson {
                variant_key: r.variant_key.clone(),
                observed: r.observed,
                expected: r.expected,
                chi_sq_contribution: r.chi_sq_contribution,
            })
            .collect(),
        overall_chi_sq: s.overall_chi_sq,
        overall_chi_sq_p: s.overall_chi_sq_p,
        health: s.health.clone(),
    }
}

fn context_type_results_to_json(c: &ContextTypeResults) -> ContextTypeResultsJson {
    ContextTypeResultsJson {
        variants: c.variants.iter().map(variant_result_to_json).collect(),
        srm: c.srm.as_ref().map(srm_result_to_json),
        guardrails: c.guardrails.iter().map(variant_result_to_json).collect(),
    }
}

fn bound_target_to_json(b: &BoundTarget) -> BoundTargetJson {
    BoundTargetJson {
        kind: b.kind.clone(),
        rule_id: if b.rule_id.is_empty() {
            None
        } else {
            Some(b.rule_id.clone())
        },
        label: b.label.clone(),
    }
}

fn iteration_to_json(i: &ExperimentIteration) -> IterationJson {
    IterationJson {
        id: i.id.clone(),
        experiment_id: i.experiment_id.clone(),
        iteration_number: i.iteration_number,
        started_at_ms: i.started_at_ms,
        ended_at_ms: i.ended_at_ms,
        traffic_allocation: i.traffic_allocation,
        unit_context_types: i.unit_context_types.clone(),
    }
}

fn status_from_str(s: &str) -> ExperimentStatus {
    match s.to_lowercase().as_str() {
        "draft" => ExperimentStatus::Draft,
        "active" | "running" => ExperimentStatus::Active,
        "paused" => ExperimentStatus::Paused,
        "concluded" | "stopped" | "completed" => ExperimentStatus::Concluded,
        _ => ExperimentStatus::Unspecified,
    }
}

// ─── Handlers ────────────────────────────────────────────────────────────────

/// `GET /v1/environments/{environment_id}/experiments`
#[utoipa::path(
    get,
    path = "/v1/environments/{environment_id}/experiments",
    tag = "experiments",
    params(
        ("environment_id" = String, Path, description = "Environment ID"),
    ),
    responses(
        (status = 200, description = "Paginated list of experiments"),
        (status = 401, description = "Unauthorized"),
        (status = 502, description = "Experimentation service unavailable"),
    ),
    security(("bearer_jwt" = []))
)]
pub async fn list_experiments(
    State(state): State<Arc<GatewayState>>,
    Path(environment_id): Path<String>,
    Query(query): Query<ListExperimentsQuery>,
) -> Result<impl IntoResponse, GatewayError> {
    let req = tonic::Request::new(ListExperimentsRequest {
        environment_id,
        page: query.pagination.effective_page(),
        per_page: query.pagination.effective_per_page(),
    });
    let mut client = state.experimentation_client.lock().await;
    let resp = client
        .list_experiments(req)
        .await
        .map_err(GatewayError::from)?;
    let inner = resp.into_inner();
    let experiments: Vec<ExperimentJson> =
        inner.experiments.iter().map(experiment_to_json).collect();
    Ok(Json(PaginatedResponse::new(
        experiments,
        inner.total,
        &query.pagination,
    )))
}

/// `POST /v1/environments/{environment_id}/experiments`
#[utoipa::path(
    post,
    path = "/v1/environments/{environment_id}/experiments",
    tag = "experiments",
    params(("environment_id" = String, Path, description = "Environment ID")),
    request_body = CreateExperimentBody,
    responses(
        (status = 201, description = "Experiment created", body = ExperimentJson),
        (status = 401, description = "Unauthorized"),
        (status = 502, description = "Experimentation service unavailable"),
    ),
    security(("bearer_jwt" = []))
)]
pub async fn create_experiment(
    State(state): State<Arc<GatewayState>>,
    Path(environment_id): Path<String>,
    Json(body): Json<CreateExperimentBody>,
) -> Result<impl IntoResponse, GatewayError> {
    // Pure translation: binding validation has moved to the experimentation-service (GL-08).
    let experiment = Experiment {
        environment_id,
        name: body.name.unwrap_or_default(),
        description: body.description.unwrap_or_default(),
        flag_key: body.flag_key.unwrap_or_default(),
        variant_keys: body.variant_keys.unwrap_or_default(),
        status: ExperimentStatus::Draft as i32,
        flag_rule_id: body.flag_rule_id.unwrap_or_default(),
        targets_default_rule: body.targets_default_rule,
        unit_context_types: body.unit_context_types,
        guardrail_metric_ids: body.guardrail_metric_ids,
        pre_period_days: body.pre_period_days,
        ..Default::default()
    };
    let req = tonic::Request::new(CreateExperimentRequest {
        experiment: Some(experiment),
    });
    let mut client = state.experimentation_client.lock().await;
    let resp = client
        .create_experiment(req)
        .await
        .map_err(GatewayError::from)?;
    Ok((
        StatusCode::CREATED,
        Json(experiment_to_json(&resp.into_inner())),
    ))
}

/// `GET /v1/environments/{environment_id}/experiments/{experiment_id}`
#[utoipa::path(
    get,
    path = "/v1/environments/{environment_id}/experiments/{experiment_id}",
    tag = "experiments",
    params(
        ("environment_id" = String, Path, description = "Environment ID"),
        ("experiment_id" = String, Path, description = "Experiment ID"),
    ),
    responses(
        (status = 200, description = "Experiment", body = ExperimentJson),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "Experiment not found"),
        (status = 502, description = "Experimentation service unavailable"),
    ),
    security(("bearer_jwt" = []))
)]
pub async fn get_experiment(
    State(state): State<Arc<GatewayState>>,
    Path((environment_id, experiment_id)): Path<(String, String)>,
) -> Result<impl IntoResponse, GatewayError> {
    let req = tonic::Request::new(GetExperimentRequest {
        environment_id,
        experiment_id,
    });
    let mut client = state.experimentation_client.lock().await;
    let resp = client
        .get_experiment(req)
        .await
        .map_err(GatewayError::from)?;
    Ok(Json(experiment_to_json(&resp.into_inner())))
}

/// `PATCH /v1/environments/{environment_id}/experiments/{experiment_id}`
#[utoipa::path(
    patch,
    path = "/v1/environments/{environment_id}/experiments/{experiment_id}",
    tag = "experiments",
    params(
        ("environment_id" = String, Path, description = "Environment ID"),
        ("experiment_id" = String, Path, description = "Experiment ID"),
    ),
    request_body = UpdateExperimentBody,
    responses(
        (status = 200, description = "Updated experiment", body = ExperimentJson),
        (status = 401, description = "Unauthorized"),
        (status = 502, description = "Experimentation service unavailable"),
    ),
    security(("bearer_jwt" = []))
)]
pub async fn update_experiment(
    State(state): State<Arc<GatewayState>>,
    Path((environment_id, experiment_id)): Path<(String, String)>,
    Json(body): Json<UpdateExperimentBody>,
) -> Result<impl IntoResponse, GatewayError> {
    // Pure translation: binding validation has moved to the experimentation-service (GL-08).
    // Binding fields are passed through so the service can validate and persist them.
    let experiment = Experiment {
        id: experiment_id.clone(),
        environment_id,
        name: body.name.unwrap_or_default(),
        description: body.description.unwrap_or_default(),
        variant_keys: body.variant_keys.unwrap_or_default(),
        version: body.version.unwrap_or(0),
        flag_key: body.flag_key.unwrap_or_default(),
        flag_rule_id: body.flag_rule_id.unwrap_or_default(),
        targets_default_rule: body.targets_default_rule,
        unit_context_types: body.unit_context_types,
        ..Default::default()
    };
    let req = tonic::Request::new(UpdateExperimentRequest {
        experiment: Some(experiment),
    });
    let mut client = state.experimentation_client.lock().await;
    let resp = client
        .update_experiment(req)
        .await
        .map_err(GatewayError::from)?;
    Ok(Json(experiment_to_json(&resp.into_inner())))
}

/// `DELETE /v1/environments/{environment_id}/experiments/{experiment_id}`
#[utoipa::path(
    delete,
    path = "/v1/environments/{environment_id}/experiments/{experiment_id}",
    tag = "experiments",
    params(
        ("environment_id" = String, Path, description = "Environment ID"),
        ("experiment_id" = String, Path, description = "Experiment ID"),
    ),
    responses(
        (status = 200, description = "Deleted experiment", body = ExperimentJson),
        (status = 401, description = "Unauthorized"),
        (status = 502, description = "Experimentation service unavailable"),
    ),
    security(("bearer_jwt" = []))
)]
pub async fn delete_experiment(
    State(state): State<Arc<GatewayState>>,
    Path((environment_id, experiment_id)): Path<(String, String)>,
) -> Result<impl IntoResponse, GatewayError> {
    let req = tonic::Request::new(DeleteExperimentRequest {
        environment_id,
        experiment_id,
    });
    let mut client = state.experimentation_client.lock().await;
    let resp = client
        .delete_experiment(req)
        .await
        .map_err(GatewayError::from)?;
    Ok(Json(experiment_to_json(&resp.into_inner())))
}

/// `POST /v1/environments/{environment_id}/experiments/{experiment_id}/transitions`
#[utoipa::path(
    post,
    path = "/v1/environments/{environment_id}/experiments/{experiment_id}/transitions",
    tag = "experiments",
    params(
        ("environment_id" = String, Path, description = "Environment ID"),
        ("experiment_id" = String, Path, description = "Experiment ID"),
    ),
    request_body = TransitionBody,
    responses(
        (status = 200, description = "Experiment after transition", body = ExperimentJson),
        (status = 401, description = "Unauthorized"),
        (status = 502, description = "Experimentation service unavailable"),
    ),
    security(("bearer_jwt" = []))
)]
pub async fn transition_experiment(
    State(state): State<Arc<GatewayState>>,
    Path((environment_id, experiment_id)): Path<(String, String)>,
    Json(body): Json<TransitionBody>,
) -> Result<impl IntoResponse, GatewayError> {
    let new_status = status_from_str(&body.new_status);
    let req = tonic::Request::new(TransitionExperimentRequest {
        environment_id,
        experiment_id,
        new_status: new_status as i32,
        reason: body.reason.unwrap_or_default(),
    });
    let mut client = state.experimentation_client.lock().await;
    let resp = client
        .transition_experiment(req)
        .await
        .map_err(GatewayError::from)?;
    Ok(Json(experiment_to_json(&resp.into_inner())))
}

/// Query parameters for `GET /experiments/{id}/iterations` — standard
/// `page` + `per_page` pagination (same shape as `GET /experiments` and
/// `GET /experiments/{id}/exposures`).
#[derive(Debug, Deserialize, ToSchema)]
pub struct ListIterationsQuery {
    #[serde(flatten)]
    pub pagination: PaginationParams,
}

/// `GET /v1/environments/{environment_id}/experiments/{experiment_id}/iterations`
///
/// Paginated list of past + active iterations for an experiment. Drives the
/// admin Iterations tab which renders one row per iteration with the snapshot
/// `unit_context_types` and start/end timestamps.
#[utoipa::path(
    get,
    path = "/v1/environments/{environment_id}/experiments/{experiment_id}/iterations",
    tag = "experiments",
    params(
        ("environment_id" = String, Path, description = "Environment ID"),
        ("experiment_id" = String, Path, description = "Experiment ID"),
        ("page" = Option<u32>, Query, description = "1-based page number"),
        ("per_page" = Option<u32>, Query, description = "Page size (default 50, max 200)"),
    ),
    responses(
        (status = 200, description = "Paginated experiment iterations"),
        (status = 401, description = "Unauthorized"),
        (status = 502, description = "Experimentation service unavailable"),
    ),
    security(("bearer_jwt" = []))
)]
pub async fn list_iterations(
    State(state): State<Arc<GatewayState>>,
    Path((environment_id, experiment_id)): Path<(String, String)>,
    Query(query): Query<ListIterationsQuery>,
) -> Result<impl IntoResponse, GatewayError> {
    let page = query.pagination.effective_page();
    let per_page = query.pagination.effective_per_page();
    let offset = u64::from(page.saturating_sub(1)) * u64::from(per_page);
    let limit = u64::from(per_page);

    let req = tonic::Request::new(ListIterationsRequest {
        environment_id,
        experiment_id,
        offset,
        limit,
    });
    let mut client = state.experimentation_client.lock().await;
    let resp = client
        .list_iterations(req)
        .await
        .map_err(GatewayError::from)?;
    let inner = resp.into_inner();
    let iterations: Vec<IterationJson> = inner.iterations.iter().map(iteration_to_json).collect();
    Ok(Json(PaginatedResponse::new(
        iterations,
        inner.total,
        &query.pagination,
    )))
}

/// `GET /v1/environments/{environment_id}/experiments/{experiment_id}/results`
#[utoipa::path(
    get,
    path = "/v1/environments/{environment_id}/experiments/{experiment_id}/results",
    tag = "experiments",
    params(
        ("environment_id" = String, Path, description = "Environment ID"),
        ("experiment_id" = String, Path, description = "Experiment ID"),
    ),
    responses(
        (status = 200, description = "Experiment results", body = ExperimentResultsJson),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "Results not found"),
        (status = 502, description = "Experimentation service unavailable"),
    ),
    security(("bearer_jwt" = []))
)]
pub async fn get_results(
    State(state): State<Arc<GatewayState>>,
    Path((environment_id, experiment_id)): Path<(String, String)>,
) -> Result<impl IntoResponse, GatewayError> {
    let req = tonic::Request::new(GetResultsRequest {
        environment_id,
        experiment_id,
    });
    let mut client = state.experimentation_client.lock().await;
    let resp = client.get_results(req).await.map_err(GatewayError::from)?;
    let inner = resp.into_inner();

    // The experimentation-service always populates `results_by_context_type`
    // (including defaulting empty `context_type` rows to "user" server-side),
    // so the gateway just passes through whatever the service returns (GL-09).
    let results_by_context_type: std::collections::HashMap<String, ContextTypeResultsJson> = inner
        .results_by_context_type
        .iter()
        .map(|bundle| {
            (
                bundle.context_type.clone(),
                context_type_results_to_json(bundle),
            )
        })
        .collect();

    let results = ExperimentResultsJson {
        experiment_id: inner.experiment_id.clone(),
        results_by_context_type,
        bound_target: inner.bound_target.as_ref().map(bound_target_to_json),
        pre_period_days: inner.pre_period_days,
        computed_at_ms: inner.computed_at_ms,
        is_stale: inner.is_stale,
        next_run_at_ms: inner.next_run_at_ms,
        computation_status: inner.computation_status.clone(),
    };
    Ok(Json(results))
}

// ─── GET /exposures (Phase 7 Task 2) ─────────────────────────────────────────

/// Query parameters for `GET /exposures`. `context_type` is required —
/// omitting it returns HTTP 400 `missing_context_type`.
#[derive(Debug, Deserialize, ToSchema)]
pub struct ListExposuresQuery {
    /// REQUIRED. One of the experiment's `unit_context_types`.
    pub context_type: Option<String>,
    #[serde(flatten)]
    pub pagination: PaginationParams,
}

/// One row from `experiment_assignments` for the gateway JSON response.
#[derive(Debug, Serialize, ToSchema)]
pub struct ExposureRowJson {
    pub context_type: String,
    pub context_key: String,
    pub variant_key: String,
    /// RFC 3339 UTC timestamp of first exposure.
    pub assigned_at: String,
    /// `None` (omitted) for default-rule experiments.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub matched_rule_id: Option<String>,
}

/// `GET /v1/environments/{environment_id}/experiments/{experiment_id}/exposures`
///
/// Paginated list of first-exposure assignments for an experiment scoped to a
/// single `context_type`. The `context_type` query parameter is required;
/// missing it returns HTTP 400 with `{ "error": "missing_context_type" }`.
#[utoipa::path(
    get,
    path = "/v1/environments/{environment_id}/experiments/{experiment_id}/exposures",
    tag = "experiments",
    params(
        ("environment_id" = String, Path, description = "Environment ID"),
        ("experiment_id" = String, Path, description = "Experiment ID"),
        ("context_type" = String, Query, description = "REQUIRED. Attribution unit (e.g. 'user', 'account')."),
        ("page" = Option<u32>, Query, description = "1-based page number"),
        ("per_page" = Option<u32>, Query, description = "Page size (default 50, max 200)"),
    ),
    responses(
        (status = 200, description = "Paginated exposures"),
        (status = 400, description = "Missing required `context_type` query parameter"),
        (status = 401, description = "Unauthorized"),
        (status = 502, description = "Experimentation service unavailable"),
    ),
    security(("bearer_jwt" = []))
)]
pub async fn list_exposures(
    State(state): State<Arc<GatewayState>>,
    Path((_environment_id, experiment_id)): Path<(String, String)>,
    Query(query): Query<ListExposuresQuery>,
) -> Result<impl IntoResponse, GatewayError> {
    let context_type = query.context_type.unwrap_or_default();
    if context_type.is_empty() {
        // The spec mandates a structured 400 body the admin UI can branch on.
        return Err(GatewayError::MissingContextType);
    }

    let page = query.pagination.effective_page();
    let per_page = query.pagination.effective_per_page();
    let offset = u64::from(page.saturating_sub(1)) * u64::from(per_page);
    let limit = u64::from(per_page);

    let req = tonic::Request::new(ListExposuresRequest {
        experiment_id,
        context_type,
        offset,
        limit,
    });

    let mut client = state.experimentation_client.lock().await;
    let resp = client
        .list_exposures(req)
        .await
        .map_err(GatewayError::from)?;
    let inner = resp.into_inner();
    let exposures: Vec<ExposureRowJson> = inner
        .exposures
        .into_iter()
        .map(|r| ExposureRowJson {
            context_type: r.context_type,
            context_key: r.context_key,
            variant_key: r.variant_key,
            assigned_at: r.assigned_at,
            matched_rule_id: if r.matched_rule_id.is_empty() {
                None
            } else {
                Some(r.matched_rule_id)
            },
        })
        .collect();
    Ok(Json(PaginatedResponse::new(
        exposures,
        inner.total,
        &query.pagination,
    )))
}

// ─── GET /interactions (Phase 7) ─────────────────────────────────────────────

/// One N-way interaction estimate among a tuple of experiments that share
/// assignment population. Surfaces possible cross-experiment effects.
/// `interaction_order` is the tuple size (2 = pairwise, 3 = three-way).
#[derive(Debug, Serialize, ToSchema)]
pub struct ExperimentInteractionJson {
    /// Experiment ids in this interaction tuple (length == `interaction_order`).
    pub experiment_ids: Vec<String>,
    /// Human-readable names aligned 1:1 with `experiment_ids` (same order, same
    /// length); ids without a resolvable experiment fall back to the id string.
    pub experiment_names: Vec<String>,
    /// Number of experiments in the tuple: 2 for pairwise, 3 for three-way.
    pub interaction_order: u32,
    /// Term key: `main:<uuid>` / `2way:<a>x<b>` / `3way:<a>x<b>x<c>`.
    pub term: String,
    pub context_type: String,
    pub metric_key: String,
    /// Number of contexts assigned in every experiment of the tuple.
    pub shared_count: u64,
    /// Estimated interaction effect size.
    pub interaction_estimate: f64,
    pub p_value: f64,
    /// Degrees of freedom of the interaction test.
    pub df: u32,
    /// `true` when the interaction is statistically significant. Always `false`
    /// when `insufficient_data` is `true`.
    pub significant: bool,
    /// `true` when the tuple lacked enough shared exposures to run a meaningful
    /// interaction test; treat `significant`/`interaction_estimate` as
    /// inconclusive in that case.
    pub insufficient_data: bool,
    /// Bayesian posterior probability that an interaction effect exists.
    pub bayes_prob: f64,
    /// Bayesian posterior expected interaction effect size.
    pub bayes_expected: f64,
    /// Lower bound of the Bayesian credible interval.
    pub bayes_ci_low: f64,
    /// Upper bound of the Bayesian credible interval.
    pub bayes_ci_high: f64,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ExperimentInteractionsJson {
    pub interactions: Vec<ExperimentInteractionJson>,
}

fn interaction_to_json(i: &ExperimentInteraction) -> ExperimentInteractionJson {
    ExperimentInteractionJson {
        experiment_ids: i.experiment_ids.clone(),
        experiment_names: i.experiment_names.clone(),
        interaction_order: i.interaction_order,
        term: i.term.clone(),
        context_type: i.context_type.clone(),
        metric_key: i.metric_key.clone(),
        shared_count: i.shared_count,
        interaction_estimate: i.interaction_estimate,
        p_value: i.p_value,
        df: i.df,
        significant: i.significant,
        insufficient_data: i.insufficient_data,
        bayes_prob: i.bayes_prob,
        bayes_expected: i.bayes_expected,
        bayes_ci_low: i.bayes_ci_low,
        bayes_ci_high: i.bayes_ci_high,
    }
}

/// `GET /v1/environments/{environment_id}/experiments/{experiment_id}/interactions`
///
/// N-way interaction estimates (orders 2 and 3) among tuples of experiments
/// that share assignment population. Drives the admin Interactions tab.
#[utoipa::path(
    get,
    path = "/v1/environments/{environment_id}/experiments/{experiment_id}/interactions",
    tag = "experiments",
    params(
        ("environment_id" = String, Path, description = "Environment ID"),
        ("experiment_id" = String, Path, description = "Experiment ID"),
    ),
    responses(
        (status = 200, description = "Experiment interactions", body = ExperimentInteractionsJson),
        (status = 401, description = "Unauthorized"),
        (status = 502, description = "Experimentation service unavailable"),
    ),
    security(("bearer_jwt" = []))
)]
pub async fn get_interactions(
    State(state): State<Arc<GatewayState>>,
    Path((environment_id, experiment_id)): Path<(String, String)>,
) -> Result<impl IntoResponse, GatewayError> {
    let req = tonic::Request::new(GetExperimentInteractionsRequest {
        env_id: environment_id,
        experiment_id,
    });
    let mut client = state.experimentation_client.lock().await;
    let resp = client
        .get_experiment_interactions(req)
        .await
        .map_err(GatewayError::from)?;
    let inner = resp.into_inner();
    let interactions: Vec<ExperimentInteractionJson> =
        inner.interactions.iter().map(interaction_to_json).collect();
    Ok(Json(ExperimentInteractionsJson { interactions }))
}

#[cfg(test)]
pub fn test_router(state: Arc<GatewayState>) -> axum::Router {
    #[allow(unused_imports)]
    use axum::routing::{delete, get, patch, post};
    axum::Router::new()
        .route(
            "/v1/environments/{environment_id}/experiments",
            get(list_experiments).post(create_experiment),
        )
        .route(
            "/v1/environments/{environment_id}/experiments/{experiment_id}",
            get(get_experiment)
                .patch(update_experiment)
                .delete(delete_experiment),
        )
        .route(
            "/v1/environments/{environment_id}/experiments/{experiment_id}/results",
            get(get_results),
        )
        .route(
            "/v1/environments/{environment_id}/experiments/{experiment_id}/transitions",
            post(transition_experiment),
        )
        .route(
            "/v1/environments/{environment_id}/experiments/{experiment_id}/iterations",
            get(list_iterations),
        )
        .route(
            "/v1/environments/{environment_id}/experiments/{experiment_id}/exposures",
            get(list_exposures),
        )
        .route(
            "/v1/environments/{environment_id}/experiments/{experiment_id}/interactions",
            get(get_interactions),
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
    fn experiment_status_str_draft() {
        assert_eq!(
            experiment_status_str(ExperimentStatus::Draft as i32),
            "draft"
        );
    }

    #[test]
    fn experiment_status_str_active() {
        assert_eq!(
            experiment_status_str(ExperimentStatus::Active as i32),
            "running"
        );
    }

    #[test]
    fn experiment_status_str_unspecified() {
        assert_eq!(experiment_status_str(0), "draft");
    }

    #[test]
    fn variant_result_to_json_maps_fields() {
        let vr = VariantResult {
            variant_key: "control".to_string(),
            participant_count: 42,
            ..Default::default()
        };
        let j = variant_result_to_json(&vr);
        assert_eq!(j.variant_key, "control");
        assert_eq!(j.participant_count, 42);
        assert!(j.p_value.is_none());
        assert!(j.p_value_corrected.is_none());
        assert!((j.lift - 0.0).abs() < f64::EPSILON);
        assert!(!j.direction_violation);
        // Empty context_type defaults to "user".
        assert_eq!(j.context_type, "user");
    }

    #[test]
    fn variant_result_to_json_propagates_p_values_and_lift() {
        let vr = VariantResult {
            variant_key: "treatment".to_string(),
            participant_count: 1024,
            p_value: 0.03,
            p_value_present: true,
            p_value_corrected: Some(0.09),
            context_type: "account".to_string(),
            direction_violation: true,
            lift: -0.12,
            ..Default::default()
        };
        let j = variant_result_to_json(&vr);
        assert_eq!(j.p_value, Some(0.03));
        assert_eq!(j.p_value_corrected, Some(0.09));
        assert_eq!(j.context_type, "account");
        assert!(j.direction_violation);
        assert!((j.lift - (-0.12)).abs() < 1e-9);
    }

    #[test]
    fn bound_target_to_json_omits_empty_rule_id() {
        let bt = BoundTarget {
            kind: "default_rule".to_string(),
            rule_id: String::new(),
            label: "Default rule (fallthrough)".to_string(),
        };
        let j = bound_target_to_json(&bt);
        assert_eq!(j.kind, "default_rule");
        assert!(j.rule_id.is_none());
        assert_eq!(j.label, "Default rule (fallthrough)");
    }

    #[test]
    fn bound_target_to_json_round_trips_rule_id() {
        let rid = "11111111-2222-3333-4444-555555555555".to_string();
        let bt = BoundTarget {
            kind: "rule".to_string(),
            rule_id: rid.clone(),
            label: rid.clone(),
        };
        let j = bound_target_to_json(&bt);
        assert_eq!(j.rule_id.as_deref(), Some(rid.as_str()));
    }

    #[test]
    fn context_type_results_to_json_renders_variants_srm_and_guardrails() {
        let bundle = ContextTypeResults {
            context_type: "user".to_string(),
            variants: vec![VariantResult {
                variant_key: "control".to_string(),
                participant_count: 100,
                context_type: "user".to_string(),
                ..Default::default()
            }],
            srm: Some(ProtoSrmResult {
                per_variant: vec![],
                overall_chi_sq: 1.5,
                overall_chi_sq_p: 0.22,
                health: "green".to_string(),
            }),
            guardrails: vec![VariantResult {
                variant_key: "treatment".to_string(),
                participant_count: 99,
                direction_violation: true,
                lift: -0.5,
                context_type: "user".to_string(),
                ..Default::default()
            }],
        };
        let j = context_type_results_to_json(&bundle);
        assert_eq!(j.variants.len(), 1);
        assert_eq!(j.guardrails.len(), 1);
        let srm = j.srm.expect("srm present");
        assert_eq!(srm.health, "green");
        assert!(j.guardrails[0].direction_violation);
    }

    #[test]
    fn experiment_to_json_maps_fields() {
        let e = Experiment {
            id: "exp-1".to_string(),
            name: "test".to_string(),
            flag_key: "flag-1".to_string(),
            status: ExperimentStatus::Active as i32,
            variant_keys: vec!["on".to_string(), "off".to_string()],
            ..Default::default()
        };
        let j = experiment_to_json(&e);
        assert_eq!(j.id, "exp-1");
        assert_eq!(j.status, "running");
        assert_eq!(j.variant_keys.len(), 2);
    }

    #[tokio::test]
    async fn list_experiments_returns_200_or_502() {
        let state = make_stub_state();
        let app = test_router(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/v1/environments/env-1/experiments")
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
    async fn create_experiment_returns_201_or_502_or_422() {
        let state = make_stub_state();
        let app = test_router(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/environments/env-1/experiments")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"name":"exp","flag_key":"f1","variant_keys":["on","off"]}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        // 422 is the post-Task-3.3 outcome — the body omits `unit_context_types`,
        // which the new binding validator rejects with EMPTY_UNIT_CONTEXT_TYPES
        // before it ever reaches the gRPC client.
        assert!(
            resp.status() == StatusCode::CREATED
                || resp.status() == StatusCode::BAD_GATEWAY
                || resp.status() == StatusCode::UNPROCESSABLE_ENTITY,
            "status: {}",
            resp.status()
        );
    }

    #[tokio::test]
    async fn delete_experiment_returns_200_or_502() {
        let state = make_stub_state();
        let app = test_router(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri("/v1/environments/env-1/experiments/exp-1")
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
    async fn update_experiment_returns_200_or_502() {
        let state = make_stub_state();
        let app = test_router(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .method("PATCH")
                    .uri("/v1/environments/env-1/experiments/exp-1")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"name":"updated"}"#))
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
    async fn transition_experiment_returns_200_or_502() {
        let state = make_stub_state();
        let app = test_router(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/environments/env-1/experiments/exp-1/transitions")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"new_status":"active"}"#))
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
    async fn list_iterations_returns_200_or_502() {
        let state = make_stub_state();
        let app = test_router(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/v1/environments/env-1/experiments/exp-1/iterations")
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
    async fn list_iterations_accepts_pagination_query_params() {
        // Confirms the route binds the `page` + `per_page` query params and
        // hands them to the experimentation client without rejecting the
        // request as a 400. The stub channel is lazy, so we accept the same
        // 200/502 outcomes as the unparameterised test.
        let state = make_stub_state();
        let app = test_router(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/v1/environments/env-1/experiments/exp-1/iterations?page=2&per_page=25")
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

    #[test]
    fn status_from_str_maps_known_values() {
        assert_eq!(status_from_str("active"), ExperimentStatus::Active);
        assert_eq!(status_from_str("paused"), ExperimentStatus::Paused);
        assert_eq!(status_from_str("concluded"), ExperimentStatus::Concluded);
        assert_eq!(status_from_str("unknown"), ExperimentStatus::Unspecified);
    }

    #[tokio::test]
    async fn get_experiment_returns_200_or_502() {
        let state = make_stub_state();
        let app = test_router(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/v1/environments/env-1/experiments/exp-1")
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

    #[test]
    fn iteration_to_json_maps_fields() {
        let i = ExperimentIteration {
            id: "iter-1".to_string(),
            experiment_id: "exp-1".to_string(),
            iteration_number: 2,
            started_at_ms: 1000,
            ended_at_ms: 2000,
            traffic_allocation: 0.5,
            unit_context_types: vec!["user".to_string(), "account".to_string()],
            ..Default::default()
        };
        let j = iteration_to_json(&i);
        assert_eq!(j.id, "iter-1");
        assert_eq!(j.experiment_id, "exp-1");
        assert_eq!(j.iteration_number, 2);
        assert_eq!(j.started_at_ms, 1000);
        assert_eq!(j.ended_at_ms, 2000);
        assert_eq!(j.unit_context_types, vec!["user", "account"]);
    }

    #[tokio::test]
    async fn list_exposures_missing_context_type_returns_400() {
        let state = make_stub_state();
        let app = test_router(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/v1/environments/env-1/experiments/exp-1/exposures")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::BAD_REQUEST,
            "expected 400 when context_type is missing; got {}",
            resp.status()
        );
        // Body should be the structured error.
        let body_bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
        assert_eq!(body["error"], "missing_context_type");
    }

    #[tokio::test]
    async fn list_exposures_empty_context_type_returns_400() {
        let state = make_stub_state();
        let app = test_router(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/v1/environments/env-1/experiments/exp-1/exposures?context_type=")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn list_exposures_with_context_type_proxies_to_grpc() {
        // With the stub state the grpc connection fails fast (lazy channel),
        // so the handler returns 502. The key behavior under test is that we
        // got past the missing-context-type guard.
        let state = make_stub_state();
        let app = test_router(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/v1/environments/env-1/experiments/exp-1/exposures?context_type=user")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(
            resp.status() == StatusCode::OK
                || resp.status() == StatusCode::BAD_GATEWAY
                || resp.status() == StatusCode::INTERNAL_SERVER_ERROR,
            "got {}",
            resp.status()
        );
    }

    #[tokio::test]
    async fn get_results_returns_200_404_or_502() {
        let state = make_stub_state();
        let app = test_router(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/v1/environments/env-1/experiments/exp-1/results")
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

    #[test]
    fn interaction_to_json_maps_3way_and_bayes_fields() {
        let proto = ExperimentInteraction {
            experiment_ids: vec!["a".into(), "b".into(), "c".into()],
            experiment_names: vec!["Exp A".into(), "Exp B".into(), "Exp C".into()],
            interaction_order: 3,
            term: "3way:axbxc".into(),
            context_type: "user".into(),
            metric_key: "checkout".into(),
            shared_count: 400,
            interaction_estimate: 0.42,
            p_value: 0.001,
            df: 4,
            significant: true,
            insufficient_data: false,
            bayes_prob: 0.97,
            bayes_expected: 0.40,
            bayes_ci_low: 0.15,
            bayes_ci_high: 0.66,
        };
        let j = interaction_to_json(&proto);
        assert_eq!(j.interaction_order, 3);
        assert_eq!(j.term, "3way:axbxc");
        assert_eq!(j.experiment_ids, vec!["a", "b", "c"]);
        assert_eq!(j.experiment_names, vec!["Exp A", "Exp B", "Exp C"]);
        assert_eq!(j.shared_count, 400);
        assert_eq!(j.df, 4);
        assert!(j.significant);
        assert!(!j.insufficient_data);
        // Bayesian fields surface unchanged.
        assert!((j.bayes_prob - 0.97).abs() < 1e-9);
        assert!((j.bayes_expected - 0.40).abs() < 1e-9);
        assert!((j.bayes_ci_low - 0.15).abs() < 1e-9);
        assert!((j.bayes_ci_high - 0.66).abs() < 1e-9);

        // Confirm the DTO serializes the new N-way shape.
        let v = serde_json::to_value(&j).unwrap();
        assert_eq!(v["interaction_order"], 3);
        assert_eq!(v["term"], "3way:axbxc");
        assert!(v["experiment_ids"].is_array());
        assert!(v.get("bayes_ci_high").is_some());
        // Old pairwise fields are gone.
        assert!(v.get("experiment_id_a").is_none());
        assert!(v.get("other_experiment_name").is_none());
    }
}
