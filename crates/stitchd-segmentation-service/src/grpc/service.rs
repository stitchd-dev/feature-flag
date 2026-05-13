//! `SegmentationService` gRPC implementation.

use std::sync::Arc;

use tonic::{Request, Response, Status};

use stitchd_core::segment::SegmentType;
use stitchd_db::SegmentRepository;
use stitchd_proto::segments::v1::{
    AdminSegment, CreateAdminSegmentRequest, DeleteAdminSegmentRequest, DeleteAdminSegmentResponse,
    EvaluateMembershipRequest, EvaluateMembershipResponse, GetAdminSegmentRequest,
    GetSegmentRequest, ListAdminSegmentsRequest, ListAdminSegmentsResponse, ListSegmentsRequest,
    ListSegmentsResponse, MutateSegmentRequest, MutateSegmentResponse, SegmentBundle,
    UpdateAdminSegmentRequest, segmentation_service_server::SegmentationService,
};

use crate::{
    error::ServiceError,
    segment::{
        parse_env_id, segment_to_list_meta_proto, segment_to_list_proto, segment_to_rule_proto,
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
                let def = self
                    .state
                    .segment_repo
                    .find_with_list(seg.id)
                    .await
                    .map_err(|e| Status::from(ServiceError::from(e)))?;
                let proto = segment_to_list_proto(&seg, &def);
                bundle.list_segments.push(proto);
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
    // Admin RPCs — forwarded to the gateway; stubs required for trait impl.
    // -----------------------------------------------------------------------

    async fn list_admin_segments(
        &self,
        _req: Request<ListAdminSegmentsRequest>,
    ) -> Result<Response<ListAdminSegmentsResponse>, Status> {
        Err(Status::unimplemented(
            "list_admin_segments is handled by the gateway",
        ))
    }

    async fn get_admin_segment(
        &self,
        _req: Request<GetAdminSegmentRequest>,
    ) -> Result<Response<AdminSegment>, Status> {
        Err(Status::unimplemented(
            "get_admin_segment is handled by the gateway",
        ))
    }

    async fn create_admin_segment(
        &self,
        _req: Request<CreateAdminSegmentRequest>,
    ) -> Result<Response<AdminSegment>, Status> {
        Err(Status::unimplemented(
            "create_admin_segment is handled by the gateway",
        ))
    }

    async fn update_admin_segment(
        &self,
        _req: Request<UpdateAdminSegmentRequest>,
    ) -> Result<Response<AdminSegment>, Status> {
        Err(Status::unimplemented(
            "update_admin_segment is handled by the gateway",
        ))
    }

    async fn delete_admin_segment(
        &self,
        _req: Request<DeleteAdminSegmentRequest>,
    ) -> Result<Response<DeleteAdminSegmentResponse>, Status> {
        Err(Status::unimplemented(
            "delete_admin_segment is handled by the gateway",
        ))
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
