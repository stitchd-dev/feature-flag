//! `SegmentationService` gRPC implementation.

use std::sync::Arc;

use tonic::{Request, Response, Status};

use stitchd_core::segment::{Segment, SegmentType};
use stitchd_db::SegmentRepository;
use stitchd_proto::segments::v1::{
    ActivateListGenerationRequest, ActivateListGenerationResponse, AddEntriesRequest,
    AddEntriesResponse, AdminSegment, CreateAdminSegmentRequest, DeleteAdminSegmentRequest,
    DeleteAdminSegmentResponse, EvaluateMembershipRequest, EvaluateMembershipResponse,
    GetAdminSegmentRequest, GetSegmentRequest, ListAdminSegmentsRequest, ListAdminSegmentsResponse,
    ListSegmentsRequest, ListSegmentsResponse, LookupSegmentEntryRequest,
    LookupSegmentEntryResponse, MutateSegmentRequest, MutateSegmentResponse,
    PatchSegmentEntriesRequest, PatchSegmentEntriesResponse, RemoveEntriesRequest,
    RemoveEntriesResponse, SegmentBundle, UpdateAdminSegmentRequest,
    segmentation_service_server::SegmentationService,
};

use crate::{
    error::SegmentationServiceError,
    segment::{parse_env_id, segment_to_list_meta_proto, segment_to_rule_proto},
};

/// Shared application state for the segmentation service.
#[derive(Clone)]
pub struct AppState {
    /// Segment repository backed by PostgreSQL.
    pub segment_repo: Arc<dyn SegmentRepository>,
    /// Optional Postgres pool used by the referential-integrity scan that
    /// blocks deleting a segment still referenced by a flag rule or another
    /// segment (`flag_lifecycle_20260604`, Phase 6). `None` disables the guard
    /// (delete proceeds unconditionally — used by the mock-based unit tests).
    pub dependency_pool: Option<sqlx::PgPool>,
}

impl AppState {
    /// Construct state with no dependency-scan pool (delete-block disabled).
    #[must_use]
    pub const fn new(segment_repo: Arc<dyn SegmentRepository>) -> Self {
        Self {
            segment_repo,
            dependency_pool: None,
        }
    }

    /// Attach the Postgres pool that powers the segment delete-block guard.
    #[must_use]
    pub fn with_dependency_pool(mut self, pool: sqlx::PgPool) -> Self {
        self.dependency_pool = Some(pool);
        self
    }
}

/// gRPC implementation of `SegmentationService`.
pub struct SegmentationServiceImpl {
    state: AppState,
}

impl SegmentationServiceImpl {
    /// Create a new service impl backed by the given state.
    #[must_use]
    pub const fn new(state: AppState) -> Self {
        Self { state }
    }

    /// Referential-integrity guard for segment delete: reject when a flag rule
    /// or another segment still references `segment_id`.
    ///
    /// On a non-empty dependent set this returns a
    /// `tonic::Status::failed_precondition` carrying the
    /// `dependency_exists:<comma-separated dependent ids>` sentinel — the
    /// gateway decodes it into a structured `409 DEPENDENCY_EXISTS` body (mirror
    /// of the `flag_locked_by_experiment:<uuid>` convention). No-op when the
    /// dependency-scan pool is not configured.
    async fn ensure_no_segment_dependents(
        &self,
        segment_id: stitchd_core::id::SegmentId,
    ) -> Result<(), Status> {
        let Some(pool) = self.state.dependency_pool.as_ref() else {
            return Ok(());
        };
        let dependents = crate::dependency_scan::dependents_of_segment(pool, segment_id)
            .await
            .map_err(|e| Status::from(SegmentationServiceError::from(e)))?;
        if dependents.is_empty() {
            return Ok(());
        }
        Err(Status::failed_precondition(
            crate::dependency_scan::dependency_exists_message(&dependents.all_ids()),
        ))
    }
}

#[tonic::async_trait]
impl SegmentationService for SegmentationServiceImpl {
    /// Fetch a single segment definition by key, returning a [`SegmentBundle`].
    async fn get_segment(
        &self,
        request: Request<GetSegmentRequest>,
    ) -> Result<Response<SegmentBundle>, Status> {
        let req = request.into_inner();
        let env_id = parse_env_id(&req.environment_id).map_err(Status::from)?;

        let seg = self
            .state
            .segment_repo
            .find_by_key(&req.segment_key, env_id)
            .await
            .map_err(|e| Status::from(SegmentationServiceError::from(e)))?;

        let mut bundle = SegmentBundle {
            rule_segments: vec![],
            list_segments: vec![],
        };

        match seg.segment_type {
            SegmentType::Rule => {
                let def = self
                    .state
                    .segment_repo
                    .find_with_rules(seg.id)
                    .await
                    .map_err(|e| Status::from(SegmentationServiceError::from(e)))?;
                let proto = segment_to_rule_proto(&seg, &def.rules).map_err(Status::from)?;
                bundle.rule_segments.push(proto);
            }
            SegmentType::List => {
                // Return lightweight metadata only — entry keys are never sent over the wire.
                let meta = segment_to_list_meta_proto(&seg);
                bundle.list_segments.push(meta);
            }
        }

        Ok(Response::new(bundle))
    }

    /// List all segments for an environment.
    async fn list_segments(
        &self,
        request: Request<ListSegmentsRequest>,
    ) -> Result<Response<ListSegmentsResponse>, Status> {
        let req = request.into_inner();
        let env_id = parse_env_id(&req.environment_id).map_err(Status::from)?;

        let segments = self
            .state
            .segment_repo
            .list_by_environment(env_id)
            .await
            .map_err(|e| Status::from(SegmentationServiceError::from(e)))?;

        let mut rule_segments = vec![];
        let mut list_segments = vec![];

        for seg in &segments {
            match seg.segment_type {
                SegmentType::Rule => {
                    let def = self
                        .state
                        .segment_repo
                        .find_with_rules(seg.id)
                        .await
                        .map_err(|e| Status::from(SegmentationServiceError::from(e)))?;
                    let proto = segment_to_rule_proto(seg, &def.rules).map_err(Status::from)?;
                    rule_segments.push(proto);
                }
                SegmentType::List => {
                    list_segments.push(segment_to_list_meta_proto(seg));
                }
            }
        }

        Ok(Response::new(ListSegmentsResponse {
            rule_segments,
            list_segments,
        }))
    }

    /// Evaluate whether a context key is a member of a segment.
    ///
    /// Handles rule-based (evaluate rules) and list-based (DB lookup) paths.
    async fn evaluate_membership(
        &self,
        request: Request<EvaluateMembershipRequest>,
    ) -> Result<Response<EvaluateMembershipResponse>, Status> {
        let req = request.into_inner();
        let env_id = parse_env_id(&req.environment_id).map_err(Status::from)?;

        let seg = self
            .state
            .segment_repo
            .find_by_key(&req.segment_key, env_id)
            .await
            .map_err(|e| Status::from(SegmentationServiceError::from(e)))?;

        let is_member = match seg.segment_type {
            SegmentType::Rule => {
                evaluate_rule_membership(&*self.state.segment_repo, &seg, &req).await?
            }
            SegmentType::List => {
                evaluate_list_membership(&*self.state.segment_repo, &req, env_id).await?
            }
        };

        Ok(Response::new(EvaluateMembershipResponse { is_member }))
    }

    /// Create, update, or delete a segment.
    async fn mutate_segment(
        &self,
        request: Request<MutateSegmentRequest>,
    ) -> Result<Response<MutateSegmentResponse>, Status> {
        use stitchd_proto::segments::v1::SegmentMutationKind;

        let req = request.into_inner();
        let env_id = parse_env_id(&req.environment_id).map_err(Status::from)?;
        let kind =
            SegmentMutationKind::try_from(req.kind).unwrap_or(SegmentMutationKind::Unspecified);

        match kind {
            SegmentMutationKind::Create => {
                mutate_create(&*self.state.segment_repo, req, env_id).await
            }
            SegmentMutationKind::Update => {
                mutate_update(&*self.state.segment_repo, req, env_id).await
            }
            SegmentMutationKind::Delete => {
                // Referential-integrity guard (Phase 6): resolve the segment and
                // block the delete if a flag rule or another segment still
                // references it (409 dependency_exists via the gateway).
                let seg_key = match &req.segment {
                    Some(
                        stitchd_proto::segments::v1::mutate_segment_request::Segment::RuleSegment(
                            r,
                        ),
                    ) => Some(r.key.clone()),
                    Some(
                        stitchd_proto::segments::v1::mutate_segment_request::Segment::ListSegment(
                            l,
                        ),
                    ) => Some(l.key.clone()),
                    None => None,
                };
                if let Some(key) = seg_key {
                    let seg = self
                        .state
                        .segment_repo
                        .find_by_key(&key, env_id)
                        .await
                        .map_err(|e| Status::from(SegmentationServiceError::from(e)))?;
                    self.ensure_no_segment_dependents(seg.id).await?;
                }
                mutate_delete(&*self.state.segment_repo, req, env_id).await
            }
            SegmentMutationKind::Unspecified => Err(Status::invalid_argument(
                "mutation kind must not be UNSPECIFIED",
            )),
        }
    }

    // -----------------------------------------------------------------------
    // Admin RPCs
    // -----------------------------------------------------------------------

    async fn list_admin_segments(
        &self,
        req: Request<ListAdminSegmentsRequest>,
    ) -> Result<Response<ListAdminSegmentsResponse>, Status> {
        let r = req.into_inner();
        let env_id = parse_env_id(&r.environment_id).map_err(Status::from)?;

        let after = stitchd_db::KeysetCursor::decode_opt(Some(&r.cursor))
            .map_err(|_| Status::invalid_argument("invalid cursor"))?;
        let limit = u64::from(stitchd_db::effective_limit(r.limit, 50, 200));

        let (segments, next_cursor) = self
            .state
            .segment_repo
            .list_by_environment_keyset(env_id, after, limit)
            .await
            .map_err(|e| Status::from(SegmentationServiceError::from(e)))?;

        let mut admin_segments = Vec::with_capacity(segments.len());
        for s in &segments {
            let list_counts = if s.segment_type == SegmentType::List {
                count_list_entries(&*self.state.segment_repo, s.id).await?
            } else {
                None
            };
            let condition_expr = if s.segment_type == SegmentType::Rule {
                self.state
                    .segment_repo
                    .get_condition_expr(s.id)
                    .await
                    .map_err(|e| Status::from(SegmentationServiceError::from(e)))?
            } else {
                None
            };
            admin_segments.push(segment_to_admin_proto_with_counts(
                s,
                list_counts,
                condition_expr.as_ref(),
            ));
        }
        Ok(Response::new(ListAdminSegmentsResponse {
            segments: admin_segments,
            next_cursor: next_cursor.unwrap_or_default(),
        }))
    }

    async fn get_admin_segment(
        &self,
        req: Request<GetAdminSegmentRequest>,
    ) -> Result<Response<AdminSegment>, Status> {
        let r = req.into_inner();
        let segment_id = r
            .segment_id
            .parse::<uuid::Uuid>()
            .map(stitchd_core::id::SegmentId::from_uuid)
            .map_err(|_| {
                Status::invalid_argument(format!("invalid segment_id: {}", r.segment_id))
            })?;

        let seg = self
            .state
            .segment_repo
            .find_by_id(segment_id)
            .await
            .map_err(|e| Status::from(SegmentationServiceError::from(e)))?;

        // For list-based segments, fetch counts (not full lists).
        let list_counts = if seg.segment_type == SegmentType::List {
            count_list_entries(&*self.state.segment_repo, segment_id).await?
        } else {
            None
        };

        let condition_expr = if seg.segment_type == SegmentType::Rule {
            self.state
                .segment_repo
                .get_condition_expr(segment_id)
                .await
                .map_err(|e| Status::from(SegmentationServiceError::from(e)))?
        } else {
            None
        };

        Ok(Response::new(segment_to_admin_proto_with_counts(
            &seg,
            list_counts,
            condition_expr.as_ref(),
        )))
    }

    async fn create_admin_segment(
        &self,
        req: Request<CreateAdminSegmentRequest>,
    ) -> Result<Response<AdminSegment>, Status> {
        use chrono::Utc;
        use stitchd_core::{id::SegmentId, segment::Segment};

        let r = req.into_inner();
        let env_id = parse_env_id(&r.environment_id).map_err(Status::from)?;

        // Determine segment type: explicit field wins, fallback to list presence.
        let seg_type = match r.segment_type.as_str() {
            "rule" => SegmentType::Rule,
            "list" => SegmentType::List,
            _ => {
                if r.user_list.is_empty() {
                    SegmentType::Rule
                } else {
                    SegmentType::List
                }
            }
        };
        let context_type = if r.context_type.is_empty() {
            "user".to_string()
        } else {
            r.context_type.clone()
        };

        let key = r.name.to_lowercase().replace(' ', "-");
        let now = Utc::now();
        let seg = Segment {
            id: SegmentId::new(),
            environment_id: env_id,
            key,
            name: r.name.clone(),
            description: r.description.clone(),
            tags: r.tags.clone(),
            segment_type: seg_type,
            created_at: now,
            updated_at: now,
            deleted_at: None,
            version: 1,
        };

        self.state
            .segment_repo
            .create(&seg)
            .await
            .map_err(|e| Status::from(SegmentationServiceError::from(e)))?;

        if seg_type == SegmentType::List {
            self.state
                .segment_repo
                .set_list_entries(seg.id, &context_type, &r.user_list, &r.excluded_keys)
                .await
                .map_err(|e| Status::from(SegmentationServiceError::from(e)))?;
        }

        // Persist condition_expr for rule-based segments.
        let condition_expr = if seg_type == SegmentType::Rule && !r.condition_expr.is_empty() {
            let v: serde_json::Value = serde_json::from_slice(&r.condition_expr)
                .map_err(|e| Status::invalid_argument(format!("invalid condition_expr: {e}")))?;
            // A3: validate no forbidden operators (InSegment / NotInSegment /
            // FlagEvaluatedAs) are present in the segment's own condition.
            validate_segment_condition_expr_proto(&v)?;
            self.state
                .segment_repo
                .set_condition_expr(seg.id, Some(&v))
                .await
                .map_err(|e| Status::from(SegmentationServiceError::from(e)))?;
            Some(v)
        } else {
            None
        };

        let list_counts = if seg_type == SegmentType::List {
            Some((
                context_type,
                u32::try_from(r.user_list.len()).unwrap_or(u32::MAX),
                u32::try_from(r.excluded_keys.len()).unwrap_or(u32::MAX),
            ))
        } else {
            None
        };
        Ok(Response::new(segment_to_admin_proto_with_counts(
            &seg,
            list_counts,
            condition_expr.as_ref(),
        )))
    }

    async fn update_admin_segment(
        &self,
        req: Request<UpdateAdminSegmentRequest>,
    ) -> Result<Response<AdminSegment>, Status> {
        let r = req.into_inner();
        let segment_id = r
            .segment_id
            .parse::<uuid::Uuid>()
            .map(stitchd_core::id::SegmentId::from_uuid)
            .map_err(|_| {
                Status::invalid_argument(format!("invalid segment_id: {}", r.segment_id))
            })?;

        let mut seg = self
            .state
            .segment_repo
            .find_by_id(segment_id)
            .await
            .map_err(|e| Status::from(SegmentationServiceError::from(e)))?;

        seg.name = r.name.clone();
        seg.description = r.description.clone();
        seg.tags = r.tags.clone();

        let updated = self
            .state
            .segment_repo
            .update(&seg)
            .await
            .map_err(|e| Status::from(SegmentationServiceError::from(e)))?;

        // Update list entries if this is a list-based segment.
        let list_counts = if updated.segment_type == SegmentType::List {
            let context_type = if r.context_type.is_empty() {
                "user".to_string()
            } else {
                r.context_type.clone()
            };
            self.state
                .segment_repo
                .set_list_entries(updated.id, &context_type, &r.user_list, &r.excluded_keys)
                .await
                .map_err(|e| Status::from(SegmentationServiceError::from(e)))?;
            Some((
                context_type,
                u32::try_from(r.user_list.len()).unwrap_or(u32::MAX),
                u32::try_from(r.excluded_keys.len()).unwrap_or(u32::MAX),
            ))
        } else {
            None
        };

        // Persist condition_expr for rule-based segments.
        let condition_expr = if updated.segment_type == SegmentType::Rule
            && !r.condition_expr.is_empty()
        {
            let v: serde_json::Value = serde_json::from_slice(&r.condition_expr)
                .map_err(|e| Status::invalid_argument(format!("invalid condition_expr: {e}")))?;
            // A3: validate no forbidden operators before persisting.
            validate_segment_condition_expr_proto(&v)?;
            self.state
                .segment_repo
                .set_condition_expr(updated.id, Some(&v))
                .await
                .map_err(|e| Status::from(SegmentationServiceError::from(e)))?;
            Some(v)
        } else {
            // Read back existing condition_expr to include in response.
            self.state
                .segment_repo
                .get_condition_expr(updated.id)
                .await
                .map_err(|e| Status::from(SegmentationServiceError::from(e)))?
        };

        Ok(Response::new(segment_to_admin_proto_with_counts(
            &updated,
            list_counts,
            condition_expr.as_ref(),
        )))
    }

    async fn delete_admin_segment(
        &self,
        req: Request<DeleteAdminSegmentRequest>,
    ) -> Result<Response<DeleteAdminSegmentResponse>, Status> {
        let r = req.into_inner();
        let segment_id = r
            .segment_id
            .parse::<uuid::Uuid>()
            .map(stitchd_core::id::SegmentId::from_uuid)
            .map_err(|_| {
                Status::invalid_argument(format!("invalid segment_id: {}", r.segment_id))
            })?;

        // Referential-integrity guard (Phase 6): block the delete if a flag rule
        // or another segment still references this segment.
        self.ensure_no_segment_dependents(segment_id).await?;

        self.state
            .segment_repo
            .soft_delete(segment_id)
            .await
            .map_err(|e| Status::from(SegmentationServiceError::from(e)))?;

        Ok(Response::new(DeleteAdminSegmentResponse {}))
    }

    async fn patch_segment_entries(
        &self,
        req: Request<PatchSegmentEntriesRequest>,
    ) -> Result<Response<PatchSegmentEntriesResponse>, Status> {
        let r = req.into_inner();
        let segment_id = r
            .segment_id
            .parse::<uuid::Uuid>()
            .map(stitchd_core::id::SegmentId::from_uuid)
            .map_err(|_| {
                Status::invalid_argument(format!("invalid segment_id: {}", r.segment_id))
            })?;

        // Validate list_type
        match r.list_type.as_str() {
            "include" | "exclude" => {}
            other => {
                return Err(Status::invalid_argument(format!(
                    "invalid list_type: {other}; expected 'include' or 'exclude'"
                )));
            }
        }

        // Use "user" as the default context_type (find_with_list removed in Scylla migration).
        let context_type = "user";

        match r.action.as_str() {
            "add" => {
                self.state
                    .segment_repo
                    .add_entries(segment_id, context_type, &r.list_type, &r.keys)
                    .await
                    .map_err(|e| Status::from(SegmentationServiceError::from(e)))?;
            }
            "remove" => {
                self.state
                    .segment_repo
                    .remove_entries(segment_id, context_type, &r.list_type, &r.keys)
                    .await
                    .map_err(|e| Status::from(SegmentationServiceError::from(e)))?;
            }
            "replace" => {
                let deduped: Vec<String> = {
                    let mut seen = std::collections::HashSet::new();
                    r.keys
                        .iter()
                        .filter(|k| seen.insert((*k).clone()))
                        .cloned()
                        .collect()
                };
                let (include, exclude) = if r.list_type == "include" {
                    (deduped.as_slice(), [].as_slice())
                } else {
                    ([].as_slice(), deduped.as_slice())
                };
                self.state
                    .segment_repo
                    .set_list_entries(segment_id, context_type, include, exclude)
                    .await
                    .map_err(|e| Status::from(SegmentationServiceError::from(e)))?;
            }
            other => {
                return Err(Status::invalid_argument(format!(
                    "invalid action: {other}; expected 'add', 'remove', or 'replace'"
                )));
            }
        }

        // Get updated counts from summary.
        let summary = self
            .state
            .segment_repo
            .get_list_segment_summary(segment_id)
            .await
            .map_err(|e| Status::from(SegmentationServiceError::from(e)))?;
        let counts = summary
            .counts
            .get(context_type)
            .cloned()
            .unwrap_or_default();

        Ok(Response::new(PatchSegmentEntriesResponse {
            include_count: u32::try_from(counts.include_count).unwrap_or(u32::MAX),
            exclude_count: u32::try_from(counts.exclude_count).unwrap_or(u32::MAX),
        }))
    }

    async fn lookup_segment_entry(
        &self,
        req: Request<LookupSegmentEntryRequest>,
    ) -> Result<Response<LookupSegmentEntryResponse>, Status> {
        let r = req.into_inner();
        let segment_id = r
            .segment_id
            .parse::<uuid::Uuid>()
            .map(stitchd_core::id::SegmentId::from_uuid)
            .map_err(|_| {
                Status::invalid_argument(format!("invalid segment_id: {}", r.segment_id))
            })?;

        let (in_include, in_exclude) = self
            .state
            .segment_repo
            .lookup_entry_raw(segment_id, "user", &r.key)
            .await
            .map_err(|e| Status::from(SegmentationServiceError::from(e)))?;

        Ok(Response::new(LookupSegmentEntryResponse {
            in_include,
            in_exclude,
        }))
    }

    async fn add_entries(
        &self,
        req: Request<AddEntriesRequest>,
    ) -> Result<Response<AddEntriesResponse>, Status> {
        let r = req.into_inner();
        let segment_id = r
            .segment_id
            .parse::<uuid::Uuid>()
            .map(stitchd_core::id::SegmentId::from_uuid)
            .map_err(|_| {
                Status::invalid_argument(format!("invalid segment_id: {}", r.segment_id))
            })?;

        // Validate list_type
        match r.list_type.as_str() {
            "include" | "exclude" => {}
            other => {
                return Err(Status::invalid_argument(format!(
                    "invalid list_type: {other}; expected 'include' or 'exclude'"
                )));
            }
        }

        let added_count = i64::try_from(r.keys.len()).unwrap_or(i64::MAX);

        self.state
            .segment_repo
            .add_entries(segment_id, &r.context_type, &r.list_type, &r.keys)
            .await
            .map_err(|e| Status::from(SegmentationServiceError::from(e)))?;

        Ok(Response::new(AddEntriesResponse { added_count }))
    }

    async fn remove_entries(
        &self,
        req: Request<RemoveEntriesRequest>,
    ) -> Result<Response<RemoveEntriesResponse>, Status> {
        let r = req.into_inner();
        let segment_id = r
            .segment_id
            .parse::<uuid::Uuid>()
            .map(stitchd_core::id::SegmentId::from_uuid)
            .map_err(|_| {
                Status::invalid_argument(format!("invalid segment_id: {}", r.segment_id))
            })?;

        // Validate list_type
        match r.list_type.as_str() {
            "include" | "exclude" => {}
            other => {
                return Err(Status::invalid_argument(format!(
                    "invalid list_type: {other}; expected 'include' or 'exclude'"
                )));
            }
        }

        let removed_count = i64::try_from(r.keys.len()).unwrap_or(i64::MAX);

        self.state
            .segment_repo
            .remove_entries(segment_id, &r.context_type, &r.list_type, &r.keys)
            .await
            .map_err(|e| Status::from(SegmentationServiceError::from(e)))?;

        Ok(Response::new(RemoveEntriesResponse { removed_count }))
    }

    async fn activate_list_generation(
        &self,
        req: Request<ActivateListGenerationRequest>,
    ) -> Result<Response<ActivateListGenerationResponse>, Status> {
        let r = req.into_inner();
        let segment_id = r
            .segment_id
            .parse::<uuid::Uuid>()
            .map(stitchd_core::id::SegmentId::from_uuid)
            .map_err(|_| {
                Status::invalid_argument(format!("invalid segment_id: {}", r.segment_id))
            })?;

        let context_type = if r.context_type.is_empty() {
            "user"
        } else {
            r.context_type.as_str()
        };

        let include_count = i64::try_from(r.include.len()).unwrap_or(i64::MAX);
        let exclude_count = i64::try_from(r.exclude.len()).unwrap_or(i64::MAX);

        // Atomic full-replace via the generation-swap: writes a fresh generation
        // for the prepared member set then CAS-flips the active pointer.
        self.state
            .segment_repo
            .set_list_entries(segment_id, context_type, &r.include, &r.exclude)
            .await
            .map_err(|e| Status::from(SegmentationServiceError::from(e)))?;

        Ok(Response::new(ActivateListGenerationResponse {
            include_count,
            exclude_count,
        }))
    }
}

// ---------------------------------------------------------------------------
// Admin proto helpers
// ---------------------------------------------------------------------------

/// Build an `AdminSegment` proto from a domain `Segment` and optional count info.
/// `list_counts` is `Some((context_type, include_count, exclude_count))` for list-based segments.
/// `condition_expr` is the raw JSON value for rule-based segments.
fn segment_to_admin_proto_with_counts(
    seg: &Segment,
    list_counts: Option<(String, u32, u32)>,
    condition_expr: Option<&serde_json::Value>,
) -> AdminSegment {
    let seg_type_str = match seg.segment_type {
        SegmentType::List => "list",
        SegmentType::Rule => "rule",
    };
    let (context_type, include_count, exclude_count) = list_counts.unwrap_or_default();
    let condition_expr_bytes = condition_expr
        .and_then(|v| serde_json::to_vec(v).ok())
        .unwrap_or_default();
    AdminSegment {
        id: seg.id.to_string(),
        environment_id: seg.environment_id.to_string(),
        name: if seg.name.is_empty() {
            seg.key.clone()
        } else {
            seg.name.clone()
        },
        description: seg.description.clone(),
        tags: seg.tags.clone(),
        condition_expr: condition_expr_bytes,
        created_at_ms: seg.created_at.timestamp_millis(),
        updated_at_ms: seg.updated_at.timestamp_millis(),
        version: u64::try_from(seg.version).unwrap_or(0),
        segment_type: seg_type_str.to_string(),
        context_type,
        include_count,
        exclude_count,
    }
}

// ---------------------------------------------------------------------------
// Count helper
// ---------------------------------------------------------------------------

/// Fetch (`context_type`, `include_count`, `exclude_count`) for a list-based segment.
/// Returns `None` if the segment has no list entries.
async fn count_list_entries(
    repo: &dyn SegmentRepository,
    segment_id: stitchd_core::id::SegmentId,
) -> Result<Option<(String, u32, u32)>, Status> {
    match repo.get_list_segment_summary(segment_id).await {
        Ok(summary) => {
            let (ctx, inc_count, exc_count) = summary
                .counts
                .into_iter()
                .next()
                .map(|(ctx, counts)| {
                    let inc = u32::try_from(counts.include_count).unwrap_or(u32::MAX);
                    let exc = u32::try_from(counts.exclude_count).unwrap_or(u32::MAX);
                    (ctx, inc, exc)
                })
                .unwrap_or_default();
            Ok(Some((ctx, inc_count, exc_count)))
        }
        Err(_) => Ok(None),
    }
}

// ---------------------------------------------------------------------------
// EvaluateMembership helpers
// ---------------------------------------------------------------------------

/// Evaluate rule-based segment membership using the core rule engine.
async fn evaluate_rule_membership(
    repo: &dyn SegmentRepository,
    seg: &stitchd_core::segment::Segment,
    req: &EvaluateMembershipRequest,
) -> Result<bool, Status> {
    use stitchd_core::{
        context::Context,
        segment::{SegmentDefinition, SegmentEvaluatorError},
    };

    let def = repo
        .find_with_rules(seg.id)
        .await
        .map_err(|e| Status::from(SegmentationServiceError::from(e)))?;

    let ctx = Context::new(&req.context_type, &req.context_key);
    let segment_def = SegmentDefinition::RuleBased(def);

    match segment_def.evaluate(&[ctx]) {
        Ok(result) => Ok(result.matched),
        Err(SegmentEvaluatorError::InvalidSegmentRule) => Err(Status::failed_precondition(
            "segment rule contains invalid in-segment condition",
        )),
        // Rule engine errors (e.g. missing parameter) indicate the rule cannot
        // be evaluated with the bare context provided to EvaluateMembership —
        // treat as not-a-member rather than an internal error.
        Err(SegmentEvaluatorError::RuleEngine(_)) => Ok(false),
    }
}

/// Evaluate list-based segment membership via DB lookup.
async fn evaluate_list_membership(
    repo: &dyn SegmentRepository,
    req: &EvaluateMembershipRequest,
    env_id: stitchd_core::id::EnvironmentId,
) -> Result<bool, Status> {
    let memberships = repo
        .check_list_membership(
            env_id,
            &req.context_type,
            &req.context_key,
            std::slice::from_ref(&req.segment_key),
        )
        .await
        .map_err(|e| Status::from(SegmentationServiceError::from(e)))?;

    Ok(*memberships.get(&req.segment_key).unwrap_or(&false))
}

// ---------------------------------------------------------------------------
// MutateSegment helpers
// ---------------------------------------------------------------------------

async fn mutate_create(
    repo: &dyn SegmentRepository,
    req: MutateSegmentRequest,
    env_id: stitchd_core::id::EnvironmentId,
) -> Result<Response<MutateSegmentResponse>, Status> {
    use chrono::Utc;
    use stitchd_core::{
        id::SegmentId,
        segment::{Segment, SegmentType},
    };
    use stitchd_proto::segments::v1::{mutate_segment_request, mutate_segment_response};

    let (seg_type, key) = match &req.segment {
        Some(mutate_segment_request::Segment::RuleSegment(r)) => (SegmentType::Rule, r.key.clone()),
        Some(mutate_segment_request::Segment::ListSegment(l)) => (SegmentType::List, l.key.clone()),
        None => return Err(Status::invalid_argument("segment payload is required")),
    };

    let now = Utc::now();
    let seg = Segment {
        id: SegmentId::new(),
        environment_id: env_id,
        key,
        name: String::new(),
        description: String::new(),
        tags: vec![],
        segment_type: seg_type,
        created_at: now,
        updated_at: now,
        deleted_at: None,
        version: 1,
    };

    repo.create(&seg)
        .await
        .map_err(|e| Status::from(SegmentationServiceError::from(e)))?;

    match &req.segment {
        Some(mutate_segment_request::Segment::RuleSegment(r)) => {
            let rules: Vec<stitchd_core::rule_engine::types::Rule> =
                serde_json::from_slice(&r.rule_payload)
                    .map_err(|e| Status::invalid_argument(format!("invalid rule payload: {e}")))?;
            if let Some(first_rule) = rules.first() {
                let expr_json = serde_json::to_value(&first_rule.condition)
                    .map_err(|e| Status::internal(format!("condition serialisation: {e}")))?;
                repo.set_condition_expr(seg.id, Some(&expr_json))
                    .await
                    .map_err(|e| Status::from(SegmentationServiceError::from(e)))?;
            }
            let proto = segment_to_rule_proto(&seg, &rules).map_err(Status::from)?;
            Ok(Response::new(MutateSegmentResponse {
                segment: Some(mutate_segment_response::Segment::RuleSegment(proto)),
                version: u64::try_from(seg.version).unwrap_or(0),
            }))
        }
        Some(mutate_segment_request::Segment::ListSegment(l)) => {
            repo.set_list_entries(seg.id, &l.context_type, &l.included_keys, &l.excluded_keys)
                .await
                .map_err(|e| Status::from(SegmentationServiceError::from(e)))?;
            Ok(Response::new(MutateSegmentResponse {
                segment: Some(mutate_segment_response::Segment::ListSegment(l.clone())),
                version: u64::try_from(seg.version).unwrap_or(0),
            }))
        }
        None => unreachable!("checked above"),
    }
}

async fn mutate_update(
    repo: &dyn SegmentRepository,
    req: MutateSegmentRequest,
    env_id: stitchd_core::id::EnvironmentId,
) -> Result<Response<MutateSegmentResponse>, Status> {
    use stitchd_proto::segments::v1::{mutate_segment_request, mutate_segment_response};

    let seg_key = match &req.segment {
        Some(mutate_segment_request::Segment::RuleSegment(r)) => r.key.clone(),
        Some(mutate_segment_request::Segment::ListSegment(l)) => l.key.clone(),
        None => return Err(Status::invalid_argument("segment payload is required")),
    };

    let mut seg = repo
        .find_by_key(&seg_key, env_id)
        .await
        .map_err(|e| Status::from(SegmentationServiceError::from(e)))?;

    seg.version = i64::try_from(req.version).unwrap_or(i64::MAX);
    let updated = repo
        .update(&seg)
        .await
        .map_err(|e| Status::from(SegmentationServiceError::from(e)))?;

    match &req.segment {
        Some(mutate_segment_request::Segment::RuleSegment(r)) => {
            let rules: Vec<stitchd_core::rule_engine::types::Rule> =
                serde_json::from_slice(&r.rule_payload)
                    .map_err(|e| Status::invalid_argument(format!("invalid rule payload: {e}")))?;
            if let Some(first_rule) = rules.first() {
                let expr_json = serde_json::to_value(&first_rule.condition)
                    .map_err(|e| Status::internal(format!("condition serialisation: {e}")))?;
                repo.set_condition_expr(updated.id, Some(&expr_json))
                    .await
                    .map_err(|e| Status::from(SegmentationServiceError::from(e)))?;
            }
            let proto = segment_to_rule_proto(&updated, &rules).map_err(Status::from)?;
            Ok(Response::new(MutateSegmentResponse {
                segment: Some(mutate_segment_response::Segment::RuleSegment(proto)),
                version: u64::try_from(updated.version).unwrap_or(0),
            }))
        }
        Some(mutate_segment_request::Segment::ListSegment(l)) => {
            repo.set_list_entries(
                updated.id,
                &l.context_type,
                &l.included_keys,
                &l.excluded_keys,
            )
            .await
            .map_err(|e| Status::from(SegmentationServiceError::from(e)))?;
            Ok(Response::new(MutateSegmentResponse {
                segment: Some(mutate_segment_response::Segment::ListSegment(l.clone())),
                version: u64::try_from(updated.version).unwrap_or(0),
            }))
        }
        None => unreachable!("checked above"),
    }
}

async fn mutate_delete(
    repo: &dyn SegmentRepository,
    req: MutateSegmentRequest,
    env_id: stitchd_core::id::EnvironmentId,
) -> Result<Response<MutateSegmentResponse>, Status> {
    use stitchd_proto::segments::v1::mutate_segment_request;

    let seg_key = match &req.segment {
        Some(mutate_segment_request::Segment::RuleSegment(r)) => r.key.clone(),
        Some(mutate_segment_request::Segment::ListSegment(l)) => l.key.clone(),
        None => return Err(Status::invalid_argument("segment payload is required")),
    };

    let seg = repo
        .find_by_key(&seg_key, env_id)
        .await
        .map_err(|e| Status::from(SegmentationServiceError::from(e)))?;

    repo.soft_delete(seg.id)
        .await
        .map_err(|e| Status::from(SegmentationServiceError::from(e)))?;

    Ok(Response::new(MutateSegmentResponse {
        segment: req.segment.map(|s| match s {
            mutate_segment_request::Segment::RuleSegment(r) => {
                stitchd_proto::segments::v1::mutate_segment_response::Segment::RuleSegment(r)
            }
            mutate_segment_request::Segment::ListSegment(l) => {
                stitchd_proto::segments::v1::mutate_segment_response::Segment::ListSegment(l)
            }
        }),
        version: u64::try_from(seg.version).unwrap_or(0),
    }))
}

// ---------------------------------------------------------------------------
// Condition-expression validation (A3 — GL-11)
// ---------------------------------------------------------------------------

/// Ops that are not permitted inside a segment's own `condition_expr`.
///
/// - `InSegment` / `NotInSegment` — would create circular segment dependencies.
/// - `FlagEvaluatedAs` — segments are resolved before flag evaluation, so a
///   flag-based condition can never be satisfied.
const SEGMENT_FORBIDDEN_OPS: &[&str] = &["InSegment", "NotInSegment", "FlagEvaluatedAs"];

/// Walk a `ConditionExpr` JSON tree and return `Status::invalid_argument` if
/// any leaf uses a forbidden operator (see [`SEGMENT_FORBIDDEN_OPS`]).
///
/// Mirrors the gateway's `validate_segment_condition_expr` but returns
/// `tonic::Status` directly so it can be used in service handlers.
fn validate_segment_condition_expr_proto(expr: &serde_json::Value) -> Result<(), Status> {
    if expr.is_null() {
        return Ok(());
    }
    // Leaf node: {"Leaf": <condition>}
    if let Some(leaf) = expr.get("Leaf") {
        if let Some(obj) = leaf.as_object() {
            for op in obj.keys() {
                if SEGMENT_FORBIDDEN_OPS.contains(&op.as_str()) {
                    return Err(Status::invalid_argument(format!(
                        "forbidden operator: '{op}' is not allowed in segment rules \
                         (segments cannot reference other segments or flag evaluations)"
                    )));
                }
            }
        }
        return Ok(());
    }
    // And / Or: recurse into children array
    for key in &["And", "Or"] {
        if let Some(arr) = expr.get(key).and_then(|v| v.as_array()) {
            for child in arr {
                validate_segment_condition_expr_proto(child)?;
            }
            return Ok(());
        }
    }
    // Not: recurse into the single inner expression
    if let Some(inner) = expr.get("Not") {
        return validate_segment_condition_expr_proto(inner);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_error_not_found_maps_to_status() {
        let err = SegmentationServiceError::NotFound("seg-123".to_string());
        let status = Status::from(err);
        assert_eq!(status.code(), tonic::Code::NotFound);
    }

    #[test]
    fn service_error_invalid_argument_maps_to_status() {
        let err = SegmentationServiceError::InvalidArgument("bad env id".to_string());
        let status = Status::from(err);
        assert_eq!(status.code(), tonic::Code::InvalidArgument);
    }

    #[test]
    fn service_error_version_conflict_maps_to_aborted() {
        let err = SegmentationServiceError::VersionConflict {
            expected: 1,
            actual: 2,
        };
        let status = Status::from(err);
        assert_eq!(status.code(), tonic::Code::Aborted);
    }

    #[test]
    fn service_error_unique_violation_maps_to_already_exists() {
        let err = SegmentationServiceError::UniqueViolation {
            field: "key".to_string(),
        };
        let status = Status::from(err);
        assert_eq!(status.code(), tonic::Code::AlreadyExists);
    }

    #[test]
    fn service_error_internal_maps_to_internal() {
        let err = SegmentationServiceError::Internal("db error".to_string());
        let status = Status::from(err);
        assert_eq!(status.code(), tonic::Code::Internal);
    }
}
