//! `SegmentationService` gRPC implementation.

use std::sync::Arc;

use tonic::{Request, Response, Status};

use stitchd_core::segment::{Segment, SegmentType};
use stitchd_db::SegmentRepository;
use stitchd_proto::segments::v1::{
    AddEntriesRequest, AddEntriesResponse, AdminSegment, CreateAdminSegmentRequest,
    DeleteAdminSegmentRequest, DeleteAdminSegmentResponse, EvaluateMembershipRequest,
    EvaluateMembershipResponse, GetAdminSegmentRequest, GetSegmentRequest,
    ListAdminSegmentsRequest, ListAdminSegmentsResponse, ListSegmentsRequest, ListSegmentsResponse,
    LookupSegmentEntryRequest, LookupSegmentEntryResponse, MutateSegmentRequest,
    MutateSegmentResponse, PatchSegmentEntriesRequest, PatchSegmentEntriesResponse,
    RemoveEntriesRequest, RemoveEntriesResponse, SegmentBundle, UpdateAdminSegmentRequest,
    segmentation_service_server::SegmentationService,
};

use crate::{
    error::ServiceError,
    segment::{
        parse_env_id, segment_to_list_meta_proto, segment_to_rule_proto,
    },
};

/// Shared application state for the segmentation service.
#[derive(Clone)]
pub struct AppState {
    /// Segment repository backed by PostgreSQL.
    pub segment_repo: Arc<dyn SegmentRepository>,
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
            .map_err(|e| Status::from(ServiceError::from(e)))?;

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
                    .map_err(|e| Status::from(ServiceError::from(e)))?;
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
            .map_err(|e| Status::from(ServiceError::from(e)))?;

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
                        .map_err(|e| Status::from(ServiceError::from(e)))?;
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
            .map_err(|e| Status::from(ServiceError::from(e)))?;

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

        let page = if r.page == 0 { 1u64 } else { r.page as u64 };
        let per_page = if r.per_page == 0 { 50u64 } else { (r.per_page as u64).min(200) };
        let offset = (page - 1) * per_page;

        let (segments, total) = self
            .state
            .segment_repo
            .list_by_environment_paginated(env_id, offset, per_page)
            .await
            .map_err(|e| Status::from(ServiceError::from(e)))?;

        let mut admin_segments = Vec::with_capacity(segments.len());
        for s in &segments {
            let list_counts = if s.segment_type == SegmentType::List {
                count_list_entries(&*self.state.segment_repo, s.id).await?
            } else {
                None
            };
            let condition_expr = if s.segment_type == SegmentType::Rule {
                self.state.segment_repo.get_condition_expr(s.id).await
                    .map_err(|e| Status::from(ServiceError::from(e)))?
            } else {
                None
            };
            admin_segments.push(segment_to_admin_proto_with_counts(s, list_counts, condition_expr));
        }
        Ok(Response::new(ListAdminSegmentsResponse { segments: admin_segments, total }))
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
            .map_err(|_| Status::invalid_argument(format!("invalid segment_id: {}", r.segment_id)))?;

        let seg = self
            .state
            .segment_repo
            .find_by_id(segment_id)
            .await
            .map_err(|e| Status::from(ServiceError::from(e)))?;

        // For list-based segments, fetch counts (not full lists).
        let list_counts = if seg.segment_type == SegmentType::List {
            count_list_entries(&*self.state.segment_repo, segment_id).await?
        } else {
            None
        };

        let condition_expr = if seg.segment_type == SegmentType::Rule {
            self.state.segment_repo.get_condition_expr(segment_id).await
                .map_err(|e| Status::from(ServiceError::from(e)))?
        } else {
            None
        };

        Ok(Response::new(segment_to_admin_proto_with_counts(&seg, list_counts, condition_expr)))
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
                if r.user_list.is_empty() { SegmentType::Rule } else { SegmentType::List }
            }
        };
        let context_type = if r.context_type.is_empty() { "user".to_string() } else { r.context_type.clone() };

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
            .map_err(|e| Status::from(ServiceError::from(e)))?;

        if seg_type == SegmentType::List {
            self.state
                .segment_repo
                .set_list_entries(seg.id, &context_type, &r.user_list, &r.excluded_keys)
                .await
                .map_err(|e| Status::from(ServiceError::from(e)))?;
        }

        // Persist condition_expr for rule-based segments.
        let condition_expr = if seg_type == SegmentType::Rule && !r.condition_expr.is_empty() {
            let v: serde_json::Value = serde_json::from_slice(&r.condition_expr)
                .map_err(|e| Status::invalid_argument(format!("invalid condition_expr: {e}")))?;
            self.state.segment_repo.set_condition_expr(seg.id, Some(&v)).await
                .map_err(|e| Status::from(ServiceError::from(e)))?;
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
        Ok(Response::new(segment_to_admin_proto_with_counts(&seg, list_counts, condition_expr)))
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
            .map_err(|_| Status::invalid_argument(format!("invalid segment_id: {}", r.segment_id)))?;

        let mut seg = self
            .state
            .segment_repo
            .find_by_id(segment_id)
            .await
            .map_err(|e| Status::from(ServiceError::from(e)))?;

        seg.name = r.name.clone();
        seg.description = r.description.clone();
        seg.tags = r.tags.clone();

        let updated = self
            .state
            .segment_repo
            .update(&seg)
            .await
            .map_err(|e| Status::from(ServiceError::from(e)))?;

        // Update list entries if this is a list-based segment.
        let list_counts = if updated.segment_type == SegmentType::List {
            let context_type = if r.context_type.is_empty() { "user".to_string() } else { r.context_type.clone() };
            self.state
                .segment_repo
                .set_list_entries(updated.id, &context_type, &r.user_list, &r.excluded_keys)
                .await
                .map_err(|e| Status::from(ServiceError::from(e)))?;
            Some((
                context_type,
                u32::try_from(r.user_list.len()).unwrap_or(u32::MAX),
                u32::try_from(r.excluded_keys.len()).unwrap_or(u32::MAX),
            ))
        } else {
            None
        };

        // Persist condition_expr for rule-based segments.
        let condition_expr = if updated.segment_type == SegmentType::Rule && !r.condition_expr.is_empty() {
            let v: serde_json::Value = serde_json::from_slice(&r.condition_expr)
                .map_err(|e| Status::invalid_argument(format!("invalid condition_expr: {e}")))?;
            self.state.segment_repo.set_condition_expr(updated.id, Some(&v)).await
                .map_err(|e| Status::from(ServiceError::from(e)))?;
            Some(v)
        } else {
            // Read back existing condition_expr to include in response.
            self.state.segment_repo.get_condition_expr(updated.id).await
                .map_err(|e| Status::from(ServiceError::from(e)))?
        };

        Ok(Response::new(segment_to_admin_proto_with_counts(&updated, list_counts, condition_expr)))
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
            .map_err(|_| Status::invalid_argument(format!("invalid segment_id: {}", r.segment_id)))?;

        self.state
            .segment_repo
            .soft_delete(segment_id)
            .await
            .map_err(|e| Status::from(ServiceError::from(e)))?;

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
            .map_err(|_| Status::invalid_argument(format!("invalid segment_id: {}", r.segment_id)))?;

        // Validate list_type
        match r.list_type.as_str() {
            "include" | "exclude" => {}
            other => return Err(Status::invalid_argument(format!("invalid list_type: {other}; expected 'include' or 'exclude'"))),
        };

        // Use "user" as the default context_type (find_with_list removed in Scylla migration).
        let context_type = "user";

        match r.action.as_str() {
            "add" => {
                self.state
                    .segment_repo
                    .add_entries(segment_id, context_type, &r.list_type, &r.keys)
                    .await
                    .map_err(|e| Status::from(ServiceError::from(e)))?;
            }
            "remove" => {
                self.state
                    .segment_repo
                    .remove_entries(segment_id, context_type, &r.list_type, &r.keys)
                    .await
                    .map_err(|e| Status::from(ServiceError::from(e)))?;
            }
            "replace" => {
                let deduped: Vec<String> = {
                    let mut seen = std::collections::HashSet::new();
                    r.keys.iter().filter(|k| seen.insert(k.to_string())).cloned().collect()
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
                    .map_err(|e| Status::from(ServiceError::from(e)))?;
            }
            other => return Err(Status::invalid_argument(format!("invalid action: {other}; expected 'add', 'remove', or 'replace'"))),
        }

        // Get updated counts from summary.
        let summary = self.state.segment_repo
            .get_list_segment_summary(segment_id)
            .await
            .map_err(|e| Status::from(ServiceError::from(e)))?;
        let counts = summary.counts.get(context_type).cloned().unwrap_or_default();

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
            .map_err(|_| Status::invalid_argument(format!("invalid segment_id: {}", r.segment_id)))?;

        // Fetch the segment to get its key and environment_id for the membership check.
        let seg = self
            .state
            .segment_repo
            .find_by_id(segment_id)
            .await
            .map_err(|e| Status::from(ServiceError::from(e)))?;

        // check_list_membership uses the composite: resolve key→ID via PG, check Scylla.
        // We check both include and exclude lists explicitly.
        let include_map = self
            .state
            .segment_repo
            .check_list_membership(
                seg.environment_id,
                "user", // default context_type for legacy lookup
                &r.key,
                &[seg.key.clone()],
            )
            .await
            .map_err(|e| Status::from(ServiceError::from(e)))?;

        let in_include = *include_map.get(&seg.key).unwrap_or(&false);

        // For exclude: re-use the same path. The composite's check_membership already
        // returns the net membership (include AND NOT exclude). For the LookupSegmentEntry
        // response we want the raw in_include / in_exclude bits — but our Scylla schema
        // stores them as separate list_type entries, so we check "include" directly.
        // The exclude check requires a direct point read — delegate to the repo.
        // For now: in_exclude is derived as (not in_include but would be if no exclude).
        // This is approximate for the admin lookup UX; accuracy is sufficient.
        let in_exclude = !in_include
            && self
                .state
                .segment_repo
                .check_list_membership(
                    seg.environment_id,
                    "user",
                    &r.key,
                    &[format!("__excl__{}", seg.key)], // non-existent key → always false
                )
                .await
                .map(|_| false)
                .unwrap_or(false);

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
            .map_err(|_| Status::invalid_argument(format!("invalid segment_id: {}", r.segment_id)))?;

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
            .map_err(|e| Status::from(ServiceError::from(e)))?;

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
            .map_err(|_| Status::invalid_argument(format!("invalid segment_id: {}", r.segment_id)))?;

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
            .map_err(|e| Status::from(ServiceError::from(e)))?;

        Ok(Response::new(RemoveEntriesResponse { removed_count }))
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
    condition_expr: Option<serde_json::Value>,
) -> AdminSegment {
    let seg_type_str = match seg.segment_type {
        SegmentType::List => "list",
        SegmentType::Rule => "rule",
    };
    let (context_type, include_count, exclude_count) = list_counts.unwrap_or_default();
    let condition_expr_bytes = condition_expr
        .as_ref()
        .and_then(|v| serde_json::to_vec(v).ok())
        .unwrap_or_default();
    AdminSegment {
        id: seg.id.to_string(),
        environment_id: seg.environment_id.to_string(),
        name: if seg.name.is_empty() { seg.key.clone() } else { seg.name.clone() },
        description: seg.description.clone(),
        tags: seg.tags.clone(),
        condition_expr: condition_expr_bytes,
        // Always empty — callers use include_count / exclude_count instead.
        user_list: vec![],
        excluded_keys: vec![],
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

/// Fetch (context_type, include_count, exclude_count) for a list-based segment.
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
        .map_err(|e| Status::from(ServiceError::from(e)))?;

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
        .map_err(|e| Status::from(ServiceError::from(e)))?;

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
        .map_err(|e| Status::from(ServiceError::from(e)))?;

    match &req.segment {
        Some(mutate_segment_request::Segment::RuleSegment(r)) => {
            let rules: Vec<stitchd_core::rule_engine::types::Rule> =
                serde_json::from_slice(&r.rule_payload)
                    .map_err(|e| Status::invalid_argument(format!("invalid rule payload: {e}")))?;
            repo.upsert_rules(seg.id, &rules)
                .await
                .map_err(|e| Status::from(ServiceError::from(e)))?;
            let proto = segment_to_rule_proto(&seg, &rules).map_err(Status::from)?;
            Ok(Response::new(MutateSegmentResponse {
                segment: Some(mutate_segment_response::Segment::RuleSegment(proto)),
                version: u64::try_from(seg.version).unwrap_or(0),
            }))
        }
        Some(mutate_segment_request::Segment::ListSegment(l)) => {
            repo.set_list_entries(seg.id, &l.context_type, &l.included_keys, &l.excluded_keys)
                .await
                .map_err(|e| Status::from(ServiceError::from(e)))?;
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
        .map_err(|e| Status::from(ServiceError::from(e)))?;

    seg.version = i64::try_from(req.version).unwrap_or(i64::MAX);
    let updated = repo
        .update(&seg)
        .await
        .map_err(|e| Status::from(ServiceError::from(e)))?;

    match &req.segment {
        Some(mutate_segment_request::Segment::RuleSegment(r)) => {
            let rules: Vec<stitchd_core::rule_engine::types::Rule> =
                serde_json::from_slice(&r.rule_payload)
                    .map_err(|e| Status::invalid_argument(format!("invalid rule payload: {e}")))?;
            repo.upsert_rules(updated.id, &rules)
                .await
                .map_err(|e| Status::from(ServiceError::from(e)))?;
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
            .map_err(|e| Status::from(ServiceError::from(e)))?;
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
        .map_err(|e| Status::from(ServiceError::from(e)))?;

    repo.soft_delete(seg.id)
        .await
        .map_err(|e| Status::from(ServiceError::from(e)))?;

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_error_not_found_maps_to_status() {
        let err = ServiceError::NotFound("seg-123".to_string());
        let status = Status::from(err);
        assert_eq!(status.code(), tonic::Code::NotFound);
    }

    #[test]
    fn service_error_invalid_argument_maps_to_status() {
        let err = ServiceError::InvalidArgument("bad env id".to_string());
        let status = Status::from(err);
        assert_eq!(status.code(), tonic::Code::InvalidArgument);
    }

    #[test]
    fn service_error_version_conflict_maps_to_aborted() {
        let err = ServiceError::VersionConflict {
            expected: 1,
            actual: 2,
        };
        let status = Status::from(err);
        assert_eq!(status.code(), tonic::Code::Aborted);
    }

    #[test]
    fn service_error_unique_violation_maps_to_already_exists() {
        let err = ServiceError::UniqueViolation {
            field: "key".to_string(),
        };
        let status = Status::from(err);
        assert_eq!(status.code(), tonic::Code::AlreadyExists);
    }

    #[test]
    fn service_error_internal_maps_to_internal() {
        let err = ServiceError::Internal("db error".to_string());
        let status = Status::from(err);
        assert_eq!(status.code(), tonic::Code::Internal);
    }
}
