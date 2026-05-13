//! Tests for list-based segment membership evaluation via `EvaluateMembership`.

#[cfg(test)]
#[allow(missing_docs)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Arc;

    use tonic::Request;

    use stitchd_core::{id::EnvironmentId, segment::ContextList};
    use stitchd_proto::segments::v1::{
        EvaluateMembershipRequest, segmentation_service_server::SegmentationService,
    };

    use crate::grpc::{
        evaluation_tests::tests::MockSegmentRepoForTest,
        service::{AppState, SegmentationServiceImpl},
    };

    fn make_service(repo: MockSegmentRepoForTest) -> SegmentationServiceImpl {
        SegmentationServiceImpl::new(AppState {
            segment_repo: Arc::new(repo),
        })
    }

    fn make_env_id() -> (EnvironmentId, String) {
        let id = EnvironmentId::new();
        (id, id.as_uuid().to_string())
    }

    fn user_lists(include: &[&str], exclude: &[&str]) -> HashMap<String, ContextList> {
        let mut lists = HashMap::new();
        lists.insert(
            "user".to_string(),
            ContextList {
                include: include
                    .iter()
                    .map(std::string::ToString::to_string)
                    .collect(),
                exclude: exclude
                    .iter()
                    .map(std::string::ToString::to_string)
                    .collect(),
            },
        );
        lists
    }

    // -------------------------------------------------------------------------
    // List-based evaluation: include list
    // -------------------------------------------------------------------------

    /// A key that appears in the include list is a member.
    #[tokio::test]
    async fn list_based_included_key_is_member() {
        let repo = MockSegmentRepoForTest::new();
        let (env_id, env_id_str) = make_env_id();

        let lists = user_lists(&["user-123", "user-456"], &[]);
        repo.insert_list_segment(env_id, "include-seg", lists);

        let svc = make_service(repo);
        let resp = svc
            .evaluate_membership(Request::new(EvaluateMembershipRequest {
                environment_id: env_id_str,
                segment_key: "include-seg".to_string(),
                context_key: "user-123".to_string(),
                context_type: "user".to_string(),
            }))
            .await
            .expect("should not error");

        assert!(resp.into_inner().is_member);
    }

    /// A key NOT in the include list is not a member.
    #[tokio::test]
    async fn list_based_unknown_key_is_not_member() {
        let repo = MockSegmentRepoForTest::new();
        let (env_id, env_id_str) = make_env_id();

        let lists = user_lists(&["user-123"], &[]);
        repo.insert_list_segment(env_id, "include-seg", lists);

        let svc = make_service(repo);
        let resp = svc
            .evaluate_membership(Request::new(EvaluateMembershipRequest {
                environment_id: env_id_str,
                segment_key: "include-seg".to_string(),
                context_key: "user-999".to_string(),
                context_type: "user".to_string(),
            }))
            .await
            .expect("should not error");

        assert!(!resp.into_inner().is_member);
    }

    // -------------------------------------------------------------------------
    // List-based evaluation: exclude list takes precedence
    // -------------------------------------------------------------------------

    /// A key on the exclude list is NOT a member, even if it is also on the include list.
    #[tokio::test]
    async fn list_based_excluded_key_is_not_member_even_if_included() {
        let repo = MockSegmentRepoForTest::new();
        let (env_id, env_id_str) = make_env_id();

        // user-123 is in BOTH include and exclude — exclude wins.
        let lists = user_lists(&["user-123"], &["user-123"]);
        repo.insert_list_segment(env_id, "mixed-seg", lists);

        let svc = make_service(repo);
        let resp = svc
            .evaluate_membership(Request::new(EvaluateMembershipRequest {
                environment_id: env_id_str,
                segment_key: "mixed-seg".to_string(),
                context_key: "user-123".to_string(),
                context_type: "user".to_string(),
            }))
            .await
            .expect("should not error");

        assert!(
            !resp.into_inner().is_member,
            "exclude must win over include"
        );
    }

    // -------------------------------------------------------------------------
    // List-based evaluation: DB-level membership check
    // -------------------------------------------------------------------------

    /// `check_list_membership` returns `true` → `is_member` is `true`.
    #[tokio::test]
    async fn list_based_db_membership_true_returns_member() {
        let repo = MockSegmentRepoForTest::new();
        let (env_id, env_id_str) = make_env_id();

        // Insert segment record only (no in-memory list; rely on DB mock).
        let seg_id = stitchd_core::id::SegmentId::new();
        {
            let seg = stitchd_core::segment::Segment {
                id: seg_id,
                environment_id: env_id,
                key: "db-seg".to_string(),
                name: String::new(),
                description: String::new(),
                tags: vec![],
                segment_type: stitchd_core::segment::SegmentType::List,
                created_at: chrono::Utc::now(),
                updated_at: chrono::Utc::now(),
                deleted_at: None,
                version: 1,
            };
            repo.segments
                .lock()
                .unwrap()
                .insert((env_id, "db-seg".to_string()), seg);
        }

        // Seed the DB-level membership result.
        repo.list_memberships.lock().unwrap().insert(
            (
                env_id,
                "db-seg".to_string(),
                "user".to_string(),
                "user-db".to_string(),
            ),
            true,
        );

        let svc = make_service(repo);
        let resp = svc
            .evaluate_membership(Request::new(EvaluateMembershipRequest {
                environment_id: env_id_str,
                segment_key: "db-seg".to_string(),
                context_key: "user-db".to_string(),
                context_type: "user".to_string(),
            }))
            .await
            .expect("should not error");

        assert!(resp.into_inner().is_member);
    }

    /// When `check_list_membership` has no entry for the key, defaults to `false`.
    #[tokio::test]
    async fn list_based_db_membership_absent_returns_not_member() {
        let repo = MockSegmentRepoForTest::new();
        let (env_id, env_id_str) = make_env_id();

        let seg_id = stitchd_core::id::SegmentId::new();
        {
            let seg = stitchd_core::segment::Segment {
                id: seg_id,
                environment_id: env_id,
                key: "db-seg2".to_string(),
                name: String::new(),
                description: String::new(),
                tags: vec![],
                segment_type: stitchd_core::segment::SegmentType::List,
                created_at: chrono::Utc::now(),
                updated_at: chrono::Utc::now(),
                deleted_at: None,
                version: 1,
            };
            repo.segments
                .lock()
                .unwrap()
                .insert((env_id, "db-seg2".to_string()), seg);
        }

        // No membership entry seeded.
        let svc = make_service(repo);
        let resp = svc
            .evaluate_membership(Request::new(EvaluateMembershipRequest {
                environment_id: env_id_str,
                segment_key: "db-seg2".to_string(),
                context_key: "user-absent".to_string(),
                context_type: "user".to_string(),
            }))
            .await
            .expect("should not error");

        assert!(!resp.into_inner().is_member);
    }

    // -------------------------------------------------------------------------
    // Error paths
    // -------------------------------------------------------------------------

    /// Requesting a list-based segment that does not exist returns `NOT_FOUND`.
    #[tokio::test]
    async fn list_based_missing_segment_returns_not_found() {
        let repo = MockSegmentRepoForTest::new();
        let (_, env_id_str) = make_env_id();

        let svc = make_service(repo);
        let result = svc
            .evaluate_membership(Request::new(EvaluateMembershipRequest {
                environment_id: env_id_str,
                segment_key: "nonexistent-list".to_string(),
                context_key: "user-1".to_string(),
                context_type: "user".to_string(),
            }))
            .await;

        assert!(result.is_err());
        assert_eq!(result.unwrap_err().code(), tonic::Code::NotFound);
    }
}
