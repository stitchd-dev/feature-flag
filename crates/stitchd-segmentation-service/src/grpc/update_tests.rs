//! Tests for `MutateSegment` Update path and `GetSegment` list-type segments.

#[cfg(test)]
#[allow(missing_docs)]
mod tests {
    use std::collections::{HashMap, HashSet};
    use std::sync::Arc;

    use tonic::Request;

    use stitchd_core::{id::EnvironmentId, segment::ContextList};
    use stitchd_proto::segments::v1::{
        GetSegmentRequest, ListSegment, MutateSegmentRequest, RuleSegment, SegmentMutationKind,
        mutate_segment_request, segmentation_service_server::SegmentationService,
    };

    use crate::grpc::{
        evaluation_tests::tests::MockSegmentRepoForTest,
        service::{AppState, SegmentationServiceImpl},
    };

    fn make_env_id() -> (EnvironmentId, String) {
        let id = EnvironmentId::new();
        (id, id.as_uuid().to_string())
    }

    fn make_service(repo: MockSegmentRepoForTest) -> SegmentationServiceImpl {
        SegmentationServiceImpl::new(AppState {
            segment_repo: Arc::new(repo),
        })
    }

    // -------------------------------------------------------------------------
    // MutateSegment — Update (rule-based)
    // -------------------------------------------------------------------------

    #[tokio::test]
    async fn mutate_update_rule_segment_succeeds() {
        let repo = MockSegmentRepoForTest::new();
        let (env_id, env_id_str) = make_env_id();

        repo.insert_rule_segment(env_id, "upd-seg", vec![]);

        let svc = make_service(repo);
        let resp = svc
            .mutate_segment(Request::new(MutateSegmentRequest {
                environment_id: env_id_str,
                kind: SegmentMutationKind::Update as i32,
                segment: Some(mutate_segment_request::Segment::RuleSegment(RuleSegment {
                    key: "upd-seg".to_string(),
                    context_type: "user".to_string(),
                    rule_payload: b"[]".to_vec(),
                    id: String::new(),
                })),
                version: 1,
            }))
            .await
            .expect("update should succeed");

        let inner = resp.into_inner();
        assert!(inner.segment.is_some());
    }

    #[tokio::test]
    async fn mutate_update_list_segment_succeeds() {
        let repo = MockSegmentRepoForTest::new();
        let (env_id, env_id_str) = make_env_id();

        let mut lists = HashMap::new();
        lists.insert(
            "user".to_string(),
            ContextList {
                include: std::iter::once("u1".to_string()).collect(),
                exclude: HashSet::default(),
            },
        );
        repo.insert_list_segment(env_id, "upd-list-seg", lists);

        let svc = make_service(repo);
        let resp = svc
            .mutate_segment(Request::new(MutateSegmentRequest {
                environment_id: env_id_str,
                kind: SegmentMutationKind::Update as i32,
                segment: Some(mutate_segment_request::Segment::ListSegment(ListSegment {
                    key: "upd-list-seg".to_string(),
                    context_type: "user".to_string(),
                    included_keys: vec!["u1".to_string(), "u2".to_string()],
                    excluded_keys: vec![],
                })),
                version: 1,
            }))
            .await
            .expect("update should succeed");

        let inner = resp.into_inner();
        assert!(inner.segment.is_some());
    }

    #[tokio::test]
    async fn mutate_update_nonexistent_segment_returns_not_found() {
        let repo = MockSegmentRepoForTest::new();
        let (_, env_id_str) = make_env_id();

        let svc = make_service(repo);
        let result = svc
            .mutate_segment(Request::new(MutateSegmentRequest {
                environment_id: env_id_str,
                kind: SegmentMutationKind::Update as i32,
                segment: Some(mutate_segment_request::Segment::RuleSegment(RuleSegment {
                    key: "ghost".to_string(),
                    context_type: String::new(),
                    rule_payload: b"[]".to_vec(),
                    id: String::new(),
                })),
                version: 1,
            }))
            .await;

        assert!(result.is_err());
        assert_eq!(result.unwrap_err().code(), tonic::Code::NotFound);
    }

    #[tokio::test]
    async fn mutate_update_without_segment_returns_invalid_argument() {
        let repo = MockSegmentRepoForTest::new();
        let (_, env_id_str) = make_env_id();

        let svc = make_service(repo);
        let result = svc
            .mutate_segment(Request::new(MutateSegmentRequest {
                environment_id: env_id_str,
                kind: SegmentMutationKind::Update as i32,
                segment: None,
                version: 1,
            }))
            .await;

        assert!(result.is_err());
        assert_eq!(result.unwrap_err().code(), tonic::Code::InvalidArgument);
    }

    // -------------------------------------------------------------------------
    // GetSegment — list-type segment
    // -------------------------------------------------------------------------

    /// Phase 4: GetSegment for a list-type segment returns `ListSegmentMeta` in the bundle —
    /// metadata only (key + context_type + id), no entry keys.
    /// Entry keys are never sent over the wire; membership is evaluated server-side via Scylla.
    #[tokio::test]
    async fn get_segment_list_type_returns_meta_bundle_without_keys() {
        let repo = MockSegmentRepoForTest::new();
        let (env_id, env_id_str) = make_env_id();

        let mut lists = HashMap::new();
        lists.insert(
            "user".to_string(),
            ContextList {
                include: ["u1".to_string(), "u2".to_string()]
                    .iter()
                    .cloned()
                    .collect(),
                exclude: std::iter::once("u3".to_string()).collect(),
            },
        );
        let seg = repo.insert_list_segment(env_id, "my-list-seg", lists);

        let svc = make_service(repo);
        let resp = svc
            .get_segment(Request::new(GetSegmentRequest {
                environment_id: env_id_str,
                segment_key: "my-list-seg".to_string(),
            }))
            .await
            .expect("get_segment should succeed");

        let bundle = resp.into_inner();
        assert_eq!(bundle.list_segments.len(), 1, "should have one list segment meta");
        assert_eq!(bundle.rule_segments.len(), 0);
        let meta = &bundle.list_segments[0];
        assert_eq!(meta.key, "my-list-seg");
        // id must be set to the segment UUID so the SDK can correlate ConditionExpr refs.
        assert_eq!(meta.id, seg.id.to_string());
        // Entry keys are NOT present — membership is server-side only.
    }

    // -------------------------------------------------------------------------
    // Delete without segment payload
    // -------------------------------------------------------------------------

    #[tokio::test]
    async fn mutate_delete_without_segment_returns_invalid_argument() {
        let repo = MockSegmentRepoForTest::new();
        let (_, env_id_str) = make_env_id();

        let svc = make_service(repo);
        let result = svc
            .mutate_segment(Request::new(MutateSegmentRequest {
                environment_id: env_id_str,
                kind: SegmentMutationKind::Delete as i32,
                segment: None,
                version: 1,
            }))
            .await;

        assert!(result.is_err());
        assert_eq!(result.unwrap_err().code(), tonic::Code::InvalidArgument);
    }

    // -------------------------------------------------------------------------
    // Invalid env_id for mutate
    // -------------------------------------------------------------------------

    #[tokio::test]
    async fn mutate_create_invalid_env_id_returns_invalid_argument() {
        let repo = MockSegmentRepoForTest::new();

        let svc = make_service(repo);
        let result = svc
            .mutate_segment(Request::new(MutateSegmentRequest {
                environment_id: "bad-uuid".to_string(),
                kind: SegmentMutationKind::Create as i32,
                segment: Some(mutate_segment_request::Segment::RuleSegment(RuleSegment {
                    key: "seg".to_string(),
                    context_type: String::new(),
                    rule_payload: b"[]".to_vec(),
                    id: String::new(),
                })),
                version: 0,
            }))
            .await;

        assert!(result.is_err());
        assert_eq!(result.unwrap_err().code(), tonic::Code::InvalidArgument);
    }

    // -------------------------------------------------------------------------
    // Mutate with invalid env_id for delete and update
    // -------------------------------------------------------------------------

    #[tokio::test]
    async fn mutate_delete_invalid_env_id_returns_invalid_argument() {
        let repo = MockSegmentRepoForTest::new();

        let svc = make_service(repo);
        let result = svc
            .mutate_segment(Request::new(MutateSegmentRequest {
                environment_id: "bad-uuid".to_string(),
                kind: SegmentMutationKind::Delete as i32,
                segment: Some(mutate_segment_request::Segment::RuleSegment(RuleSegment {
                    key: "seg".to_string(),
                    context_type: String::new(),
                    rule_payload: vec![],
                    id: String::new(),
                })),
                version: 1,
            }))
            .await;

        assert!(result.is_err());
        assert_eq!(result.unwrap_err().code(), tonic::Code::InvalidArgument);
    }

    #[tokio::test]
    async fn mutate_update_invalid_env_id_returns_invalid_argument() {
        let repo = MockSegmentRepoForTest::new();

        let svc = make_service(repo);
        let result = svc
            .mutate_segment(Request::new(MutateSegmentRequest {
                environment_id: "bad-uuid".to_string(),
                kind: SegmentMutationKind::Update as i32,
                segment: Some(mutate_segment_request::Segment::RuleSegment(RuleSegment {
                    key: "seg".to_string(),
                    context_type: String::new(),
                    rule_payload: b"[]".to_vec(),
                    id: String::new(),
                })),
                version: 1,
            }))
            .await;

        assert!(result.is_err());
        assert_eq!(result.unwrap_err().code(), tonic::Code::InvalidArgument);
    }

    // -------------------------------------------------------------------------
    // Invalid rule payload in create/update
    // -------------------------------------------------------------------------

    #[tokio::test]
    async fn mutate_create_invalid_rule_payload_returns_invalid_argument() {
        let repo = MockSegmentRepoForTest::new();
        let (_, env_id_str) = make_env_id();

        let svc = make_service(repo);
        let result = svc
            .mutate_segment(Request::new(MutateSegmentRequest {
                environment_id: env_id_str,
                kind: SegmentMutationKind::Create as i32,
                segment: Some(mutate_segment_request::Segment::RuleSegment(RuleSegment {
                    key: "seg".to_string(),
                    context_type: String::new(),
                    rule_payload: b"NOT VALID JSON".to_vec(),
                    id: String::new(),
                })),
                version: 0,
            }))
            .await;

        assert!(result.is_err());
        assert_eq!(result.unwrap_err().code(), tonic::Code::InvalidArgument);
    }

    #[tokio::test]
    async fn mutate_update_invalid_rule_payload_returns_invalid_argument() {
        let repo = MockSegmentRepoForTest::new();
        let (env_id, env_id_str) = make_env_id();

        repo.insert_rule_segment(env_id, "upd-seg2", vec![]);

        let svc = make_service(repo);
        let result = svc
            .mutate_segment(Request::new(MutateSegmentRequest {
                environment_id: env_id_str,
                kind: SegmentMutationKind::Update as i32,
                segment: Some(mutate_segment_request::Segment::RuleSegment(RuleSegment {
                    key: "upd-seg2".to_string(),
                    context_type: String::new(),
                    rule_payload: b"INVALID JSON".to_vec(),
                    id: String::new(),
                })),
                version: 1,
            }))
            .await;

        assert!(result.is_err());
        assert_eq!(result.unwrap_err().code(), tonic::Code::InvalidArgument);
    }

    // -------------------------------------------------------------------------
    // Invalid env_id for get_segment and list_segments
    // -------------------------------------------------------------------------

    #[tokio::test]
    async fn get_segment_invalid_env_id_returns_invalid_argument() {
        let repo = MockSegmentRepoForTest::new();
        let svc = make_service(repo);

        let result = svc
            .get_segment(Request::new(GetSegmentRequest {
                environment_id: "not-a-uuid".to_string(),
                segment_key: "seg".to_string(),
            }))
            .await;

        assert!(result.is_err());
        assert_eq!(result.unwrap_err().code(), tonic::Code::InvalidArgument);
    }

    #[tokio::test]
    async fn list_segments_invalid_env_id_returns_invalid_argument() {
        use stitchd_proto::segments::v1::{
            ListSegmentsRequest, segmentation_service_server::SegmentationService,
        };

        let repo = MockSegmentRepoForTest::new();
        let svc = make_service(repo);

        let result = svc
            .list_segments(Request::new(ListSegmentsRequest {
                environment_id: "not-a-uuid".to_string(),
            }))
            .await;

        assert!(result.is_err());
        assert_eq!(result.unwrap_err().code(), tonic::Code::InvalidArgument);
    }

    // -------------------------------------------------------------------------
    // ListSegments includes list segments
    // -------------------------------------------------------------------------

    #[tokio::test]
    async fn list_segments_includes_list_type_segments() {
        use stitchd_proto::segments::v1::{
            ListSegmentsRequest, segmentation_service_server::SegmentationService,
        };

        let repo = MockSegmentRepoForTest::new();
        let (env_id, env_id_str) = make_env_id();

        let mut lists = HashMap::new();
        lists.insert(
            "user".to_string(),
            ContextList {
                include: HashSet::default(),
                exclude: HashSet::default(),
            },
        );
        repo.insert_rule_segment(env_id, "rule-seg", vec![]);
        repo.insert_list_segment(env_id, "list-seg", lists);

        let svc = make_service(repo);
        let resp = svc
            .list_segments(Request::new(ListSegmentsRequest {
                environment_id: env_id_str,
            }))
            .await
            .expect("list_segments should succeed");

        let body = resp.into_inner();
        assert_eq!(body.rule_segments.len(), 1);
        assert_eq!(body.list_segments.len(), 1);
    }
}
