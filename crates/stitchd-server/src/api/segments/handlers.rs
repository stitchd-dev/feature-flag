use crate::AppState;
use crate::api::sdk_auth::SdkAuth;
use crate::api::segments::types::{
    BatchContextMembership, BatchListCheckRequest, BatchListCheckResponse, CreateSegmentRequest,
    ListCheckRequest, ListCheckResponse, SegmentResponse, UpdateSegmentRequest, ValidationError,
    validate_rules,
};
use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use chrono::Utc;
use stitchd_core::{
    id::{EnvironmentId, SegmentId},
    segment::{Segment, SegmentDefinition, SegmentType},
};
use stitchd_db::RepositoryError;

/// API error type for mapping internal errors to HTTP responses.
pub enum ApiError {
    /// Resource not found.
    NotFound(String),
    /// Optimistic concurrency or unique constraint conflict.
    Conflict(String),
    /// Invalid input or business rule violation.
    BadRequest(String),
    /// Internal database error.
    Database(String),
}

impl IntoResponse for ApiError {
    fn into_response(self) -> axum::response::Response {
        let (status, msg) = match self {
            Self::NotFound(msg) => (StatusCode::NOT_FOUND, msg),
            Self::Conflict(msg) => (StatusCode::CONFLICT, msg),
            Self::BadRequest(msg) => (StatusCode::UNPROCESSABLE_ENTITY, msg),
            Self::Database(msg) => (StatusCode::INTERNAL_SERVER_ERROR, msg),
        };
        (status, Json(serde_json::json!({ "error": msg }))).into_response()
    }
}

impl From<RepositoryError> for ApiError {
    fn from(e: RepositoryError) -> Self {
        match e {
            RepositoryError::NotFound { id } => Self::NotFound(format!("not found: {id}")),
            RepositoryError::VersionConflict { expected, actual } => Self::Conflict(format!(
                "version conflict: expected {expected}, actual {actual}"
            )),
            RepositoryError::UniqueViolation { field } => {
                Self::Conflict(format!("unique violation on: {field}"))
            }
            RepositoryError::Database(e) => Self::Database(e.to_string()),
            RepositoryError::Unexpected(e) => Self::Database(e.to_string()),
            RepositoryError::InvalidState { reason } => Self::Conflict(reason),
        }
    }
}

impl From<ValidationError> for ApiError {
    fn from(e: ValidationError) -> Self {
        Self::BadRequest(e.to_string())
    }
}

/// `GET /v1/environments/{env_id}/segments` — List active segments.
///
/// # Errors
/// Returns [`ApiError::Database`] if the repository fails to list segments.
#[utoipa::path(
    get,
    path = "/v1/environments/{env_id}/segments",
    params(("env_id" = String, Path, description = "Environment UUID")),
    responses(
        (status = 200, description = "List of segments", body = Vec<stitchd_core::segment::Segment>),
        (status = 500, description = "Internal error"),
    ),
    tag = "segments"
)]
pub async fn list_segments(
    Path(env_id): Path<EnvironmentId>,
    State(state): State<AppState>,
) -> Result<Json<Vec<Segment>>, ApiError> {
    let segments = state.segment_repo.list_by_environment(env_id).await?;
    Ok(Json(segments))
}

/// `POST /v1/environments/{env_id}/segments` — Create segment.
///
/// # Errors
/// Returns [`ApiError::BadRequest`] if validation fails or [`ApiError::Conflict`] if the key exists.
#[utoipa::path(
    post,
    path = "/v1/environments/{env_id}/segments",
    params(("env_id" = String, Path, description = "Environment UUID")),
    request_body = CreateSegmentRequest,
    responses(
        (status = 201, description = "Segment created", body = stitchd_core::segment::Segment),
        (status = 422, description = "Validation error"),
        (status = 409, description = "Key conflict"),
    ),
    tag = "segments"
)]
pub async fn create_segment(
    Path(env_id): Path<EnvironmentId>,
    State(state): State<AppState>,
    Json(req): Json<CreateSegmentRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let segment = Segment {
        id: SegmentId::new(),
        environment_id: env_id,
        key: req.key,
        segment_type: req.segment_type,
        created_at: Utc::now(),
        updated_at: Utc::now(),
        deleted_at: None,
        version: 1,
    };

    state.segment_repo.create(&segment).await?;

    match segment.segment_type {
        SegmentType::Rule => {
            let rules = req
                .rules
                .ok_or(ValidationError::MissingDefinition(SegmentType::Rule))?;
            validate_rules(&rules)?;
            state.segment_repo.upsert_rules(segment.id, &rules).await?;
        }
        SegmentType::List => {
            let lists = req
                .lists
                .ok_or(ValidationError::MissingDefinition(SegmentType::List))?;
            for (context_type, list) in lists {
                state
                    .segment_repo
                    .set_list_entries(
                        segment.id,
                        &context_type,
                        &list.include.into_iter().collect::<Vec<_>>(),
                        &list.exclude.into_iter().collect::<Vec<_>>(),
                    )
                    .await?;
            }
        }
    }

    Ok((StatusCode::CREATED, Json(segment)))
}

/// `GET /v1/environments/{env_id}/segments/{seg_id}` — Get segment + definition.
///
/// # Errors
/// Returns [`ApiError::NotFound`] if the segment does not exist.
#[utoipa::path(
    get,
    path = "/v1/environments/{env_id}/segments/{seg_id}",
    params(
        ("env_id" = String, Path, description = "Environment UUID"),
        ("seg_id" = String, Path, description = "Segment UUID"),
    ),
    responses(
        (status = 200, description = "Segment details", body = SegmentResponse),
        (status = 404, description = "Segment not found"),
    ),
    tag = "segments"
)]
pub async fn get_segment(
    Path((_env_id, seg_id)): Path<(EnvironmentId, SegmentId)>,
    State(state): State<AppState>,
) -> Result<Json<SegmentResponse>, ApiError> {
    let segment = state.segment_repo.find_by_id(seg_id).await?;

    let definition = match segment.segment_type {
        SegmentType::Rule => {
            SegmentDefinition::RuleBased(state.segment_repo.find_with_rules(seg_id).await?)
        }
        SegmentType::List => {
            SegmentDefinition::ListBased(state.segment_repo.find_with_list(seg_id).await?)
        }
    };

    Ok(Json(SegmentResponse {
        id: segment.id,
        key: segment.key,
        segment_type: segment.segment_type,
        definition,
        version: segment.version,
    }))
}

/// `PUT /v1/environments/{env_id}/segments/{seg_id}` — Replace definition.
///
/// # Errors
/// Returns [`ApiError::Conflict`] if the version mismatch or [`ApiError::NotFound`] if missing.
#[utoipa::path(
    put,
    path = "/v1/environments/{env_id}/segments/{seg_id}",
    params(
        ("env_id" = String, Path, description = "Environment UUID"),
        ("seg_id" = String, Path, description = "Segment UUID"),
    ),
    request_body = UpdateSegmentRequest,
    responses(
        (status = 200, description = "Updated segment"),
        (status = 404, description = "Segment not found"),
        (status = 409, description = "Version conflict"),
    ),
    tag = "segments"
)]
pub async fn update_segment(
    Path((_env_id, seg_id)): Path<(EnvironmentId, SegmentId)>,
    State(state): State<AppState>,
    Json(req): Json<UpdateSegmentRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let mut segment = state.segment_repo.find_by_id(seg_id).await?;

    if segment.version != req.version {
        return Err(ApiError::Conflict(format!(
            "version conflict: expected {}, actual {}",
            req.version, segment.version
        )));
    }

    match segment.segment_type {
        SegmentType::Rule => {
            let rules = req
                .rules
                .ok_or(ValidationError::MissingDefinition(SegmentType::Rule))?;
            validate_rules(&rules)?;
            state.segment_repo.upsert_rules(seg_id, &rules).await?;
        }
        SegmentType::List => {
            let lists = req
                .lists
                .ok_or(ValidationError::MissingDefinition(SegmentType::List))?;
            for (context_type, list) in lists {
                state
                    .segment_repo
                    .set_list_entries(
                        seg_id,
                        &context_type,
                        &list.include.into_iter().collect::<Vec<_>>(),
                        &list.exclude.into_iter().collect::<Vec<_>>(),
                    )
                    .await?;
            }
        }
    }

    // reload segment to get updated version/timestamp
    segment = state.segment_repo.find_by_id(seg_id).await?;

    Ok(Json(segment))
}

/// `DELETE /v1/environments/{env_id}/segments/{seg_id}` — Soft-delete.
///
/// # Errors
/// Returns [`ApiError::NotFound`] if the segment does not exist.
#[utoipa::path(
    delete,
    path = "/v1/environments/{env_id}/segments/{seg_id}",
    params(
        ("env_id" = String, Path, description = "Environment UUID"),
        ("seg_id" = String, Path, description = "Segment UUID"),
    ),
    responses(
        (status = 204, description = "Segment deleted"),
        (status = 404, description = "Segment not found"),
    ),
    tag = "segments"
)]
pub async fn delete_segment(
    Path((_env_id, seg_id)): Path<(EnvironmentId, SegmentId)>,
    State(state): State<AppState>,
) -> Result<impl IntoResponse, ApiError> {
    state.segment_repo.soft_delete(seg_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /v1/environments/{env_id}/segments/list-check` — Check list-segment membership.
///
/// Requires a valid `x-sdk-key` header for the target environment.
///
/// # Errors
/// Returns [`ApiError::BadRequest`] for empty segment keys, [`ApiError::Database`] on failure.
#[utoipa::path(
    post,
    path = "/v1/environments/{env_id}/segments/list-check",
    params(("env_id" = String, Path, description = "Environment UUID")),
    request_body = ListCheckRequest,
    responses(
        (status = 200, description = "Membership results", body = ListCheckResponse),
        (status = 500, description = "Internal error"),
    ),
    tag = "segments",
    security(("sdk_key" = []))
)]
pub async fn list_check_membership(
    _auth: SdkAuth,
    Path(env_id): Path<EnvironmentId>,
    State(state): State<AppState>,
    Json(req): Json<ListCheckRequest>,
) -> Result<Json<ListCheckResponse>, ApiError> {
    if req.segment_keys.is_empty() {
        return Ok(Json(ListCheckResponse {
            memberships: std::collections::HashMap::new(),
        }));
    }

    let memberships = state
        .segment_repo
        .check_list_membership(
            env_id,
            &req.context_type,
            &req.context_key,
            &req.segment_keys,
        )
        .await?;

    Ok(Json(ListCheckResponse { memberships }))
}

/// `POST /v1/environments/{env_id}/segments/list-check/batch` — Batch list-segment membership.
///
/// Requires a valid `x-sdk-key` header for the target environment.
///
/// # Errors
/// Returns [`ApiError::Database`] on failure.
#[utoipa::path(
    post,
    path = "/v1/environments/{env_id}/segments/list-check/batch",
    params(("env_id" = String, Path, description = "Environment UUID")),
    request_body = BatchListCheckRequest,
    responses(
        (status = 200, description = "Batch membership results", body = BatchListCheckResponse),
        (status = 500, description = "Internal error"),
    ),
    tag = "segments",
    security(("sdk_key" = []))
)]
pub async fn batch_list_check_membership(
    _auth: SdkAuth,
    Path(env_id): Path<EnvironmentId>,
    State(state): State<AppState>,
    Json(req): Json<BatchListCheckRequest>,
) -> Result<Json<BatchListCheckResponse>, ApiError> {
    if req.segment_keys.is_empty() || req.contexts.is_empty() {
        return Ok(Json(BatchListCheckResponse {
            results: Vec::new(),
        }));
    }

    let ctx_pairs: Vec<(String, String)> = req
        .contexts
        .iter()
        .map(|c| (c.context_type.clone(), c.context_key.clone()))
        .collect();

    let raw = state
        .segment_repo
        .batch_check_list_membership(env_id, &ctx_pairs, &req.segment_keys)
        .await?;

    let results = raw
        .into_iter()
        .map(|cm| BatchContextMembership {
            context_type: cm.context_type,
            context_key: cm.context_key,
            memberships: cm.memberships,
        })
        .collect();

    Ok(Json(BatchListCheckResponse { results }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::AppState;
    use async_trait::async_trait;
    use axum::{
        body::Body,
        http::{Request, StatusCode},
    };
    use metrics_exporter_prometheus::PrometheusBuilder;
    use std::{
        collections::HashMap,
        sync::{Arc, Mutex},
    };
    use stitchd_core::{
        flag::Variant,
        id::{EnvironmentId, FlagId, ProjectId, SdkKeyId, SegmentId, VariantId},
        rule_engine::types::Rule,
        segment::{ListBasedSegment, RuleBasedSegment, Segment, SegmentType},
        tenant::SdkKey,
    };
    use stitchd_db::{
        ContextMembership, FlagRepository, RepositoryError, SdkKeyRepository, SegmentRepository,
        VariantRepository,
    };
    use tower::ServiceExt as _;

    // ---------------------------------------------------------------------------
    // Minimal mock repositories — only segment operations are exercised here
    // ---------------------------------------------------------------------------

    struct MockSegmentRepo {
        segments: Mutex<Vec<Segment>>,
        fail_create: bool,
    }

    impl MockSegmentRepo {
        fn with_segments(segments: Vec<Segment>) -> Arc<Self> {
            Arc::new(Self {
                segments: Mutex::new(segments),
                fail_create: false,
            })
        }
    }

    #[async_trait]
    impl SegmentRepository for MockSegmentRepo {
        async fn find_by_id(&self, id: SegmentId) -> Result<Segment, RepositoryError> {
            self.segments
                .lock()
                .unwrap()
                .iter()
                .find(|s| s.id == id)
                .cloned()
                .ok_or(RepositoryError::NotFound { id: id.to_string() })
        }

        async fn find_by_key(
            &self,
            key: &str,
            _environment_id: EnvironmentId,
        ) -> Result<Segment, RepositoryError> {
            self.segments
                .lock()
                .unwrap()
                .iter()
                .find(|s| s.key == key)
                .cloned()
                .ok_or(RepositoryError::NotFound {
                    id: key.to_string(),
                })
        }

        async fn list_by_environment(
            &self,
            env_id: EnvironmentId,
        ) -> Result<Vec<Segment>, RepositoryError> {
            Ok(self
                .segments
                .lock()
                .unwrap()
                .iter()
                .filter(|s| s.environment_id == env_id)
                .cloned()
                .collect())
        }

        async fn create(&self, segment: &Segment) -> Result<(), RepositoryError> {
            if self.fail_create {
                return Err(RepositoryError::UniqueViolation {
                    field: "key".to_string(),
                });
            }
            self.segments.lock().unwrap().push(segment.clone());
            Ok(())
        }

        async fn update(&self, segment: &Segment) -> Result<Segment, RepositoryError> {
            Ok(segment.clone())
        }

        async fn find_with_rules(
            &self,
            id: SegmentId,
        ) -> Result<RuleBasedSegment, RepositoryError> {
            Ok(RuleBasedSegment {
                id,
                rules: Vec::new(),
            })
        }

        async fn find_with_list(&self, id: SegmentId) -> Result<ListBasedSegment, RepositoryError> {
            Ok(ListBasedSegment {
                id,
                lists: HashMap::new(),
            })
        }

        async fn upsert_rules(
            &self,
            _id: SegmentId,
            _rules: &[Rule],
        ) -> Result<(), RepositoryError> {
            Ok(())
        }

        async fn set_list_entries(
            &self,
            _id: SegmentId,
            _context_type: &str,
            _include: &[String],
            _exclude: &[String],
        ) -> Result<(), RepositoryError> {
            Ok(())
        }

        async fn soft_delete(&self, id: SegmentId) -> Result<(), RepositoryError> {
            let segs = self.segments.lock().unwrap();
            if segs.iter().any(|s| s.id == id) {
                Ok(())
            } else {
                Err(RepositoryError::NotFound { id: id.to_string() })
            }
        }

        async fn check_list_membership(
            &self,
            _environment_id: EnvironmentId,
            _context_type: &str,
            _context_key: &str,
            segment_keys: &[String],
        ) -> Result<HashMap<String, bool>, RepositoryError> {
            Ok(segment_keys.iter().map(|k| (k.clone(), true)).collect())
        }

        async fn batch_check_list_membership(
            &self,
            _environment_id: EnvironmentId,
            contexts: &[(String, String)],
            segment_keys: &[String],
        ) -> Result<Vec<ContextMembership>, RepositoryError> {
            Ok(contexts
                .iter()
                .map(|(ct, ck)| ContextMembership {
                    context_type: ct.clone(),
                    context_key: ck.clone(),
                    memberships: segment_keys.iter().map(|k| (k.clone(), true)).collect(),
                })
                .collect())
        }
    }

    struct MockFlagRepo;

    #[async_trait]
    impl FlagRepository for MockFlagRepo {
        async fn find_by_id(
            &self,
            id: FlagId,
        ) -> Result<stitchd_core::flag::FlagRecord, RepositoryError> {
            Err(RepositoryError::NotFound { id: id.to_string() })
        }

        async fn find_by_key(
            &self,
            key: &stitchd_core::id::FlagKey,
            _project_id: ProjectId,
        ) -> Result<stitchd_core::flag::FlagRecord, RepositoryError> {
            Err(RepositoryError::NotFound {
                id: key.to_string(),
            })
        }

        async fn list_by_project(
            &self,
            _project_id: ProjectId,
        ) -> Result<Vec<stitchd_core::flag::FlagRecord>, RepositoryError> {
            Ok(Vec::new())
        }

        async fn list_by_environment(
            &self,
            _environment_id: EnvironmentId,
        ) -> Result<Vec<stitchd_core::flag::FlagRecord>, RepositoryError> {
            Ok(Vec::new())
        }

        async fn create(
            &self,
            _flag: &stitchd_core::flag::FlagRecord,
        ) -> Result<(), RepositoryError> {
            Ok(())
        }

        async fn update(
            &self,
            flag: &stitchd_core::flag::FlagRecord,
        ) -> Result<stitchd_core::flag::FlagRecord, RepositoryError> {
            Ok(flag.clone())
        }

        async fn soft_delete(&self, id: FlagId) -> Result<(), RepositoryError> {
            Err(RepositoryError::NotFound { id: id.to_string() })
        }

        async fn find_hashing_config(
            &self,
            _flag_id: FlagId,
        ) -> Result<Vec<stitchd_core::flag::FlagHashingConfig>, RepositoryError> {
            Ok(Vec::new())
        }

        async fn upsert_hashing_config(
            &self,
            _flag_id: FlagId,
            _config: &[stitchd_core::flag::FlagHashingConfig],
        ) -> Result<(), RepositoryError> {
            Ok(())
        }

        async fn find_rules(
            &self,
            _flag_id: FlagId,
        ) -> Result<Vec<stitchd_core::flag::FlagRule>, RepositoryError> {
            Ok(Vec::new())
        }

        async fn upsert_rules(
            &self,
            _flag_id: FlagId,
            _rules: &[stitchd_core::flag::FlagRule],
        ) -> Result<(), RepositoryError> {
            Ok(())
        }
    }

    struct MockVariantRepo;

    #[async_trait]
    impl VariantRepository for MockVariantRepo {
        async fn find_by_flag(&self, _flag_id: FlagId) -> Result<Vec<Variant>, RepositoryError> {
            Ok(Vec::new())
        }

        async fn create(
            &self,
            _flag_id: FlagId,
            _variant: &Variant,
        ) -> Result<(), RepositoryError> {
            Ok(())
        }

        async fn update(&self, variant: &Variant) -> Result<Variant, RepositoryError> {
            Ok(variant.clone())
        }

        async fn delete(&self, _id: VariantId) -> Result<(), RepositoryError> {
            Ok(())
        }
    }

    struct MockSdkKeyRepo {
        active_keys: Vec<SdkKey>,
    }

    impl MockSdkKeyRepo {
        fn empty() -> Arc<Self> {
            Arc::new(Self {
                active_keys: Vec::new(),
            })
        }

        fn with_active_key(key_hash: String, env_id: EnvironmentId) -> Arc<Self> {
            let key = SdkKey {
                id: SdkKeyId::new(),
                environment_id: env_id,
                key_hash,
                is_active: true,
                created_at: chrono::Utc::now(),
                revoked_at: None,
            };
            Arc::new(Self {
                active_keys: vec![key],
            })
        }
    }

    #[async_trait]
    impl SdkKeyRepository for MockSdkKeyRepo {
        async fn find_by_id(&self, id: SdkKeyId) -> Result<SdkKey, RepositoryError> {
            Err(RepositoryError::NotFound { id: id.to_string() })
        }

        async fn list_by_environment(
            &self,
            _environment_id: EnvironmentId,
        ) -> Result<Vec<SdkKey>, RepositoryError> {
            Ok(self.active_keys.clone())
        }

        async fn create(&self, _key: &SdkKey) -> Result<(), RepositoryError> {
            Ok(())
        }

        async fn revoke(&self, id: SdkKeyId) -> Result<(), RepositoryError> {
            Err(RepositoryError::NotFound { id: id.to_string() })
        }

        async fn find_active_by_environment(
            &self,
            env_id: EnvironmentId,
        ) -> Result<Vec<SdkKey>, RepositoryError> {
            Ok(self
                .active_keys
                .iter()
                .filter(|k| k.environment_id == env_id && k.is_active)
                .cloned()
                .collect())
        }

        async fn find_active_by_hash(&self, key_hash: &str) -> Result<SdkKey, RepositoryError> {
            self.active_keys
                .iter()
                .find(|k| k.key_hash == key_hash)
                .cloned()
                .ok_or(RepositoryError::NotFound {
                    id: key_hash.to_string(),
                })
        }
    }

    // ---------------------------------------------------------------------------
    // Test helpers
    // ---------------------------------------------------------------------------

    #[allow(clippy::too_many_lines)]
    fn make_test_state_with_sdk_key(
        segment_repo: Arc<dyn SegmentRepository>,
        sdk_key_repo: Arc<dyn SdkKeyRepository>,
    ) -> AppState {
        struct MockEventDefinitionRepo;
        #[async_trait::async_trait]
        impl stitchd_db::EventDefinitionRepository for MockEventDefinitionRepo {
            async fn find_by_id(
                &self,
                id: stitchd_core::id::EventDefinitionId,
            ) -> Result<stitchd_core::event::EventDefinition, stitchd_db::RepositoryError>
            {
                Err(stitchd_db::RepositoryError::NotFound { id: id.to_string() })
            }
            async fn find_by_key(
                &self,
                key: &str,
                _: stitchd_core::id::EnvironmentId,
            ) -> Result<stitchd_core::event::EventDefinition, stitchd_db::RepositoryError>
            {
                Err(stitchd_db::RepositoryError::NotFound {
                    id: key.to_string(),
                })
            }
            async fn list_by_environment(
                &self,
                _: stitchd_core::id::EnvironmentId,
            ) -> Result<Vec<stitchd_core::event::EventDefinition>, stitchd_db::RepositoryError>
            {
                Ok(vec![])
            }
            async fn create(
                &self,
                _: &stitchd_core::event::EventDefinition,
            ) -> Result<(), stitchd_db::RepositoryError> {
                Ok(())
            }
            async fn update(
                &self,
                d: &stitchd_core::event::EventDefinition,
            ) -> Result<stitchd_core::event::EventDefinition, stitchd_db::RepositoryError>
            {
                Ok(d.clone())
            }
            async fn soft_delete(
                &self,
                id: stitchd_core::id::EventDefinitionId,
            ) -> Result<(), stitchd_db::RepositoryError> {
                Err(stitchd_db::RepositoryError::NotFound { id: id.to_string() })
            }
        }
        struct MockExperimentRepo;
        #[async_trait::async_trait]
        impl stitchd_db::ExperimentRepository for MockExperimentRepo {
            async fn find_by_id(
                &self,
                id: stitchd_core::id::ExperimentId,
            ) -> Result<stitchd_core::experimentation::Experiment, stitchd_db::RepositoryError>
            {
                Err(stitchd_db::RepositoryError::NotFound { id: id.to_string() })
            }
            async fn list_by_environment(
                &self,
                _: stitchd_core::id::EnvironmentId,
                _: Option<stitchd_core::experimentation::ExperimentStatus>,
            ) -> Result<Vec<stitchd_core::experimentation::Experiment>, stitchd_db::RepositoryError>
            {
                Ok(vec![])
            }
            async fn create(
                &self,
                _: &stitchd_core::experimentation::Experiment,
            ) -> Result<(), stitchd_db::RepositoryError> {
                Ok(())
            }
            async fn update(
                &self,
                e: &stitchd_core::experimentation::Experiment,
            ) -> Result<stitchd_core::experimentation::Experiment, stitchd_db::RepositoryError>
            {
                Ok(e.clone())
            }
            async fn soft_delete(
                &self,
                id: stitchd_core::id::ExperimentId,
            ) -> Result<(), stitchd_db::RepositoryError> {
                Err(stitchd_db::RepositoryError::NotFound { id: id.to_string() })
            }
            async fn list_iterations(
                &self,
                _: stitchd_core::id::ExperimentId,
            ) -> Result<
                Vec<stitchd_core::experimentation::ExperimentIteration>,
                stitchd_db::RepositoryError,
            > {
                Ok(vec![])
            }
            async fn apply_transition(
                &self,
                id: stitchd_core::id::ExperimentId,
                _: stitchd_core::experimentation::ExperimentStatus,
                _: Option<stitchd_core::id::UserId>,
            ) -> Result<stitchd_core::experimentation::Experiment, stitchd_db::RepositoryError>
            {
                Err(stitchd_db::RepositoryError::NotFound { id: id.to_string() })
            }
        }
        struct MockResultsRepo;
        #[async_trait]
        impl stitchd_db::experiment_results::ExperimentResultsRepository for MockResultsRepo {
            async fn upsert(
                &self,
                _: &stitchd_db::experiment_results::UpsertResultRow,
            ) -> Result<stitchd_db::experiment_results::ExperimentResultRow, sqlx::Error>
            {
                Err(sqlx::Error::RowNotFound)
            }
            async fn fetch_latest(
                &self,
                _: uuid::Uuid,
            ) -> Result<Vec<stitchd_db::experiment_results::ExperimentResultRow>, sqlx::Error>
            {
                Ok(vec![])
            }
            async fn fetch_by_iteration(
                &self,
                _: uuid::Uuid,
                _: uuid::Uuid,
            ) -> Result<Vec<stitchd_db::experiment_results::ExperimentResultRow>, sqlx::Error>
            {
                Ok(vec![])
            }
            async fn is_stale(&self, _: uuid::Uuid, _: uuid::Uuid) -> Result<bool, sqlx::Error> {
                Ok(false)
            }
        }

        struct MockUserRepo;
        #[async_trait::async_trait]
        impl stitchd_db::UserRepository for MockUserRepo {
            async fn find_by_id(&self, id: stitchd_core::id::UserId) -> Result<stitchd_core::auth::User, stitchd_db::RepositoryError> { Err(stitchd_db::RepositoryError::NotFound { id: id.to_string() }) }
            async fn find_by_email(&self, e: &str) -> Result<stitchd_core::auth::User, stitchd_db::RepositoryError> { Err(stitchd_db::RepositoryError::NotFound { id: e.to_string() }) }
            async fn list_by_organisation(&self, _: stitchd_core::id::OrganisationId) -> Result<Vec<stitchd_core::auth::User>, stitchd_db::RepositoryError> { Ok(vec![]) }
            async fn create(&self, _: &stitchd_core::auth::User) -> Result<(), stitchd_db::RepositoryError> { Ok(()) }
            async fn update(&self, u: &stitchd_core::auth::User) -> Result<stitchd_core::auth::User, stitchd_db::RepositoryError> { Ok(u.clone()) }
            async fn find_permissions_for_user(&self, _: stitchd_core::id::UserId, _: stitchd_core::id::ProjectId) -> Result<Vec<stitchd_core::user::Permission>, stitchd_db::RepositoryError> { Ok(vec![]) }
        }

        struct StubAuthUserRepo;
        #[async_trait::async_trait]
        impl stitchd_db::AuthUserRepository for StubAuthUserRepo {
            async fn create(&self, e: &str, _: &str, _: Option<&str>) -> Result<stitchd_core::auth::User, stitchd_db::RepositoryError> { Err(stitchd_db::RepositoryError::NotFound { id: e.to_string() }) }
            async fn find_by_email(&self, _: &str) -> Result<Option<stitchd_core::auth::User>, stitchd_db::RepositoryError> { Ok(None) }
            async fn find_by_id(&self, id: stitchd_core::id::UserId) -> Result<Option<stitchd_core::auth::User>, stitchd_db::RepositoryError> { let _ = id; Ok(None) }
            async fn rotate_token_secret(&self, id: stitchd_core::id::UserId) -> Result<uuid::Uuid, stitchd_db::RepositoryError> { Err(stitchd_db::RepositoryError::NotFound { id: id.to_string() }) }
            async fn update_status(&self, id: stitchd_core::id::UserId, _: stitchd_core::auth::UserStatus) -> Result<(), stitchd_db::RepositoryError> { Err(stitchd_db::RepositoryError::NotFound { id: id.to_string() }) }
            async fn update_password_hash(&self, id: stitchd_core::id::UserId, _: &str) -> Result<(), stitchd_db::RepositoryError> { Err(stitchd_db::RepositoryError::NotFound { id: id.to_string() }) }
            async fn update_profile(&self, id: stitchd_core::id::UserId, _: &str, _: Option<&str>) -> Result<(), stitchd_db::RepositoryError> { Err(stitchd_db::RepositoryError::NotFound { id: id.to_string() }) }
            async fn list_org_users(&self, _: stitchd_core::id::OrganisationId) -> Result<Vec<(stitchd_core::auth::User, stitchd_core::auth::OrgRole)>, stitchd_db::RepositoryError> { Ok(vec![]) }
        }
        struct StubMembershipRepo;
        #[async_trait::async_trait]
        impl stitchd_db::OrgMembershipRepository for StubMembershipRepo {
            async fn add_member(&self, id: stitchd_core::id::UserId, _: stitchd_core::id::OrganisationId, _: stitchd_core::auth::OrgRole) -> Result<stitchd_core::auth::OrgMembership, stitchd_db::RepositoryError> { Err(stitchd_db::RepositoryError::NotFound { id: id.to_string() }) }
            async fn find_membership(&self, _: stitchd_core::id::UserId, _: stitchd_core::id::OrganisationId) -> Result<Option<stitchd_core::auth::OrgMembership>, stitchd_db::RepositoryError> { Ok(None) }
            async fn list_orgs_for_user(&self, _: stitchd_core::id::UserId) -> Result<Vec<stitchd_core::auth::OrgMembership>, stitchd_db::RepositoryError> { Ok(vec![]) }
            async fn remove_member(&self, id: stitchd_core::id::UserId, _: stitchd_core::id::OrganisationId) -> Result<(), stitchd_db::RepositoryError> { Err(stitchd_db::RepositoryError::NotFound { id: id.to_string() }) }
            async fn update_role(&self, id: stitchd_core::id::UserId, _: stitchd_core::id::OrganisationId, _: stitchd_core::auth::OrgRole) -> Result<(), stitchd_db::RepositoryError> { Err(stitchd_db::RepositoryError::NotFound { id: id.to_string() }) }
        }
        struct StubRefreshTokenRepo;
        #[async_trait::async_trait]
        impl stitchd_db::RefreshTokenRepository for StubRefreshTokenRepo {
            async fn create(&self, id: stitchd_core::id::UserId, _: stitchd_core::id::OrganisationId, _: Option<&str>, _: i64) -> Result<(stitchd_core::auth::RefreshToken, String), stitchd_db::RepositoryError> { Err(stitchd_db::RepositoryError::NotFound { id: id.to_string() }) }
            async fn find_by_hash(&self, _: &str) -> Result<Option<stitchd_core::auth::RefreshToken>, stitchd_db::RepositoryError> { Ok(None) }
            async fn consume(&self, id: stitchd_core::id::RefreshTokenId) -> Result<Option<stitchd_core::auth::RefreshToken>, stitchd_db::RepositoryError> { Err(stitchd_db::RepositoryError::NotFound { id: id.to_string() }) }
            async fn revoke(&self, id: stitchd_core::id::RefreshTokenId) -> Result<(), stitchd_db::RepositoryError> { Err(stitchd_db::RepositoryError::NotFound { id: id.to_string() }) }
            async fn revoke_all_for_user(&self, id: stitchd_core::id::UserId) -> Result<(), stitchd_db::RepositoryError> { Err(stitchd_db::RepositoryError::NotFound { id: id.to_string() }) }
            async fn list_active(&self, _: stitchd_core::id::UserId) -> Result<Vec<stitchd_core::auth::RefreshToken>, stitchd_db::RepositoryError> { Ok(vec![]) }
        }
        struct StubInviteRepo;
        #[async_trait::async_trait]
        impl stitchd_db::InviteRepository for StubInviteRepo {
            async fn create(&self, _: stitchd_core::id::OrganisationId, _: &str, _: stitchd_core::auth::OrgRole, _: Option<stitchd_core::id::UserId>, _: i64) -> Result<(stitchd_core::auth::Invite, String), stitchd_db::RepositoryError> { Err(stitchd_db::RepositoryError::NotFound { id: "stub".to_string() }) }
            async fn find_by_token_hash(&self, _: &str) -> Result<Option<stitchd_core::auth::Invite>, stitchd_db::RepositoryError> { Ok(None) }
            async fn accept(&self, id: stitchd_core::id::InviteId) -> Result<(), stitchd_db::RepositoryError> { Err(stitchd_db::RepositoryError::NotFound { id: id.to_string() }) }
            async fn list_for_org(&self, _: stitchd_core::id::OrganisationId) -> Result<Vec<stitchd_core::auth::Invite>, stitchd_db::RepositoryError> { Ok(vec![]) }
            async fn revoke(&self, id: stitchd_core::id::InviteId) -> Result<(), stitchd_db::RepositoryError> { Err(stitchd_db::RepositoryError::NotFound { id: id.to_string() }) }
        }
        struct StubOtpRepo;
        #[async_trait::async_trait]
        impl stitchd_db::OtpRepository for StubOtpRepo {
            async fn create(&self, _: &str) -> Result<(uuid::Uuid, String), stitchd_db::RepositoryError> { Err(stitchd_db::RepositoryError::NotFound { id: "stub".to_string() }) }
            async fn find_valid_by_email(&self, _: &str) -> Result<Option<(uuid::Uuid, String)>, stitchd_db::RepositoryError> { Ok(None) }
            async fn consume(&self, id: uuid::Uuid) -> Result<(), stitchd_db::RepositoryError> { Err(stitchd_db::RepositoryError::NotFound { id: id.to_string() }) }
        }

        let db =
            sqlx::PgPool::connect_lazy("postgres://stitchd:stitchd@localhost:5432/stitchd_test")
                .expect("lazy pool creation should never fail");
        AppState {
            db,
            metrics_handle: PrometheusBuilder::new().build_recorder().handle(),
            user_repo: Arc::new(MockUserRepo),
            segment_repo,
            flag_repo: Arc::new(MockFlagRepo),
            variant_repo: Arc::new(MockVariantRepo),
            sdk_key_repo,
            event_definition_repo: Arc::new(MockEventDefinitionRepo),
            experiment_repo: Arc::new(MockExperimentRepo),
            results_repo: Arc::new(MockResultsRepo),
            ch_client: None,
            event_writer: None,
            auth_user_repo: Arc::new(StubAuthUserRepo),
            membership_repo: Arc::new(StubMembershipRepo),
            refresh_token_repo: Arc::new(StubRefreshTokenRepo),
            email_service: Arc::new(crate::email::EmailService::from_env()),
            invite_repo: Arc::new(StubInviteRepo),
            otp_repo: Arc::new(StubOtpRepo),
        }
    }

    fn make_test_state(segment_repo: Arc<dyn SegmentRepository>) -> AppState {
        make_test_state_with_sdk_key(segment_repo, MockSdkKeyRepo::empty())
    }

    fn make_rule_segment(env_id: EnvironmentId) -> Segment {
        Segment {
            id: SegmentId::new(),
            environment_id: env_id,
            key: "test-segment".to_string(),
            segment_type: SegmentType::Rule,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            deleted_at: None,
            version: 1,
        }
    }

    fn make_list_segment(env_id: EnvironmentId) -> Segment {
        Segment {
            id: SegmentId::new(),
            environment_id: env_id,
            key: "list-segment".to_string(),
            segment_type: SegmentType::List,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            deleted_at: None,
            version: 1,
        }
    }

    fn build_router(state: AppState) -> axum::Router {
        crate::api::router::build_api_router().with_state(state)
    }

    // ---------------------------------------------------------------------------
    // Tests: list_segments
    // ---------------------------------------------------------------------------

    #[tokio::test]
    async fn list_segments_returns_ok_with_segments() {
        let env_id = EnvironmentId::new();
        let seg = make_rule_segment(env_id);
        let repo = MockSegmentRepo::with_segments(vec![seg]);
        let state = make_test_state(repo);
        let app = build_router(state);

        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(format!("/v1/environments/{env_id}/segments"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let segs: Vec<Segment> = serde_json::from_slice(&body).unwrap();
        assert_eq!(segs.len(), 1);
    }

    #[tokio::test]
    async fn list_segments_returns_empty_when_none_exist() {
        let env_id = EnvironmentId::new();
        let repo = MockSegmentRepo::with_segments(vec![]);
        let state = make_test_state(repo);
        let app = build_router(state);

        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(format!("/v1/environments/{env_id}/segments"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let segs: Vec<Segment> = serde_json::from_slice(&body).unwrap();
        assert!(segs.is_empty());
    }

    // ---------------------------------------------------------------------------
    // Tests: create_segment (rule-based)
    // ---------------------------------------------------------------------------

    #[tokio::test]
    async fn create_rule_segment_returns_created() {
        let env_id = EnvironmentId::new();
        let repo = MockSegmentRepo::with_segments(vec![]);
        let state = make_test_state(repo);
        let app = build_router(state);

        let body = serde_json::json!({
            "key": "beta-users",
            "segment_type": "rule",
            "rules": []
        });

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/v1/environments/{env_id}/segments"))
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_vec(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::CREATED);
    }

    #[tokio::test]
    async fn create_rule_segment_missing_rules_returns_bad_request() {
        let env_id = EnvironmentId::new();
        let repo = MockSegmentRepo::with_segments(vec![]);
        let state = make_test_state(repo);
        let app = build_router(state);

        let body = serde_json::json!({
            "key": "beta-users",
            "segment_type": "rule"
            // no "rules" field
        });

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/v1/environments/{env_id}/segments"))
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_vec(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }

    // ---------------------------------------------------------------------------
    // Tests: create_segment (list-based)
    // ---------------------------------------------------------------------------

    #[tokio::test]
    async fn create_list_segment_returns_created() {
        let env_id = EnvironmentId::new();
        let repo = MockSegmentRepo::with_segments(vec![]);
        let state = make_test_state(repo);
        let app = build_router(state);

        let body = serde_json::json!({
            "key": "allowed-users",
            "segment_type": "list",
            "lists": {}
        });

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/v1/environments/{env_id}/segments"))
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_vec(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::CREATED);
    }

    #[tokio::test]
    async fn create_list_segment_missing_lists_returns_bad_request() {
        let env_id = EnvironmentId::new();
        let repo = MockSegmentRepo::with_segments(vec![]);
        let state = make_test_state(repo);
        let app = build_router(state);

        let body = serde_json::json!({
            "key": "allowed-users",
            "segment_type": "list"
            // no "lists" field
        });

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/v1/environments/{env_id}/segments"))
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_vec(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }

    // ---------------------------------------------------------------------------
    // Tests: get_segment
    // ---------------------------------------------------------------------------

    #[tokio::test]
    async fn get_rule_segment_returns_ok_with_definition() {
        let env_id = EnvironmentId::new();
        let seg = make_rule_segment(env_id);
        let seg_id = seg.id;
        let repo = MockSegmentRepo::with_segments(vec![seg]);
        let state = make_test_state(repo);
        let app = build_router(state);

        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(format!("/v1/environments/{env_id}/segments/{seg_id}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let resp: SegmentResponse = serde_json::from_slice(&body).unwrap();
        assert_eq!(resp.id, seg_id);
        assert_eq!(resp.segment_type, SegmentType::Rule);
    }

    #[tokio::test]
    async fn get_list_segment_returns_ok_with_definition() {
        let env_id = EnvironmentId::new();
        let seg = make_list_segment(env_id);
        let seg_id = seg.id;
        let repo = MockSegmentRepo::with_segments(vec![seg]);
        let state = make_test_state(repo);
        let app = build_router(state);

        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(format!("/v1/environments/{env_id}/segments/{seg_id}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let resp: SegmentResponse = serde_json::from_slice(&body).unwrap();
        assert_eq!(resp.id, seg_id);
        assert_eq!(resp.segment_type, SegmentType::List);
    }

    #[tokio::test]
    async fn get_segment_returns_not_found_for_unknown_id() {
        let env_id = EnvironmentId::new();
        let repo = MockSegmentRepo::with_segments(vec![]);
        let state = make_test_state(repo);
        let app = build_router(state);

        let missing_id = SegmentId::new();
        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(format!("/v1/environments/{env_id}/segments/{missing_id}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    // ---------------------------------------------------------------------------
    // Tests: update_segment
    // ---------------------------------------------------------------------------

    #[tokio::test]
    async fn update_rule_segment_returns_ok() {
        let env_id = EnvironmentId::new();
        let seg = make_rule_segment(env_id);
        let seg_id = seg.id;
        let repo = MockSegmentRepo::with_segments(vec![seg]);
        let state = make_test_state(repo);
        let app = build_router(state);

        let body = serde_json::json!({
            "rules": [],
            "version": 1
        });

        let response = app
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri(format!("/v1/environments/{env_id}/segments/{seg_id}"))
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_vec(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn update_segment_returns_conflict_on_version_mismatch() {
        let env_id = EnvironmentId::new();
        let seg = make_rule_segment(env_id);
        let seg_id = seg.id;
        let repo = MockSegmentRepo::with_segments(vec![seg]);
        let state = make_test_state(repo);
        let app = build_router(state);

        let body = serde_json::json!({
            "rules": [],
            "version": 99
        });

        let response = app
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri(format!("/v1/environments/{env_id}/segments/{seg_id}"))
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_vec(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn update_rule_segment_missing_rules_returns_bad_request() {
        let env_id = EnvironmentId::new();
        let seg = make_rule_segment(env_id);
        let seg_id = seg.id;
        let repo = MockSegmentRepo::with_segments(vec![seg]);
        let state = make_test_state(repo);
        let app = build_router(state);

        let body = serde_json::json!({
            "version": 1
            // no rules for a rule-based segment
        });

        let response = app
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri(format!("/v1/environments/{env_id}/segments/{seg_id}"))
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_vec(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }

    // ---------------------------------------------------------------------------
    // Tests: delete_segment
    // ---------------------------------------------------------------------------

    #[tokio::test]
    async fn delete_segment_returns_no_content() {
        let env_id = EnvironmentId::new();
        let seg = make_rule_segment(env_id);
        let seg_id = seg.id;
        let repo = MockSegmentRepo::with_segments(vec![seg]);
        let state = make_test_state(repo);
        let app = build_router(state);

        let response = app
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri(format!("/v1/environments/{env_id}/segments/{seg_id}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::NO_CONTENT);
    }

    #[tokio::test]
    async fn delete_segment_returns_not_found_for_unknown_id() {
        let env_id = EnvironmentId::new();
        let repo = MockSegmentRepo::with_segments(vec![]);
        let state = make_test_state(repo);
        let app = build_router(state);

        let missing_id = SegmentId::new();
        let response = app
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri(format!("/v1/environments/{env_id}/segments/{missing_id}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    // ---------------------------------------------------------------------------
    // Tests: list_check_membership (SDK-authenticated)
    // ---------------------------------------------------------------------------

    #[tokio::test]
    async fn list_check_with_valid_sdk_key_returns_memberships() {
        let env_id = EnvironmentId::new();
        let raw_key = "test-sdk-key-12345";
        let key_hash = crate::api::sdk_auth::hash_sdk_key(raw_key);
        let sdk_key_repo = MockSdkKeyRepo::with_active_key(key_hash, env_id);
        let seg_repo = MockSegmentRepo::with_segments(vec![]);
        let state = make_test_state_with_sdk_key(seg_repo, sdk_key_repo);
        let app = build_router(state);

        let body = serde_json::json!({
            "context_type": "user",
            "context_key": "user-abc",
            "segment_keys": ["beta-users"]
        });

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/v1/environments/{env_id}/segments/list-check"))
                    .header("content-type", "application/json")
                    .header("x-sdk-key", raw_key)
                    .body(Body::from(serde_json::to_vec(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn list_check_without_sdk_key_returns_401() {
        let env_id = EnvironmentId::new();
        let repo = MockSegmentRepo::with_segments(vec![]);
        let state = make_test_state(repo);
        let app = build_router(state);

        let body = serde_json::json!({
            "context_type": "user",
            "context_key": "user-abc",
            "segment_keys": ["beta-users"]
        });

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/v1/environments/{env_id}/segments/list-check"))
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_vec(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn list_check_with_invalid_sdk_key_returns_401() {
        let env_id = EnvironmentId::new();
        let sdk_key_repo = MockSdkKeyRepo::empty();
        let seg_repo = MockSegmentRepo::with_segments(vec![]);
        let state = make_test_state_with_sdk_key(seg_repo, sdk_key_repo);
        let app = build_router(state);

        let body = serde_json::json!({
            "context_type": "user",
            "context_key": "user-abc",
            "segment_keys": ["beta-users"]
        });

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/v1/environments/{env_id}/segments/list-check"))
                    .header("content-type", "application/json")
                    .header("x-sdk-key", "wrong-key")
                    .body(Body::from(serde_json::to_vec(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn list_check_empty_segment_keys_returns_empty_memberships() {
        let env_id = EnvironmentId::new();
        let raw_key = "test-sdk-key-empty";
        let key_hash = crate::api::sdk_auth::hash_sdk_key(raw_key);
        let sdk_key_repo = MockSdkKeyRepo::with_active_key(key_hash, env_id);
        let seg_repo = MockSegmentRepo::with_segments(vec![]);
        let state = make_test_state_with_sdk_key(seg_repo, sdk_key_repo);
        let app = build_router(state);

        let body = serde_json::json!({
            "context_type": "user",
            "context_key": "user-abc",
            "segment_keys": []
        });

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/v1/environments/{env_id}/segments/list-check"))
                    .header("content-type", "application/json")
                    .header("x-sdk-key", raw_key)
                    .body(Body::from(serde_json::to_vec(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let resp: ListCheckResponse = serde_json::from_slice(&body_bytes).unwrap();
        assert!(resp.memberships.is_empty());
    }

    // ---------------------------------------------------------------------------
    // Tests: batch_list_check_membership
    // ---------------------------------------------------------------------------

    #[tokio::test]
    async fn batch_list_check_with_valid_sdk_key_returns_ok() {
        let env_id = EnvironmentId::new();
        let raw_key = "test-sdk-key-batch";
        let key_hash = crate::api::sdk_auth::hash_sdk_key(raw_key);
        let sdk_key_repo = MockSdkKeyRepo::with_active_key(key_hash, env_id);
        let seg_repo = MockSegmentRepo::with_segments(vec![]);
        let state = make_test_state_with_sdk_key(seg_repo, sdk_key_repo);
        let app = build_router(state);

        let body = serde_json::json!({
            "contexts": [
                { "context_type": "user", "context_key": "user-a" }
            ],
            "segment_keys": ["beta-users"]
        });

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!(
                        "/v1/environments/{env_id}/segments/list-check/batch"
                    ))
                    .header("content-type", "application/json")
                    .header("x-sdk-key", raw_key)
                    .body(Body::from(serde_json::to_vec(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn batch_list_check_empty_inputs_returns_empty_results() {
        let env_id = EnvironmentId::new();
        let raw_key = "test-sdk-key-batch-empty";
        let key_hash = crate::api::sdk_auth::hash_sdk_key(raw_key);
        let sdk_key_repo = MockSdkKeyRepo::with_active_key(key_hash, env_id);
        let seg_repo = MockSegmentRepo::with_segments(vec![]);
        let state = make_test_state_with_sdk_key(seg_repo, sdk_key_repo);
        let app = build_router(state);

        let body = serde_json::json!({
            "contexts": [],
            "segment_keys": []
        });

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!(
                        "/v1/environments/{env_id}/segments/list-check/batch"
                    ))
                    .header("content-type", "application/json")
                    .header("x-sdk-key", raw_key)
                    .body(Body::from(serde_json::to_vec(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let resp: BatchListCheckResponse = serde_json::from_slice(&body_bytes).unwrap();
        assert!(resp.results.is_empty());
    }

    // ---------------------------------------------------------------------------
    // Tests: ApiError IntoResponse
    // ---------------------------------------------------------------------------

    #[test]
    fn api_error_not_found_is_404() {
        use axum::response::IntoResponse as _;
        let err = ApiError::NotFound("segment not found".to_string());
        assert_eq!(err.into_response().status(), StatusCode::NOT_FOUND);
    }

    #[test]
    fn api_error_conflict_is_409() {
        use axum::response::IntoResponse as _;
        let err = ApiError::Conflict("version mismatch".to_string());
        assert_eq!(err.into_response().status(), StatusCode::CONFLICT);
    }

    #[test]
    fn api_error_bad_request_is_422() {
        use axum::response::IntoResponse as _;
        let err = ApiError::BadRequest("bad input".to_string());
        assert_eq!(
            err.into_response().status(),
            StatusCode::UNPROCESSABLE_ENTITY
        );
    }

    #[test]
    fn api_error_database_is_500() {
        use axum::response::IntoResponse as _;
        let err = ApiError::Database("db error".to_string());
        assert_eq!(
            err.into_response().status(),
            StatusCode::INTERNAL_SERVER_ERROR
        );
    }

    #[test]
    fn repository_not_found_converts_correctly() {
        use axum::response::IntoResponse as _;
        let err: ApiError = RepositoryError::NotFound {
            id: "seg-id".to_string(),
        }
        .into();
        assert_eq!(err.into_response().status(), StatusCode::NOT_FOUND);
    }

    #[test]
    fn repository_version_conflict_converts_correctly() {
        use axum::response::IntoResponse as _;
        let err: ApiError = RepositoryError::VersionConflict {
            expected: 1,
            actual: 2,
        }
        .into();
        assert_eq!(err.into_response().status(), StatusCode::CONFLICT);
    }

    #[test]
    fn repository_unique_violation_converts_correctly() {
        use axum::response::IntoResponse as _;
        let err: ApiError = RepositoryError::UniqueViolation {
            field: "key".to_string(),
        }
        .into();
        assert_eq!(err.into_response().status(), StatusCode::CONFLICT);
    }

    #[test]
    fn validation_error_converts_to_bad_request() {
        use crate::api::segments::types::ValidationError;
        use axum::response::IntoResponse as _;
        let err: ApiError = ValidationError::InvalidSegmentRule.into();
        assert_eq!(
            err.into_response().status(),
            StatusCode::UNPROCESSABLE_ENTITY
        );
    }

    // RepositoryError::Database and RepositoryError::Unexpected map to 500
    #[test]
    fn repository_database_error_converts_to_500() {
        use axum::response::IntoResponse as _;
        let err: ApiError =
            RepositoryError::Database(sqlx::Error::Protocol("db connection error".to_string()))
                .into();
        assert_eq!(
            err.into_response().status(),
            StatusCode::INTERNAL_SERVER_ERROR
        );
    }

    #[test]
    fn repository_unexpected_error_converts_to_500() {
        use axum::response::IntoResponse as _;
        let err: ApiError = RepositoryError::Unexpected(anyhow::anyhow!("unexpected error")).into();
        assert_eq!(
            err.into_response().status(),
            StatusCode::INTERNAL_SERVER_ERROR
        );
    }

    // ValidationError::MissingDefinition converts to bad request
    #[test]
    fn missing_definition_validation_error_converts_to_bad_request() {
        use crate::api::segments::types::ValidationError;
        use axum::response::IntoResponse as _;
        let err: ApiError = ValidationError::MissingDefinition(SegmentType::List).into();
        assert_eq!(
            err.into_response().status(),
            StatusCode::UNPROCESSABLE_ENTITY
        );
    }

    // ---------------------------------------------------------------------------
    // Tests: update_segment (list-based)
    // ---------------------------------------------------------------------------

    #[tokio::test]
    async fn update_list_segment_returns_ok() {
        let env_id = EnvironmentId::new();
        let seg = make_list_segment(env_id);
        let seg_id = seg.id;
        let repo = MockSegmentRepo::with_segments(vec![seg]);
        let state = make_test_state(repo);
        let app = build_router(state);

        let body = serde_json::json!({
            "lists": {},
            "version": 1
        });

        let response = app
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri(format!("/v1/environments/{env_id}/segments/{seg_id}"))
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_vec(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn update_list_segment_missing_lists_returns_bad_request() {
        let env_id = EnvironmentId::new();
        let seg = make_list_segment(env_id);
        let seg_id = seg.id;
        let repo = MockSegmentRepo::with_segments(vec![seg]);
        let state = make_test_state(repo);
        let app = build_router(state);

        let body = serde_json::json!({
            "version": 1
            // no "lists" for a list-based segment
        });

        let response = app
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri(format!("/v1/environments/{env_id}/segments/{seg_id}"))
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_vec(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }
}
