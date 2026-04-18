//! Feature flag management handlers.

use crate::AppState;
use crate::api::segments::handlers::ApiError;
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
    evaluation::FlagEvaluator,
    flag::{FlagHashingConfig, FlagRecord, FlagRule, FlagValueType, Variant},
    id::{EnvironmentId, FlagId, FlagKey, ProjectId, VariantId},
    segment::SegmentEvaluator,
    variants::VariantValue,
};

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
#[derive(Serialize, Deserialize)]
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
            return Err(ApiError::BadRequest(format!(
                "variant {variant_id} not found for flag"
            )));
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

    let variant_value =
        serde_json::from_value(req.value).map_err(|e| ApiError::BadRequest(e.to_string()))?;

    let vv: VariantValue = variant_value;
    if !vv.matches_type(&flag.value_type) {
        return Err(ApiError::BadRequest(format!(
            "Variant value does not match flag type {:?}",
            flag.value_type
        )));
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
#[derive(Serialize, Deserialize)]
pub struct EvaluationResult {
    /// Flag key.
    pub flag_key: String,
    /// Matched variant key.
    pub variant_key: String,
    /// Variant value.
    pub variant_value: serde_json::Value,
}

/// Response containing all evaluated flags for the requested context.
#[derive(Serialize, Deserialize)]
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
                stitchd_core::segment::SegmentDefinition::RuleBased(
                    state.segment_repo.find_with_rules(sr.id).await?,
                )
            }
            stitchd_core::segment::SegmentType::List => {
                stitchd_core::segment::SegmentDefinition::ListBased(
                    state.segment_repo.find_with_list(sr.id).await?,
                )
            }
        };
        segment_definitions.push(def);
    }

    // 2. Resolve segments
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
        let variant = FlagEvaluator::evaluate(&flag, &req.context, &resolved_segments, env_id)
            .map_err(|e| ApiError::Database(e.to_string()))?;

        let variant_value =
            serde_json::to_value(&variant.value).map_err(|e| ApiError::Database(e.to_string()))?;

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
        flag::{FlagHashingConfig, FlagRecord, FlagRule, FlagValueType},
        id::{EnvironmentId, FlagId, FlagKey, ProjectId, SegmentId, VariantId},
        segment::{ListBasedSegment, RuleBasedSegment, Segment},
        tenant::SdkKey,
    };
    use stitchd_db::{
        ContextMembership, FlagRepository, RepositoryError, SdkKeyRepository, SegmentRepository,
        VariantRepository,
    };
    use tower::ServiceExt as _;

    // ---------------------------------------------------------------------------
    // Mock repositories
    // ---------------------------------------------------------------------------

    #[derive(Default)]
    struct MockFlagRepo {
        flags: Mutex<Vec<FlagRecord>>,
        hashing_configs: Mutex<HashMap<FlagId, Vec<FlagHashingConfig>>>,
        rules: Mutex<HashMap<FlagId, Vec<FlagRule>>>,
    }

    impl MockFlagRepo {
        fn with_flags(flags: Vec<FlagRecord>) -> Arc<Self> {
            Arc::new(Self {
                flags: Mutex::new(flags),
                hashing_configs: Mutex::new(HashMap::new()),
                rules: Mutex::new(HashMap::new()),
            })
        }
    }

    #[async_trait]
    impl FlagRepository for MockFlagRepo {
        async fn find_by_id(&self, id: FlagId) -> Result<FlagRecord, RepositoryError> {
            self.flags
                .lock()
                .unwrap()
                .iter()
                .find(|f| f.id == id)
                .cloned()
                .ok_or(RepositoryError::NotFound { id: id.to_string() })
        }

        async fn find_by_key(
            &self,
            key: &FlagKey,
            _project_id: ProjectId,
        ) -> Result<FlagRecord, RepositoryError> {
            self.flags
                .lock()
                .unwrap()
                .iter()
                .find(|f| f.key.as_str() == key.as_str())
                .cloned()
                .ok_or(RepositoryError::NotFound {
                    id: key.to_string(),
                })
        }

        async fn list_by_project(
            &self,
            project_id: ProjectId,
        ) -> Result<Vec<FlagRecord>, RepositoryError> {
            Ok(self
                .flags
                .lock()
                .unwrap()
                .iter()
                .filter(|f| f.project_id == project_id)
                .cloned()
                .collect())
        }

        async fn list_by_environment(
            &self,
            _environment_id: EnvironmentId,
        ) -> Result<Vec<FlagRecord>, RepositoryError> {
            Ok(self.flags.lock().unwrap().clone())
        }

        async fn create(&self, flag: &FlagRecord) -> Result<(), RepositoryError> {
            self.flags.lock().unwrap().push(flag.clone());
            Ok(())
        }

        async fn update(&self, flag: &FlagRecord) -> Result<FlagRecord, RepositoryError> {
            let mut flags = self.flags.lock().unwrap();
            for f in flags.iter_mut() {
                if f.id == flag.id {
                    *f = flag.clone();
                    drop(flags);
                    return Ok(flag.clone());
                }
            }
            drop(flags);
            Err(RepositoryError::NotFound {
                id: flag.id.to_string(),
            })
        }

        async fn soft_delete(&self, id: FlagId) -> Result<(), RepositoryError> {
            let flags = self.flags.lock().unwrap();
            if flags.iter().any(|f| f.id == id) {
                Ok(())
            } else {
                Err(RepositoryError::NotFound { id: id.to_string() })
            }
        }

        async fn find_hashing_config(
            &self,
            flag_id: FlagId,
        ) -> Result<Vec<FlagHashingConfig>, RepositoryError> {
            Ok(self
                .hashing_configs
                .lock()
                .unwrap()
                .get(&flag_id)
                .cloned()
                .unwrap_or_default())
        }

        async fn upsert_hashing_config(
            &self,
            flag_id: FlagId,
            config: &[FlagHashingConfig],
        ) -> Result<(), RepositoryError> {
            self.hashing_configs
                .lock()
                .unwrap()
                .insert(flag_id, config.to_vec());
            Ok(())
        }

        async fn find_rules(&self, flag_id: FlagId) -> Result<Vec<FlagRule>, RepositoryError> {
            Ok(self
                .rules
                .lock()
                .unwrap()
                .get(&flag_id)
                .cloned()
                .unwrap_or_default())
        }

        async fn upsert_rules(
            &self,
            flag_id: FlagId,
            rules: &[FlagRule],
        ) -> Result<(), RepositoryError> {
            self.rules.lock().unwrap().insert(flag_id, rules.to_vec());
            Ok(())
        }
    }

    #[derive(Default)]
    struct MockVariantRepo {
        variants: Mutex<HashMap<FlagId, Vec<stitchd_core::flag::Variant>>>,
    }

    impl MockVariantRepo {
        fn new() -> Arc<Self> {
            Arc::new(Self::default())
        }
    }

    #[async_trait]
    impl VariantRepository for MockVariantRepo {
        async fn find_by_flag(
            &self,
            flag_id: FlagId,
        ) -> Result<Vec<stitchd_core::flag::Variant>, RepositoryError> {
            Ok(self
                .variants
                .lock()
                .unwrap()
                .get(&flag_id)
                .cloned()
                .unwrap_or_default())
        }

        async fn create(
            &self,
            flag_id: FlagId,
            variant: &stitchd_core::flag::Variant,
        ) -> Result<(), RepositoryError> {
            self.variants
                .lock()
                .unwrap()
                .entry(flag_id)
                .or_default()
                .push(variant.clone());
            Ok(())
        }

        async fn update(
            &self,
            variant: &stitchd_core::flag::Variant,
        ) -> Result<stitchd_core::flag::Variant, RepositoryError> {
            Ok(variant.clone())
        }

        async fn delete(&self, _id: VariantId) -> Result<(), RepositoryError> {
            Ok(())
        }
    }

    #[derive(Default)]
    struct MockSegmentRepo;

    #[async_trait]
    impl SegmentRepository for MockSegmentRepo {
        async fn find_by_id(&self, id: SegmentId) -> Result<Segment, RepositoryError> {
            Err(RepositoryError::NotFound { id: id.to_string() })
        }

        async fn find_by_key(
            &self,
            key: &str,
            _environment_id: EnvironmentId,
        ) -> Result<Segment, RepositoryError> {
            Err(RepositoryError::NotFound {
                id: key.to_string(),
            })
        }

        async fn list_by_environment(
            &self,
            _environment_id: EnvironmentId,
        ) -> Result<Vec<Segment>, RepositoryError> {
            Ok(Vec::new())
        }

        async fn create(&self, _segment: &Segment) -> Result<(), RepositoryError> {
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
            _rules: &[stitchd_core::rule_engine::types::Rule],
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

        async fn soft_delete(&self, _id: SegmentId) -> Result<(), RepositoryError> {
            Ok(())
        }

        async fn check_list_membership(
            &self,
            _environment_id: EnvironmentId,
            _context_type: &str,
            _context_key: &str,
            segment_keys: &[String],
        ) -> Result<HashMap<String, bool>, RepositoryError> {
            Ok(segment_keys.iter().map(|k| (k.clone(), false)).collect())
        }

        async fn batch_check_list_membership(
            &self,
            _environment_id: EnvironmentId,
            _contexts: &[(String, String)],
            _segment_keys: &[String],
        ) -> Result<Vec<ContextMembership>, RepositoryError> {
            Ok(Vec::new())
        }
    }

    #[derive(Default)]
    struct MockSdkKeyRepo;

    #[async_trait]
    impl SdkKeyRepository for MockSdkKeyRepo {
        async fn find_by_id(
            &self,
            id: stitchd_core::id::SdkKeyId,
        ) -> Result<SdkKey, RepositoryError> {
            Err(RepositoryError::NotFound { id: id.to_string() })
        }

        async fn list_by_environment(
            &self,
            _environment_id: EnvironmentId,
        ) -> Result<Vec<SdkKey>, RepositoryError> {
            Ok(Vec::new())
        }

        async fn create(&self, _key: &SdkKey) -> Result<(), RepositoryError> {
            Ok(())
        }

        async fn revoke(&self, id: stitchd_core::id::SdkKeyId) -> Result<(), RepositoryError> {
            Err(RepositoryError::NotFound { id: id.to_string() })
        }

        async fn find_active_by_environment(
            &self,
            _environment_id: EnvironmentId,
        ) -> Result<Vec<SdkKey>, RepositoryError> {
            Ok(Vec::new())
        }

        async fn find_active_by_hash(&self, key_hash: &str) -> Result<SdkKey, RepositoryError> {
            Err(RepositoryError::NotFound {
                id: key_hash.to_string(),
            })
        }
    }

    // ---------------------------------------------------------------------------
    // Test helpers
    // ---------------------------------------------------------------------------

    fn make_metrics_handle() -> metrics_exporter_prometheus::PrometheusHandle {
        PrometheusBuilder::new().build_recorder().handle()
    }

    fn make_test_state(
        flag_repo: Arc<dyn FlagRepository>,
        variant_repo: Arc<dyn VariantRepository>,
    ) -> AppState {
        // Use a lazy pool — it never connects until actually used,
        // and our flag/segment handlers never touch the raw PgPool.
        let db =
            sqlx::PgPool::connect_lazy("postgres://stitchd:stitchd@localhost:5432/stitchd_test")
                .expect("lazy pool creation should never fail");
        AppState {
            db,
            metrics_handle: make_metrics_handle(),
            segment_repo: Arc::new(MockSegmentRepo),
            flag_repo,
            variant_repo,
            sdk_key_repo: Arc::new(MockSdkKeyRepo),
        }
    }

    fn make_flag_record(project_id: ProjectId) -> FlagRecord {
        FlagRecord {
            id: FlagId::new(),
            project_id,
            key: FlagKey::new("test-flag").unwrap(),
            value_type: FlagValueType::Bool,
            enabled: true,
            default_variant_id: None,
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
    // Tests: list_flags
    // ---------------------------------------------------------------------------

    #[tokio::test]
    async fn list_flags_returns_ok_and_flags() {
        let project_id = ProjectId::new();
        let flag = make_flag_record(project_id);
        let flag_repo = MockFlagRepo::with_flags(vec![flag]);
        let state = make_test_state(flag_repo, MockVariantRepo::new());
        let app = build_router(state);

        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(format!("/v1/projects/{project_id}/flags"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let flags: Vec<FlagRecord> = serde_json::from_slice(&body).unwrap();
        assert_eq!(flags.len(), 1);
    }

    #[tokio::test]
    async fn list_flags_returns_empty_when_no_flags() {
        let project_id = ProjectId::new();
        let flag_repo = MockFlagRepo::with_flags(vec![]);
        let state = make_test_state(flag_repo, MockVariantRepo::new());
        let app = build_router(state);

        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(format!("/v1/projects/{project_id}/flags"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let flags: Vec<FlagRecord> = serde_json::from_slice(&body).unwrap();
        assert!(flags.is_empty());
    }

    // ---------------------------------------------------------------------------
    // Tests: create_flag
    // ---------------------------------------------------------------------------

    #[tokio::test]
    async fn create_flag_returns_created_with_valid_payload() {
        let project_id = ProjectId::new();
        let flag_repo = MockFlagRepo::with_flags(vec![]);
        let state = make_test_state(flag_repo, MockVariantRepo::new());
        let app = build_router(state);

        let body = serde_json::json!({
            "key": "new-feature",
            "value_type": "bool",
            "enabled": true
        });

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/v1/projects/{project_id}/flags"))
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_vec(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::CREATED);
    }

    #[tokio::test]
    async fn create_flag_returns_bad_request_for_empty_key() {
        let project_id = ProjectId::new();
        let flag_repo = MockFlagRepo::with_flags(vec![]);
        let state = make_test_state(flag_repo, MockVariantRepo::new());
        let app = build_router(state);

        let body = serde_json::json!({
            "key": "",
            "value_type": "bool",
            "enabled": true
        });

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/v1/projects/{project_id}/flags"))
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_vec(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }

    // ---------------------------------------------------------------------------
    // Tests: get_flag
    // ---------------------------------------------------------------------------

    #[tokio::test]
    async fn get_flag_returns_flag_with_details() {
        let project_id = ProjectId::new();
        let flag = make_flag_record(project_id);
        let flag_id = flag.id;
        let flag_repo = MockFlagRepo::with_flags(vec![flag]);
        let state = make_test_state(flag_repo, MockVariantRepo::new());
        let app = build_router(state);

        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(format!("/v1/projects/{project_id}/flags/{flag_id}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let resp: FlagResponse = serde_json::from_slice(&body).unwrap();
        assert_eq!(resp.record.id, flag_id);
    }

    #[tokio::test]
    async fn get_flag_returns_not_found_for_unknown_id() {
        let project_id = ProjectId::new();
        let flag_repo = MockFlagRepo::with_flags(vec![]);
        let state = make_test_state(flag_repo, MockVariantRepo::new());
        let app = build_router(state);

        let missing_id = FlagId::new();
        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(format!("/v1/projects/{project_id}/flags/{missing_id}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    // ---------------------------------------------------------------------------
    // Tests: update_flag
    // ---------------------------------------------------------------------------

    #[tokio::test]
    async fn update_flag_changes_enabled_status() {
        let project_id = ProjectId::new();
        let flag = make_flag_record(project_id);
        let flag_id = flag.id;
        let flag_repo = MockFlagRepo::with_flags(vec![flag]);
        let state = make_test_state(flag_repo, MockVariantRepo::new());
        let app = build_router(state);

        let body = serde_json::json!({
            "enabled": false,
            "version": 1
        });

        let response = app
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri(format!("/v1/projects/{project_id}/flags/{flag_id}"))
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_vec(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let updated: FlagRecord = serde_json::from_slice(&body_bytes).unwrap();
        assert!(!updated.enabled);
    }

    #[tokio::test]
    async fn update_flag_returns_conflict_on_version_mismatch() {
        let project_id = ProjectId::new();
        let flag = make_flag_record(project_id);
        let flag_id = flag.id;
        let flag_repo = MockFlagRepo::with_flags(vec![flag]);
        let state = make_test_state(flag_repo, MockVariantRepo::new());
        let app = build_router(state);

        let body = serde_json::json!({
            "enabled": false,
            "version": 99
        });

        let response = app
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri(format!("/v1/projects/{project_id}/flags/{flag_id}"))
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_vec(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn update_flag_returns_bad_request_for_invalid_variant_id() {
        let project_id = ProjectId::new();
        let flag = make_flag_record(project_id);
        let flag_id = flag.id;
        let flag_repo = MockFlagRepo::with_flags(vec![flag]);
        let state = make_test_state(flag_repo, MockVariantRepo::new());
        let app = build_router(state);

        let nonexistent_variant = VariantId::new();
        let body = serde_json::json!({
            "default_variant_id": nonexistent_variant,
            "version": 1
        });

        let response = app
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri(format!("/v1/projects/{project_id}/flags/{flag_id}"))
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_vec(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }

    // ---------------------------------------------------------------------------
    // Tests: delete_flag
    // ---------------------------------------------------------------------------

    #[tokio::test]
    async fn delete_flag_returns_no_content() {
        let project_id = ProjectId::new();
        let flag = make_flag_record(project_id);
        let flag_id = flag.id;
        let flag_repo = MockFlagRepo::with_flags(vec![flag]);
        let state = make_test_state(flag_repo, MockVariantRepo::new());
        let app = build_router(state);

        let response = app
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri(format!("/v1/projects/{project_id}/flags/{flag_id}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::NO_CONTENT);
    }

    #[tokio::test]
    async fn delete_flag_returns_not_found_for_unknown_id() {
        let project_id = ProjectId::new();
        let flag_repo = MockFlagRepo::with_flags(vec![]);
        let state = make_test_state(flag_repo, MockVariantRepo::new());
        let app = build_router(state);

        let missing_id = FlagId::new();
        let response = app
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri(format!("/v1/projects/{project_id}/flags/{missing_id}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    // ---------------------------------------------------------------------------
    // Tests: create_variant
    // ---------------------------------------------------------------------------

    #[tokio::test]
    async fn create_variant_returns_created_for_valid_bool_variant() {
        let project_id = ProjectId::new();
        let flag = make_flag_record(project_id);
        let flag_id = flag.id;
        let flag_repo = MockFlagRepo::with_flags(vec![flag]);
        let state = make_test_state(flag_repo, MockVariantRepo::new());
        let app = build_router(state);

        let body = serde_json::json!({
            "key": "on",
            "value": true
        });

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!(
                        "/v1/projects/{project_id}/flags/{flag_id}/variants"
                    ))
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_vec(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::CREATED);
    }

    #[tokio::test]
    async fn create_variant_returns_bad_request_for_type_mismatch() {
        let project_id = ProjectId::new();
        let flag = make_flag_record(project_id); // value_type = Bool
        let flag_id = flag.id;
        let flag_repo = MockFlagRepo::with_flags(vec![flag]);
        let state = make_test_state(flag_repo, MockVariantRepo::new());
        let app = build_router(state);

        let body = serde_json::json!({
            "key": "on",
            "value": "string_instead_of_bool"
        });

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!(
                        "/v1/projects/{project_id}/flags/{flag_id}/variants"
                    ))
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_vec(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }

    // ---------------------------------------------------------------------------
    // Tests: update_hashing_config
    // ---------------------------------------------------------------------------

    #[tokio::test]
    async fn update_hashing_config_returns_no_content() {
        let project_id = ProjectId::new();
        let flag = make_flag_record(project_id);
        let flag_id = flag.id;
        let flag_repo = MockFlagRepo::with_flags(vec![flag]);
        let state = make_test_state(flag_repo, MockVariantRepo::new());
        let app = build_router(state);

        let config = serde_json::json!([{
            "flag_id": flag_id,
            "parameter_key": "user_id",
            "parameter_type": "user",
            "order": 0
        }]);

        let response = app
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri(format!("/v1/projects/{project_id}/flags/{flag_id}/hashing"))
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_vec(&config).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::NO_CONTENT);
    }

    // ---------------------------------------------------------------------------
    // Tests: update_rules
    // ---------------------------------------------------------------------------

    #[tokio::test]
    async fn update_rules_returns_no_content() {
        let project_id = ProjectId::new();
        let flag = make_flag_record(project_id);
        let flag_id = flag.id;
        let flag_repo = MockFlagRepo::with_flags(vec![flag]);
        let state = make_test_state(flag_repo, MockVariantRepo::new());
        let app = build_router(state);

        let rules = serde_json::json!([]);

        let response = app
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri(format!("/v1/projects/{project_id}/flags/{flag_id}/rules"))
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_vec(&rules).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::NO_CONTENT);
    }

    // ---------------------------------------------------------------------------
    // Tests: ApiError mapping
    // ---------------------------------------------------------------------------

    #[test]
    fn api_error_not_found_maps_to_404() {
        use axum::response::IntoResponse as _;
        let err = ApiError::NotFound("flag not found".to_string());
        let resp = err.into_response();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[test]
    fn api_error_conflict_maps_to_409() {
        use axum::response::IntoResponse as _;
        let err = ApiError::Conflict("version conflict".to_string());
        let resp = err.into_response();
        assert_eq!(resp.status(), StatusCode::CONFLICT);
    }

    #[test]
    fn api_error_bad_request_maps_to_422() {
        use axum::response::IntoResponse as _;
        let err = ApiError::BadRequest("bad input".to_string());
        let resp = err.into_response();
        assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[test]
    fn api_error_database_maps_to_500() {
        use axum::response::IntoResponse as _;
        let err = ApiError::Database("internal error".to_string());
        let resp = err.into_response();
        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[test]
    fn repository_error_not_found_converts_to_api_error() {
        use axum::response::IntoResponse as _;
        let repo_err = RepositoryError::NotFound {
            id: "flag-id".to_string(),
        };
        let api_err: ApiError = repo_err.into();
        let resp = api_err.into_response();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[test]
    fn repository_error_version_conflict_converts_to_conflict() {
        use axum::response::IntoResponse as _;
        let repo_err = RepositoryError::VersionConflict {
            expected: 1,
            actual: 2,
        };
        let api_err: ApiError = repo_err.into();
        let resp = api_err.into_response();
        assert_eq!(resp.status(), StatusCode::CONFLICT);
    }

    #[test]
    fn repository_error_unique_violation_converts_to_conflict() {
        use axum::response::IntoResponse as _;
        let repo_err = RepositoryError::UniqueViolation {
            field: "key".to_string(),
        };
        let api_err: ApiError = repo_err.into();
        let resp = api_err.into_response();
        assert_eq!(resp.status(), StatusCode::CONFLICT);
    }

    // ---------------------------------------------------------------------------
    // Tests: update_flag with valid variant ID
    // ---------------------------------------------------------------------------

    #[tokio::test]
    async fn update_flag_sets_default_variant_when_variant_exists() {
        let project_id = ProjectId::new();
        let flag = make_flag_record(project_id);
        let flag_id = flag.id;
        let variant_id = VariantId::new();
        let variant = stitchd_core::flag::Variant {
            id: variant_id,
            key: "on".to_string(),
            value: stitchd_core::variants::VariantValue::BoolValue(true),
        };
        let flag_repo = MockFlagRepo::with_flags(vec![flag]);
        let variant_repo = Arc::new(MockVariantRepo {
            variants: std::sync::Mutex::new(std::collections::HashMap::from([(
                flag_id,
                vec![variant],
            )])),
        });
        let state = make_test_state(flag_repo, variant_repo);
        let app = build_router(state);

        let body = serde_json::json!({
            "default_variant_id": variant_id,
            "version": 1
        });

        let response = app
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri(format!("/v1/projects/{project_id}/flags/{flag_id}"))
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_vec(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let updated: FlagRecord = serde_json::from_slice(&body_bytes).unwrap();
        assert_eq!(updated.default_variant_id, Some(variant_id));
    }

    // ---------------------------------------------------------------------------
    // Tests: evaluate_all_flags
    // ---------------------------------------------------------------------------

    #[tokio::test]
    async fn evaluate_all_flags_processes_flag_with_default_variant() {
        let env_id = EnvironmentId::new();
        let project_id = ProjectId::new();

        // Create a flag with a default variant
        let variant_id = VariantId::new();
        let flag_id = FlagId::new();
        let flag = FlagRecord {
            id: flag_id,
            project_id,
            key: FlagKey::new("my-feature").unwrap(),
            value_type: stitchd_core::flag::FlagValueType::Bool,
            enabled: false, // disabled -> falls back to default variant
            default_variant_id: Some(variant_id),
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            deleted_at: None,
            version: 1,
        };

        let variant = stitchd_core::flag::Variant {
            id: variant_id,
            key: "off".to_string(),
            value: stitchd_core::variants::VariantValue::BoolValue(false),
        };

        let flag_repo = MockFlagRepo::with_flags(vec![flag]);
        let variant_repo = Arc::new(MockVariantRepo {
            variants: std::sync::Mutex::new(std::collections::HashMap::from([(
                flag_id,
                vec![variant],
            )])),
        });
        let state = AppState {
            db: sqlx::PgPool::connect_lazy(
                "postgres://stitchd:stitchd@localhost:5432/stitchd_test",
            )
            .expect("lazy pool"),
            metrics_handle: make_metrics_handle(),
            segment_repo: Arc::new(MockSegmentRepo),
            flag_repo,
            variant_repo,
            sdk_key_repo: Arc::new(MockSdkKeyRepo),
        };
        let app = build_router(state);

        let body = serde_json::json!({
            "context": {
                "contexts": []
            }
        });

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/v1/environments/{env_id}/evaluate"))
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_vec(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let resp: BatchEvaluationResponse = serde_json::from_slice(&body_bytes).unwrap();
        assert_eq!(resp.results.len(), 1);
        assert_eq!(resp.results[0].flag_key, "my-feature");
        assert_eq!(resp.results[0].variant_key, "off");
    }

    #[tokio::test]
    async fn evaluate_all_flags_returns_ok_for_empty_environment() {
        let env_id = EnvironmentId::new();
        let flag_repo = MockFlagRepo::with_flags(vec![]);
        let state = AppState {
            db: sqlx::PgPool::connect_lazy(
                "postgres://stitchd:stitchd@localhost:5432/stitchd_test",
            )
            .expect("lazy pool"),
            metrics_handle: make_metrics_handle(),
            segment_repo: Arc::new(MockSegmentRepo),
            flag_repo,
            variant_repo: MockVariantRepo::new(),
            sdk_key_repo: Arc::new(MockSdkKeyRepo),
        };
        let app = build_router(state);

        let body = serde_json::json!({
            "context": {
                "contexts": []
            }
        });

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/v1/environments/{env_id}/evaluate"))
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_vec(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let resp: BatchEvaluationResponse = serde_json::from_slice(&body_bytes).unwrap();
        assert!(resp.results.is_empty());
    }
}
