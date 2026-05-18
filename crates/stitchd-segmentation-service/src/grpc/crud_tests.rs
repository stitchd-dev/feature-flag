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
        AddEntriesRequest, GetSegmentRequest, ListSegment, ListSegmentsRequest,
        LookupSegmentEntryRequest, MutateSegmentRequest, RemoveEntriesRequest, RuleSegment,
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

    // -------------------------------------------------------------------------
    // Phase 4: AddEntries RPC
    // -------------------------------------------------------------------------

    #[tokio::test]
    async fn add_entries_invalid_segment_id_returns_invalid_argument() {
        let repo = MockSegmentRepoForTest::new();
        let svc = make_service(repo);

        let result = svc
            .add_entries(Request::new(AddEntriesRequest {
                segment_id: "not-a-uuid".to_string(),
                context_type: "user".to_string(),
                list_type: "include".to_string(),
                keys: vec!["u1".to_string()],
            }))
            .await;

        assert!(result.is_err());
        assert_eq!(result.unwrap_err().code(), tonic::Code::InvalidArgument);
    }

    #[tokio::test]
    async fn add_entries_invalid_list_type_returns_invalid_argument() {
        let repo = MockSegmentRepoForTest::new();
        let svc = make_service(repo);
        let seg_id = uuid::Uuid::new_v4().to_string();

        let result = svc
            .add_entries(Request::new(AddEntriesRequest {
                segment_id: seg_id,
                context_type: "user".to_string(),
                list_type: "bad".to_string(),
                keys: vec!["u1".to_string()],
            }))
            .await;

        assert!(result.is_err());
        assert_eq!(result.unwrap_err().code(), tonic::Code::InvalidArgument);
    }

    #[tokio::test]
    async fn add_entries_valid_request_returns_added_count() {
        let repo = MockSegmentRepoForTest::new();
        let svc = make_service(repo);
        let seg_id = uuid::Uuid::new_v4().to_string();

        let resp = svc
            .add_entries(Request::new(AddEntriesRequest {
                segment_id: seg_id,
                context_type: "user".to_string(),
                list_type: "include".to_string(),
                keys: vec!["u1".to_string(), "u2".to_string()],
            }))
            .await
            .expect("add_entries should succeed");

        assert_eq!(resp.into_inner().added_count, 2);
    }

    // -------------------------------------------------------------------------
    // Phase 4: RemoveEntries RPC
    // -------------------------------------------------------------------------

    #[tokio::test]
    async fn remove_entries_invalid_segment_id_returns_invalid_argument() {
        let repo = MockSegmentRepoForTest::new();
        let svc = make_service(repo);

        let result = svc
            .remove_entries(Request::new(RemoveEntriesRequest {
                segment_id: "not-a-uuid".to_string(),
                context_type: "user".to_string(),
                list_type: "include".to_string(),
                keys: vec!["u1".to_string()],
            }))
            .await;

        assert!(result.is_err());
        assert_eq!(result.unwrap_err().code(), tonic::Code::InvalidArgument);
    }

    #[tokio::test]
    async fn remove_entries_invalid_list_type_returns_invalid_argument() {
        let repo = MockSegmentRepoForTest::new();
        let svc = make_service(repo);
        let seg_id = uuid::Uuid::new_v4().to_string();

        let result = svc
            .remove_entries(Request::new(RemoveEntriesRequest {
                segment_id: seg_id,
                context_type: "user".to_string(),
                list_type: "neither".to_string(),
                keys: vec!["u1".to_string()],
            }))
            .await;

        assert!(result.is_err());
        assert_eq!(result.unwrap_err().code(), tonic::Code::InvalidArgument);
    }

    #[tokio::test]
    async fn remove_entries_valid_request_returns_removed_count() {
        let repo = MockSegmentRepoForTest::new();
        let svc = make_service(repo);
        let seg_id = uuid::Uuid::new_v4().to_string();

        let resp = svc
            .remove_entries(Request::new(RemoveEntriesRequest {
                segment_id: seg_id,
                context_type: "user".to_string(),
                list_type: "exclude".to_string(),
                keys: vec!["u3".to_string()],
            }))
            .await
            .expect("remove_entries should succeed");

        assert_eq!(resp.into_inner().removed_count, 1);
    }

    // -------------------------------------------------------------------------
    // Phase 4: LookupSegmentEntry RPC
    // -------------------------------------------------------------------------

    #[tokio::test]
    async fn lookup_segment_entry_invalid_segment_id_returns_invalid_argument() {
        let repo = MockSegmentRepoForTest::new();
        let svc = make_service(repo);

        let result = svc
            .lookup_segment_entry(Request::new(LookupSegmentEntryRequest {
                segment_id: "bad-id".to_string(),
                key: "u1".to_string(),
                org_id: String::new(),
            }))
            .await;

        assert!(result.is_err());
        assert_eq!(result.unwrap_err().code(), tonic::Code::InvalidArgument);
    }

    #[tokio::test]
    async fn lookup_segment_entry_nonexistent_segment_returns_not_found() {
        let repo = MockSegmentRepoForTest::new();
        let svc = make_service(repo);

        let result = svc
            .lookup_segment_entry(Request::new(LookupSegmentEntryRequest {
                segment_id: uuid::Uuid::new_v4().to_string(),
                key: "u1".to_string(),
                org_id: String::new(),
            }))
            .await;

        assert!(result.is_err());
        assert_eq!(result.unwrap_err().code(), tonic::Code::NotFound);
    }

    #[tokio::test]
    async fn lookup_segment_entry_included_key_returns_in_include() {
        use std::collections::HashMap;
        use stitchd_core::segment::ContextList;

        let repo = MockSegmentRepoForTest::new();
        let (env_id, _) = make_env_id();

        let mut lists = HashMap::new();
        lists.insert(
            "user".to_string(),
            ContextList {
                include: ["u1".to_string()].iter().cloned().collect(),
                exclude: std::collections::HashSet::new(),
            },
        );
        let seg = repo.insert_list_segment(env_id, "my-list", lists);

        let svc = make_service(repo);
        let resp = svc
            .lookup_segment_entry(Request::new(LookupSegmentEntryRequest {
                segment_id: seg.id.to_string(),
                key: "u1".to_string(),
                org_id: String::new(),
            }))
            .await
            .expect("lookup should succeed");

        let inner = resp.into_inner();
        assert!(inner.in_include, "u1 should be in the include list");
    }
}
