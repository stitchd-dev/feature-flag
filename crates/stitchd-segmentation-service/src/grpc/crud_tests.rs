//! Tests for segment CRUD — `MutateSegment` and query handlers.
//!
//! Exercises `Create` / `Update` / `Delete` mutations and `GetSegment` / `ListSegments`
//! reads via the shared `MockSegmentRepoForTest` defined in `evaluation_tests`.

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use tonic::Request;

    use stitchd_core::id::EnvironmentId;
    use stitchd_proto::segments::v1::{
        GetSegmentRequest, ListSegment, ListSegmentsRequest, MutateSegmentRequest, RuleSegment,
        SegmentMutationKind, mutate_segment_request,
        segmentation_service_server::SegmentationService,
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

    fn empty_rule_payload() -> Vec<u8> {
        b"[]".to_vec()
    }

    // -------------------------------------------------------------------------
    // MutateSegment — Create
    // -------------------------------------------------------------------------

    #[tokio::test]
    async fn mutate_create_rule_segment_succeeds() {
        let repo = MockSegmentRepoForTest::new();
        let (_, env_id_str) = make_env_id();

        let svc = make_service(repo);
        let resp = svc
            .mutate_segment(Request::new(MutateSegmentRequest {
                environment_id: env_id_str,
                kind: SegmentMutationKind::Create as i32,
                segment: Some(mutate_segment_request::Segment::RuleSegment(RuleSegment {
                    key: "new-rule-seg".to_string(),
                    context_type: "user".to_string(),
                    rule_payload: empty_rule_payload(),
                    id: String::new(),
                })),
                version: 0,
            }))
            .await
            .expect("create should succeed");

        let inner = resp.into_inner();
        assert!(inner.segment.is_some());
        assert_eq!(inner.version, 1);
    }

    #[tokio::test]
    async fn mutate_create_list_segment_succeeds() {
        let repo = MockSegmentRepoForTest::new();
        let (_, env_id_str) = make_env_id();

        let svc = make_service(repo);
        let resp = svc
            .mutate_segment(Request::new(MutateSegmentRequest {
                environment_id: env_id_str,
                kind: SegmentMutationKind::Create as i32,
                segment: Some(mutate_segment_request::Segment::ListSegment(ListSegment {
                    key: "beta-list".to_string(),
                    context_type: "user".to_string(),
                    included_keys: vec!["u1".to_string(), "u2".to_string()],
                    excluded_keys: vec!["u3".to_string()],
                })),
                version: 0,
            }))
            .await
            .expect("create should succeed");

        let inner = resp.into_inner();
        assert!(inner.segment.is_some());
    }

    #[tokio::test]
    async fn mutate_create_without_segment_returns_invalid_argument() {
        let repo = MockSegmentRepoForTest::new();
        let (_, env_id_str) = make_env_id();

        let svc = make_service(repo);
        let result = svc
            .mutate_segment(Request::new(MutateSegmentRequest {
                environment_id: env_id_str,
                kind: SegmentMutationKind::Create as i32,
                segment: None,
                version: 0,
            }))
            .await;

        assert!(result.is_err());
        assert_eq!(result.unwrap_err().code(), tonic::Code::InvalidArgument);
    }

    #[tokio::test]
    async fn mutate_unspecified_kind_returns_invalid_argument() {
        let repo = MockSegmentRepoForTest::new();
        let (_, env_id_str) = make_env_id();

        let svc = make_service(repo);
        let result = svc
            .mutate_segment(Request::new(MutateSegmentRequest {
                environment_id: env_id_str,
                kind: SegmentMutationKind::Unspecified as i32,
                segment: Some(mutate_segment_request::Segment::RuleSegment(RuleSegment {
                    key: "seg".to_string(),
                    context_type: String::new(),
                    rule_payload: empty_rule_payload(),
                    id: String::new(),
                })),
                version: 0,
            }))
            .await;

        assert!(result.is_err());
        assert_eq!(result.unwrap_err().code(), tonic::Code::InvalidArgument);
    }

    // -------------------------------------------------------------------------
    // MutateSegment — Delete
    // -------------------------------------------------------------------------

    #[tokio::test]
    async fn mutate_delete_existing_segment_succeeds() {
        let repo = MockSegmentRepoForTest::new();
        let (env_id, env_id_str) = make_env_id();

        repo.insert_rule_segment(env_id, "to-delete", vec![]);

        let svc = make_service(repo);
        let resp = svc
            .mutate_segment(Request::new(MutateSegmentRequest {
                environment_id: env_id_str,
                kind: SegmentMutationKind::Delete as i32,
                segment: Some(mutate_segment_request::Segment::RuleSegment(RuleSegment {
                    key: "to-delete".to_string(),
                    context_type: String::new(),
                    rule_payload: vec![],
                    id: String::new(),
                })),
                version: 1,
            }))
            .await
            .expect("delete should succeed");

        // Deleted segment still returns the payload but marks as soft-deleted.
        assert!(resp.into_inner().segment.is_some());
    }

    #[tokio::test]
    async fn mutate_delete_nonexistent_segment_returns_not_found() {
        let repo = MockSegmentRepoForTest::new();
        let (_, env_id_str) = make_env_id();

        let svc = make_service(repo);
        let result = svc
            .mutate_segment(Request::new(MutateSegmentRequest {
                environment_id: env_id_str,
                kind: SegmentMutationKind::Delete as i32,
                segment: Some(mutate_segment_request::Segment::RuleSegment(RuleSegment {
                    key: "ghost".to_string(),
                    context_type: String::new(),
                    rule_payload: vec![],
                    id: String::new(),
                })),
                version: 1,
            }))
            .await;

        assert!(result.is_err());
        assert_eq!(result.unwrap_err().code(), tonic::Code::NotFound);
    }

    // -------------------------------------------------------------------------
    // GetSegment
    // -------------------------------------------------------------------------

    #[tokio::test]
    async fn get_segment_rule_type_returns_bundle() {
        let repo = MockSegmentRepoForTest::new();
        let (env_id, env_id_str) = make_env_id();

        repo.insert_rule_segment(env_id, "my-rule-seg", vec![]);

        let svc = make_service(repo);
        let resp = svc
            .get_segment(Request::new(GetSegmentRequest {
                environment_id: env_id_str,
                segment_key: "my-rule-seg".to_string(),
            }))
            .await
            .expect("get_segment should succeed");

        let bundle = resp.into_inner();
        assert_eq!(bundle.rule_segments.len(), 1);
        assert_eq!(bundle.list_segments.len(), 0);
        assert_eq!(bundle.rule_segments[0].key, "my-rule-seg");
    }

    #[tokio::test]
    async fn get_segment_missing_returns_not_found() {
        let repo = MockSegmentRepoForTest::new();
        let (_, env_id_str) = make_env_id();

        let svc = make_service(repo);
        let result = svc
            .get_segment(Request::new(GetSegmentRequest {
                environment_id: env_id_str,
                segment_key: "ghost".to_string(),
            }))
            .await;

        assert!(result.is_err());
        assert_eq!(result.unwrap_err().code(), tonic::Code::NotFound);
    }

    // -------------------------------------------------------------------------
    // ListSegments
    // -------------------------------------------------------------------------

    #[tokio::test]
    async fn list_segments_returns_all_for_environment() {
        let repo = MockSegmentRepoForTest::new();
        let (env_id, env_id_str) = make_env_id();
        let (other_env_id, _) = make_env_id();

        repo.insert_rule_segment(env_id, "seg-a", vec![]);
        repo.insert_rule_segment(env_id, "seg-b", vec![]);
        repo.insert_rule_segment(other_env_id, "seg-other", vec![]);

        let svc = make_service(repo);
        let resp = svc
            .list_segments(Request::new(ListSegmentsRequest {
                environment_id: env_id_str,
            }))
            .await
            .expect("list_segments should succeed");

        let body = resp.into_inner();
        assert_eq!(body.rule_segments.len(), 2);
        let keys: Vec<_> = body.rule_segments.iter().map(|s| s.key.as_str()).collect();
        assert!(keys.contains(&"seg-a"));
        assert!(keys.contains(&"seg-b"));
    }

    #[tokio::test]
    async fn list_segments_empty_environment_returns_empty() {
        let repo = MockSegmentRepoForTest::new();
        let (_, env_id_str) = make_env_id();

        let svc = make_service(repo);
        let resp = svc
            .list_segments(Request::new(ListSegmentsRequest {
                environment_id: env_id_str,
            }))
            .await
            .expect("list_segments should succeed");

        let body = resp.into_inner();
        assert!(body.rule_segments.is_empty());
        assert!(body.list_segments.is_empty());
    }
}
