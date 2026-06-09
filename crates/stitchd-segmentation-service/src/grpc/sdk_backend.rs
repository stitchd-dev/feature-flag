//! Backend gRPC implementation of [`SegmentationSdkBackendService`].
//!
//! Called by the gateway after SDK-key validation succeeds. Trusts the
//! gateway-supplied `x-env-id` metadata; does NOT re-validate the SDK key.
//! See `sdks/spec/proto/sdk/v1/backend.proto`.

use std::sync::Arc;

use tonic::{Request, Response, Status};

use stitchd_core::id::{EnvironmentId, SegmentId};
use stitchd_db::SegmentRepository;
use stitchd_proto::sdk::v1::{
    BatchCheckListMembershipRequest, BatchCheckListMembershipResponse, MembershipResult,
    segmentation_sdk_backend_service_server::SegmentationSdkBackendService,
};

const ENV_ID_METADATA_KEY: &str = "x-env-id";

/// Backend gRPC service hosted on `stitchd-segmentation-service`.
pub struct SegmentationSdkBackendServiceImpl {
    segment_repo: Arc<dyn SegmentRepository>,
}

impl SegmentationSdkBackendServiceImpl {
    /// Construct a new service backed by the given segment repository.
    #[must_use]
    pub const fn new(segment_repo: Arc<dyn SegmentRepository>) -> Self {
        Self { segment_repo }
    }
}

/// Extract the resolved environment id from `x-env-id` gRPC metadata. Mirrors
/// the helper in `stitchd-flag-service::sdk_backend` (same contract).
#[allow(clippy::result_large_err)]
fn env_id_from_metadata(req: &Request<impl Sized>) -> Result<EnvironmentId, Status> {
    let raw = req
        .metadata()
        .get(ENV_ID_METADATA_KEY)
        .ok_or_else(|| Status::unauthenticated(format!("missing {ENV_ID_METADATA_KEY} metadata")))?
        .to_str()
        .map_err(|_| {
            Status::unauthenticated(format!("{ENV_ID_METADATA_KEY} is not valid UTF-8"))
        })?;
    let uuid = uuid::Uuid::parse_str(raw).map_err(|_| {
        Status::unauthenticated(format!("{ENV_ID_METADATA_KEY} is not a valid UUID"))
    })?;
    Ok(EnvironmentId::from_uuid(uuid))
}

#[tonic::async_trait]
impl SegmentationSdkBackendService for SegmentationSdkBackendServiceImpl {
    async fn batch_check_list_membership(
        &self,
        request: Request<BatchCheckListMembershipRequest>,
    ) -> Result<Response<BatchCheckListMembershipResponse>, Status> {
        let env_id = env_id_from_metadata(&request)?;
        let queries = request.into_inner().queries;

        if queries.is_empty() {
            return Ok(Response::new(BatchCheckListMembershipResponse {
                results: vec![],
            }));
        }

        // Per the proto, each query carries its own segment_ids list. The
        // typical SDK case has the SAME segment_ids across all queries (the
        // flag-referenced filter set). We optimise for that by:
        //   1. Computing the UNION of all requested segment_ids (deduped)
        //   2. Running ONE SQL query covering all (context, segment) pairs
        //   3. Per-query filtering: include only segment_ids the caller asked for
        let contexts: Vec<(String, String)> = queries
            .iter()
            .map(|q| (q.context_type.clone(), q.context_key.clone()))
            .collect();

        let mut all_segment_uuids: std::collections::HashSet<uuid::Uuid> =
            std::collections::HashSet::new();
        for q in &queries {
            for sid in &q.segment_ids {
                if let Ok(uuid) = uuid::Uuid::parse_str(sid) {
                    all_segment_uuids.insert(uuid);
                } else {
                    return Err(Status::invalid_argument(format!(
                        "segment_id is not a valid UUID: {sid:?}"
                    )));
                }
            }
        }
        let segment_ids: Vec<SegmentId> = all_segment_uuids
            .iter()
            .copied()
            .map(SegmentId::from_uuid)
            .collect();

        let id_results = self
            .segment_repo
            .find_memberships_batch(env_id, &contexts, &segment_ids)
            .await
            .map_err(|e| Status::internal(format!("find_memberships_batch failed: {e}")))?;

        // Build a lookup: (context_type, context_key) → id-keyed membership map.
        let mut by_ctx: std::collections::HashMap<
            (String, String),
            &std::collections::HashMap<SegmentId, bool>,
        > = std::collections::HashMap::with_capacity(id_results.len());
        for r in &id_results {
            by_ctx.insert(
                (r.context_type.clone(), r.context_key.clone()),
                &r.memberships,
            );
        }

        // Build the response in the SAME ORDER as the request's queries, and
        // filter each query's memberships to ONLY the segment_ids it asked for.
        let results: Vec<MembershipResult> = queries
            .into_iter()
            .map(|q| {
                let key = (q.context_type.clone(), q.context_key.clone());
                let mut memberships = std::collections::HashMap::with_capacity(q.segment_ids.len());
                if let Some(all_for_ctx) = by_ctx.get(&key) {
                    for sid_str in &q.segment_ids {
                        if let Ok(uuid) = uuid::Uuid::parse_str(sid_str) {
                            let sid = SegmentId::from_uuid(uuid);
                            let is_member = all_for_ctx.get(&sid).copied().unwrap_or(false);
                            memberships.insert(sid_str.clone(), is_member);
                        }
                    }
                } else {
                    // No row for this context in the SQL result → all false.
                    for sid_str in &q.segment_ids {
                        memberships.insert(sid_str.clone(), false);
                    }
                }
                MembershipResult {
                    context_type: q.context_type,
                    context_key: q.context_key,
                    memberships,
                }
            })
            .collect();

        Ok(Response::new(BatchCheckListMembershipResponse { results }))
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use std::collections::HashMap;
    use std::sync::Mutex;

    use stitchd_core::segment::{RuleBasedSegment, Segment};
    use stitchd_db::{ContextMembership, RepositoryError, SegmentIdMembership};
    use stitchd_proto::sdk::v1::MembershipQuery;

    /// Arguments captured from `find_memberships_batch` for assertion.
    type LastCallArgs = Option<(EnvironmentId, Vec<(String, String)>, Vec<SegmentId>)>;

    /// Stub that records calls + lets tests pre-program a fixed membership matrix.
    #[derive(Default)]
    struct StubSegmentRepo {
        /// Programmed memberships: (`env_id`, `context_type`, `context_key`, `segment_id`) → `is_member`
        membership_matrix: Mutex<HashMap<(EnvironmentId, String, String, SegmentId), bool>>,
        /// Capture the last `find_memberships_batch` arguments for assertion.
        last_call: Mutex<LastCallArgs>,
    }
    impl StubSegmentRepo {
        fn arc() -> Arc<Self> {
            Arc::new(Self::default())
        }
        fn set_member(
            self: &Arc<Self>,
            env: EnvironmentId,
            ctx_type: &str,
            ctx_key: &str,
            seg_id: SegmentId,
            is_member: bool,
        ) {
            self.membership_matrix
                .lock()
                .unwrap()
                .insert((env, ctx_type.into(), ctx_key.into(), seg_id), is_member);
        }
    }

    #[async_trait]
    impl SegmentRepository for StubSegmentRepo {
        async fn find_by_id(&self, _id: SegmentId) -> Result<Segment, RepositoryError> {
            unimplemented!()
        }
        async fn find_by_key(
            &self,
            _key: &str,
            _env: EnvironmentId,
        ) -> Result<Segment, RepositoryError> {
            unimplemented!()
        }
        async fn list_by_environment(
            &self,
            _env: EnvironmentId,
        ) -> Result<Vec<Segment>, RepositoryError> {
            unimplemented!()
        }
        async fn list_by_environment_keyset(
            &self,
            _env: EnvironmentId,
            _after: Option<stitchd_db::KeysetCursor>,
            _limit: u64,
        ) -> Result<(Vec<Segment>, Option<String>), RepositoryError> {
            unimplemented!()
        }
        async fn create(&self, _s: &Segment) -> Result<(), RepositoryError> {
            unimplemented!()
        }
        async fn update(&self, _s: &Segment) -> Result<Segment, RepositoryError> {
            unimplemented!()
        }
        async fn find_with_rules(
            &self,
            _id: SegmentId,
        ) -> Result<RuleBasedSegment, RepositoryError> {
            unimplemented!()
        }
        async fn set_list_entries(
            &self,
            _id: SegmentId,
            _ctx_type: &str,
            _include: &[String],
            _exclude: &[String],
        ) -> Result<(), RepositoryError> {
            unimplemented!()
        }
        async fn get_condition_expr(
            &self,
            _id: SegmentId,
        ) -> Result<Option<serde_json::Value>, RepositoryError> {
            unimplemented!()
        }
        async fn set_condition_expr(
            &self,
            _id: SegmentId,
            _expr: Option<&serde_json::Value>,
        ) -> Result<(), RepositoryError> {
            unimplemented!()
        }
        async fn soft_delete(&self, _id: SegmentId) -> Result<(), RepositoryError> {
            unimplemented!()
        }
        async fn check_list_membership(
            &self,
            _env: EnvironmentId,
            _ct: &str,
            _ck: &str,
            _keys: &[String],
        ) -> Result<HashMap<String, bool>, RepositoryError> {
            unimplemented!()
        }
        async fn lookup_entry_raw(
            &self,
            _id: SegmentId,
            _context_type: &str,
            _key: &str,
        ) -> Result<(bool, bool), RepositoryError> {
            Ok((false, false))
        }
        async fn batch_check_list_membership(
            &self,
            _env: EnvironmentId,
            _contexts: &[(String, String)],
            _keys: &[String],
        ) -> Result<Vec<ContextMembership>, RepositoryError> {
            unimplemented!()
        }
        async fn find_batch_by_ids(
            &self,
            _ids: &[SegmentId],
        ) -> Result<Vec<Segment>, RepositoryError> {
            unimplemented!()
        }
        async fn find_rules_batch(
            &self,
            _ids: &[SegmentId],
        ) -> Result<HashMap<SegmentId, RuleBasedSegment>, RepositoryError> {
            unimplemented!()
        }
        async fn add_entries(
            &self,
            _id: SegmentId,
            _ctx: &str,
            _lt: &str,
            _keys: &[String],
        ) -> Result<(), RepositoryError> {
            Ok(())
        }
        async fn remove_entries(
            &self,
            _id: SegmentId,
            _ctx: &str,
            _lt: &str,
            _keys: &[String],
        ) -> Result<(), RepositoryError> {
            Ok(())
        }
        async fn get_list_segment_summary(
            &self,
            _id: SegmentId,
        ) -> Result<stitchd_db::ListSegmentSummary, RepositoryError> {
            Ok(stitchd_db::ListSegmentSummary::default())
        }
        async fn find_memberships_batch(
            &self,
            env_id: EnvironmentId,
            contexts: &[(String, String)],
            segment_ids: &[SegmentId],
        ) -> Result<Vec<SegmentIdMembership>, RepositoryError> {
            *self.last_call.lock().unwrap() =
                Some((env_id, contexts.to_vec(), segment_ids.to_vec()));
            let matrix = self.membership_matrix.lock().unwrap();
            Ok(contexts
                .iter()
                .map(|(t, k)| {
                    let memberships = segment_ids
                        .iter()
                        .map(|id| {
                            let v = matrix
                                .get(&(env_id, t.clone(), k.clone(), *id))
                                .copied()
                                .unwrap_or(false);
                            (*id, v)
                        })
                        .collect();
                    SegmentIdMembership {
                        context_type: t.clone(),
                        context_key: k.clone(),
                        memberships,
                    }
                })
                .collect())
        }
    }

    fn make_request_with_env(
        env_id: EnvironmentId,
        queries: Vec<MembershipQuery>,
    ) -> Request<BatchCheckListMembershipRequest> {
        let mut req = Request::new(BatchCheckListMembershipRequest { queries });
        req.metadata_mut()
            .insert("x-env-id", env_id.to_string().parse().unwrap());
        req
    }

    // ── Tests ───────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn rejects_missing_env_id_metadata() {
        let svc = SegmentationSdkBackendServiceImpl::new(StubSegmentRepo::arc());
        let req = Request::new(BatchCheckListMembershipRequest { queries: vec![] });
        let status = svc.batch_check_list_membership(req).await.unwrap_err();
        assert_eq!(status.code(), tonic::Code::Unauthenticated);
    }

    #[tokio::test]
    async fn empty_queries_returns_empty_results() {
        let env = EnvironmentId::new();
        let svc = SegmentationSdkBackendServiceImpl::new(StubSegmentRepo::arc());
        let resp = svc
            .batch_check_list_membership(make_request_with_env(env, vec![]))
            .await
            .unwrap()
            .into_inner();
        assert_eq!(resp.results.len(), 0);
    }

    #[tokio::test]
    async fn rejects_invalid_segment_uuid() {
        let env = EnvironmentId::new();
        let svc = SegmentationSdkBackendServiceImpl::new(StubSegmentRepo::arc());
        let req = make_request_with_env(
            env,
            vec![MembershipQuery {
                context_type: "user".into(),
                context_key: "alice".into(),
                segment_ids: vec!["not-a-uuid".into()],
            }],
        );
        let status = svc.batch_check_list_membership(req).await.unwrap_err();
        assert_eq!(status.code(), tonic::Code::InvalidArgument);
    }

    #[tokio::test]
    async fn happy_path_5_queries_correct_matrix() {
        let env = EnvironmentId::new();
        let repo = StubSegmentRepo::arc();

        // Two segments — beta = "is in beta"; admin = "is admin"
        let beta = SegmentId::new();
        let admin = SegmentId::new();

        // Pre-program the matrix:
        //   alice → beta=true, admin=false
        //   bob   → beta=false, admin=false
        //   carol → beta=true,  admin=true
        //   acme  → beta=false, admin=false  (org context)
        //   widgets → beta=true (programmed only for beta) (org context)
        repo.set_member(env, "user", "alice", beta, true);
        repo.set_member(env, "user", "carol", beta, true);
        repo.set_member(env, "user", "carol", admin, true);
        repo.set_member(env, "org", "widgets", beta, true);

        let svc = SegmentationSdkBackendServiceImpl::new(repo.clone());

        let queries = vec![
            MembershipQuery {
                context_type: "user".into(),
                context_key: "alice".into(),
                segment_ids: vec![beta.to_string(), admin.to_string()],
            },
            MembershipQuery {
                context_type: "user".into(),
                context_key: "bob".into(),
                segment_ids: vec![beta.to_string(), admin.to_string()],
            },
            MembershipQuery {
                context_type: "user".into(),
                context_key: "carol".into(),
                segment_ids: vec![beta.to_string(), admin.to_string()],
            },
            MembershipQuery {
                context_type: "org".into(),
                context_key: "acme".into(),
                segment_ids: vec![beta.to_string()],
            },
            MembershipQuery {
                context_type: "org".into(),
                context_key: "widgets".into(),
                segment_ids: vec![beta.to_string()],
            },
        ];

        let resp = svc
            .batch_check_list_membership(make_request_with_env(env, queries))
            .await
            .unwrap()
            .into_inner();

        // Same order as request:
        assert_eq!(resp.results.len(), 5);
        assert_eq!(resp.results[0].context_key, "alice");
        assert!(resp.results[0].memberships[&beta.to_string()]);
        assert!(!resp.results[0].memberships[&admin.to_string()]);

        assert_eq!(resp.results[1].context_key, "bob");
        assert!(!resp.results[1].memberships[&beta.to_string()]);
        assert!(!resp.results[1].memberships[&admin.to_string()]);

        assert_eq!(resp.results[2].context_key, "carol");
        assert!(resp.results[2].memberships[&beta.to_string()]);
        assert!(resp.results[2].memberships[&admin.to_string()]);

        assert_eq!(resp.results[3].context_type, "org");
        assert_eq!(resp.results[3].context_key, "acme");
        assert!(!resp.results[3].memberships[&beta.to_string()]);

        assert_eq!(resp.results[4].context_key, "widgets");
        assert!(resp.results[4].memberships[&beta.to_string()]);
    }

    #[tokio::test]
    async fn deduplicates_segment_ids_in_repo_call() {
        // Even with 5 queries each carrying segment_ids: [beta, admin],
        // the underlying repo call should receive the deduped set (2 ids),
        // not 10. Verifies the union-then-filter strategy.
        let env = EnvironmentId::new();
        let beta = SegmentId::new();
        let admin = SegmentId::new();
        let repo = StubSegmentRepo::arc();
        let svc = SegmentationSdkBackendServiceImpl::new(repo.clone());

        let queries = (0..5)
            .map(|i| MembershipQuery {
                context_type: "user".into(),
                context_key: format!("user-{i}"),
                segment_ids: vec![beta.to_string(), admin.to_string()],
            })
            .collect();

        let _ = svc
            .batch_check_list_membership(make_request_with_env(env, queries))
            .await
            .unwrap();

        let captured = repo.last_call.lock().unwrap().clone().unwrap();
        let (called_env, called_contexts, called_segment_ids) = captured;
        assert_eq!(called_env, env);
        assert_eq!(called_contexts.len(), 5); // 5 distinct contexts
        assert_eq!(called_segment_ids.len(), 2); // 2 deduped segments
        assert!(called_segment_ids.contains(&beta));
        assert!(called_segment_ids.contains(&admin));
    }

    #[tokio::test]
    async fn per_query_segment_filter_respects_query_scope() {
        // Query A asks for [beta, admin]; query B asks for [beta] only.
        // Response[B].memberships must contain ONLY beta, not admin.
        let env = EnvironmentId::new();
        let beta = SegmentId::new();
        let admin = SegmentId::new();
        let repo = StubSegmentRepo::arc();
        repo.set_member(env, "user", "alice", beta, true);
        repo.set_member(env, "user", "alice", admin, true);
        repo.set_member(env, "user", "bob", beta, true);
        repo.set_member(env, "user", "bob", admin, true);

        let svc = SegmentationSdkBackendServiceImpl::new(repo);

        let queries = vec![
            MembershipQuery {
                context_type: "user".into(),
                context_key: "alice".into(),
                segment_ids: vec![beta.to_string(), admin.to_string()],
            },
            MembershipQuery {
                context_type: "user".into(),
                context_key: "bob".into(),
                segment_ids: vec![beta.to_string()],
            },
        ];

        let resp = svc
            .batch_check_list_membership(make_request_with_env(env, queries))
            .await
            .unwrap()
            .into_inner();

        assert_eq!(resp.results[0].memberships.len(), 2);
        assert_eq!(resp.results[1].memberships.len(), 1);
        assert!(resp.results[1].memberships.contains_key(&beta.to_string()));
        assert!(!resp.results[1].memberships.contains_key(&admin.to_string()));
    }
}
