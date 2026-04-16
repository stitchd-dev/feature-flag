//! Feature flag management handlers.

use crate::AppState;
use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use stitchd_core::{
    context::EvaluationContext,
    flag::{FlagHashingConfig, FlagRecord, FlagRule, FlagValueType, Variant},
    id::{EnvironmentId, FlagId, FlagKey, ProjectId, VariantId},
};
use crate::api::segments::handlers::ApiError;

/// Request to create a new feature flag.
#[derive(Deserialize)]
pub struct CreateFlagRequest {
    /// URL-safe key.
    pub key: String,
    /// Value type (bool, int, etc).
    pub value_type: FlagValueType,
    /// Initial enabled status.
    pub enabled: bool,
}

/// Request to update an existing flag's metadata.
#[derive(Deserialize)]
pub struct UpdateFlagRequest {
    /// New enabled status.
    pub enabled: Option<bool>,
    /// New default variant ID.
    pub default_variant_id: Option<VariantId>,
    /// Optimistic concurrency version.
    pub version: i64,
}

/// Full response for a single flag, including its rules and variants.
#[derive(Serialize)]
pub struct FlagResponse {
    /// Core record.
    pub record: FlagRecord,
    /// Associated variants.
    pub variants: Vec<Variant>,
    /// Hashing configuration.
    pub hashing_config: Vec<FlagHashingConfig>,
    /// Evaluation rules.
    pub rules: Vec<FlagRule>,
}

/// `GET /v1/projects/{project_id}/flags` — List flags in a project.
///
/// # Errors
/// Returns [`ApiError::Database`] if the repository fails.
pub async fn list_flags(
    Path(project_id): Path<ProjectId>,
    State(state): State<AppState>,
) -> Result<Json<Vec<FlagRecord>>, ApiError> {
    let flags = state.flag_repo.list_by_project(project_id).await?;
    Ok(Json(flags))
}

/// `POST /v1/projects/{project_id}/flags` — Create a new flag.
///
/// # Errors
/// Returns [`ApiError::BadRequest`] if validation fails or [`ApiError::Conflict`] if the key exists.
pub async fn create_flag(
    Path(project_id): Path<ProjectId>,
    State(state): State<AppState>,
    Json(req): Json<CreateFlagRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let flag_key = FlagKey::new(req.key).map_err(|e| ApiError::BadRequest(e.to_string()))?;
    
    let flag = FlagRecord {
        id: FlagId::new(),
        project_id,
        key: flag_key,
        value_type: req.value_type,
        enabled: req.enabled,
        default_variant_id: None,
        created_at: Utc::now(),
        updated_at: Utc::now(),
        deleted_at: None,
        version: 1,
    };

    state.flag_repo.create(&flag).await?;
    Ok((StatusCode::CREATED, Json(flag)))
}

/// `GET /v1/projects/{project_id}/flags/{flag_id}` — Get a flag with its details.
///
/// # Errors
/// Returns [`ApiError::NotFound`] if the flag is missing.
pub async fn get_flag(
    Path((_project_id, flag_id)): Path<(ProjectId, FlagId)>,
    State(state): State<AppState>,
) -> Result<Json<FlagResponse>, ApiError> {
    let record = state.flag_repo.find_by_id(flag_id).await?;
    let variants = state.variant_repo.find_by_flag(flag_id).await?;
    let hashing_config = state.flag_repo.find_hashing_config(flag_id).await?;
    let rules = state.flag_repo.find_rules(flag_id).await?;

    Ok(Json(FlagResponse {
        record,
        variants,
        hashing_config,
        rules,
    }))
}

/// `PUT /v1/projects/{project_id}/flags/{flag_id}` — Update flag metadata.
///
/// # Errors
/// Returns [`ApiError::Conflict`] if the version mismatch or [`ApiError::NotFound`] if missing.
pub async fn update_flag(
    Path((_project_id, flag_id)): Path<(ProjectId, FlagId)>,
    State(state): State<AppState>,
    Json(req): Json<UpdateFlagRequest>,
) -> Result<Json<FlagRecord>, ApiError> {
    let mut flag = state.flag_repo.find_by_id(flag_id).await?;

    if flag.version != req.version {
        return Err(ApiError::Conflict(format!(
            "version conflict: expected {}, actual {}",
            req.version, flag.version
        )));
    }

    if let Some(enabled) = req.enabled {
        flag.enabled = enabled;
    }
    
    if let Some(variant_id) = req.default_variant_id {
        // Verify variant exists
        let variants = state.variant_repo.find_by_flag(flag_id).await?;
        if !variants.iter().any(|v| v.id == variant_id) {
            return Err(ApiError::BadRequest(format!("variant {} not found for flag", variant_id)));
        }
        flag.default_variant_id = Some(variant_id);
    }

    let updated = state.flag_repo.update(&flag).await?;
    Ok(Json(updated))
}

/// `DELETE /v1/projects/{project_id}/flags/{flag_id}` — Soft-delete a flag.
///
/// # Errors
/// Returns [`ApiError::NotFound`] if the flag does not exist.
pub async fn delete_flag(
    Path((_project_id, flag_id)): Path<(ProjectId, FlagId)>,
    State(state): State<AppState>,
) -> Result<StatusCode, ApiError> {
    state.flag_repo.soft_delete(flag_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

// ---------------------------------------------------------------------------
// Variant Handlers
// ---------------------------------------------------------------------------

/// Request to create a new flag variant.
#[derive(Deserialize)]
pub struct CreateVariantRequest {
    /// Unique key within the flag.
    pub key: String,
    /// Typed value.
    pub value: serde_json::Value,
}

/// `POST /v1/projects/{project_id}/flags/{flag_id}/variants` — Add a variant to a flag.
///
/// # Errors
/// Returns [`ApiError::BadRequest`] if type mismatch.
pub async fn create_variant(
    Path((_project_id, flag_id)): Path<(ProjectId, FlagId)>,
    State(state): State<AppState>,
    Json(req): Json<CreateVariantRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let flag = state.flag_repo.find_by_id(flag_id).await?;
    
    let variant_value = serde_json::from_value(req.value).map_err(|e| ApiError::BadRequest(e.to_string()))?;
    
    // Core check for type compatibility
    use stitchd_core::variants::VariantValue;
    let vv: VariantValue = variant_value;
    if !vv.matches_type(&flag.value_type) {
         return Err(ApiError::BadRequest(format!("Variant value does not match flag type {:?}", flag.value_type)));
    }

    let variant = Variant {
        id: VariantId::new(),
        key: req.key,
        value: vv,
    };

    state.variant_repo.create(flag_id, &variant).await?;
    Ok((StatusCode::CREATED, Json(variant)))
}

// ---------------------------------------------------------------------------
// Hashing Config Handlers
// ---------------------------------------------------------------------------

/// `PUT /v1/projects/{project_id}/flags/{flag_id}/hashing` — Set hashing config.
///
/// # Errors
/// Returns [`ApiError::Database`] if upsert fails.
pub async fn update_hashing_config(
    Path((_project_id, flag_id)): Path<(ProjectId, FlagId)>,
    State(state): State<AppState>,
    Json(req): Json<Vec<FlagHashingConfig>>,
) -> Result<StatusCode, ApiError> {
    state.flag_repo.upsert_hashing_config(flag_id, &req).await?;
    Ok(StatusCode::NO_CONTENT)
}

// ---------------------------------------------------------------------------
// Rule Handlers
// ---------------------------------------------------------------------------

/// `PUT /v1/projects/{project_id}/flags/{flag_id}/rules` — Set evaluation rules.
///
/// # Errors
/// Returns [`ApiError::Database`] if upsert fails.
pub async fn update_rules(
    Path((_project_id, flag_id)): Path<(ProjectId, FlagId)>,
    State(state): State<AppState>,
    Json(req): Json<Vec<FlagRule>>,
) -> Result<StatusCode, ApiError> {
    state.flag_repo.upsert_rules(flag_id, &req).await?;
    Ok(StatusCode::NO_CONTENT)
}

// ---------------------------------------------------------------------------
// Evaluation Handlers
// ---------------------------------------------------------------------------

/// Request for context-based flag evaluation.
#[derive(Deserialize)]
pub struct EvaluationRequest {
    /// Context parameters for evaluation.
    pub context: EvaluationContext,
}

/// The result of a single flag evaluation.
#[derive(Serialize)]
pub struct EvaluationResult {
    /// Flag key.
    pub flag_key: String,
    /// Matched variant key.
    pub variant_key: String,
    /// Variant value.
    pub variant_value: serde_json::Value,
}

/// Response containing all evaluated flags for the requested context.
#[derive(Serialize)]
pub struct BatchEvaluationResponse {
    /// Map of flag keys to results.
    pub results: Vec<EvaluationResult>,
}

/// `POST /v1/environments/{env_id}/evaluate` — Evaluate all flags for a context.
///
/// # Errors
/// Returns [`ApiError::Database`] if evaluation logic fails.
pub async fn evaluate_all_flags(
    Path(env_id): Path<EnvironmentId>,
    State(state): State<AppState>,
    Json(req): Json<EvaluationRequest>,
) -> Result<Json<BatchEvaluationResponse>, ApiError> {
    // 1. Fetch all segments for the environment
    let segment_records = state.segment_repo.list_by_environment(env_id).await?;
    let mut segment_definitions = Vec::with_capacity(segment_records.len());
    for sr in segment_records {
        let def = match sr.segment_type {
            stitchd_core::segment::SegmentType::Rule => {
                stitchd_core::segment::SegmentDefinition::RuleBased(state.segment_repo.find_with_rules(sr.id).await?)
            }
            stitchd_core::segment::SegmentType::List => {
                stitchd_core::segment::SegmentDefinition::ListBased(state.segment_repo.find_with_list(sr.id).await?)
            }
        };
        segment_definitions.push(def);
    }

    // 2. Resolve segments
    use stitchd_core::segment::SegmentEvaluator;
    let match_results = SegmentEvaluator::evaluate_all(&req.context.contexts, &segment_definitions)
        .map_err(|e| ApiError::Database(e.to_string()))?;

    let resolved_segments: std::collections::HashSet<_> = match_results
        .into_iter()
        .filter(|(_, res)| res.matched)
        .map(|(id, _)| id)
        .collect();

    // 3. Fetch all flags for the environment
    let flag_records = state.flag_repo.list_by_environment(env_id).await?;

    // 4. Evaluate each flag
    use stitchd_core::evaluation::FlagEvaluator;
    let mut evaluation_results = Vec::with_capacity(flag_records.len());

    for record in flag_records {
        // Fetch full flag aggregate
        let hashing_config = state.flag_repo.find_hashing_config(record.id).await?;
        let rules = state.flag_repo.find_rules(record.id).await?;
        let variants = state.variant_repo.find_by_flag(record.id).await?;

        let flag = stitchd_core::flag::Flag {
            record: record.clone(),
            hashing_config,
            rules,
            variants,
        };

        // Evaluate
        let variant = FlagEvaluator::evaluate(&flag, &req.context, &resolved_segments)
            .map_err(|e| ApiError::Database(e.to_string()))?;

        let variant_value = serde_json::to_value(&variant.value).map_err(|e| ApiError::Database(e.to_string()))?;

        evaluation_results.push(EvaluationResult {
            flag_key: record.key.to_string(),
            variant_key: variant.key.clone(),
            variant_value,
        });
    }

    Ok(Json(BatchEvaluationResponse {
        results: evaluation_results,
    }))
}
