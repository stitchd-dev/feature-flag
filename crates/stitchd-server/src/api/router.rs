use crate::AppState;
use crate::api::{flags, segments};
use axum::{
    Router,
    routing::{get, post, put},
};


/// Build the API router.
pub fn build_api_router() -> Router<AppState> {
    Router::new()
        .nest(
            "/v1/environments/{env_id}",
            Router::new()
                .nest(
                    "/segments",
                    Router::new()
                        .route(
                            "/",
                            get(segments::handlers::list_segments)
                                .post(segments::handlers::create_segment),
                        )
                        .route(
                            "/{seg_id}",
                            get(segments::handlers::get_segment)
                                .put(segments::handlers::update_segment)
                                .delete(segments::handlers::delete_segment),
                        )
                        // SDK-authenticated list-check endpoints
                        .route(
                            "/list-check",
                            post(segments::handlers::list_check_membership),
                        )
                        .route(
                            "/list-check/batch",
                            post(segments::handlers::batch_list_check_membership),
                        ),
                )
                .route("/evaluate", post(flags::handlers::evaluate_all_flags)),
        )
        .nest(
            "/v1/projects/{project_id}/flags",
            Router::new()
                .route(
                    "/",
                    get(flags::handlers::list_flags).post(flags::handlers::create_flag),
                )
                .route(
                    "/{flag_id}",
                    get(flags::handlers::get_flag)
                        .put(flags::handlers::update_flag)
                        .delete(flags::handlers::delete_flag),
                )
                .route("/{flag_id}/variants", post(flags::handlers::create_variant))
                .route(
                    "/{flag_id}/hashing",
                    put(flags::handlers::update_hashing_config),
                )
                .route("/{flag_id}/rules", put(flags::handlers::update_rules)),
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use axum::{
        body::Body,
        http::{Request, StatusCode},
    };
    use metrics_exporter_prometheus::PrometheusBuilder;
    use std::{
        collections::HashMap,
        sync::Arc,
    };
    use stitchd_core::{
        flag::Variant,
        id::{EnvironmentId, FlagId, ProjectId, SegmentId, SdkKeyId, VariantId},
        rule_engine::types::Rule,
        segment::{ListBasedSegment, RuleBasedSegment, Segment},
        tenant::SdkKey,
    };
    use stitchd_db::{
        ContextMembership, FlagRepository, RepositoryError, SdkKeyRepository, SegmentRepository,
        VariantRepository,
    };
    use tower::ServiceExt as _;

    // Minimal stubs — all methods fail; only used for routing smoke tests
    struct StubFlagRepo;

    #[async_trait]
    impl FlagRepository for StubFlagRepo {
        async fn find_by_id(
            &self,
            id: FlagId,
        ) -> Result<stitchd_core::flag::FlagRecord, RepositoryError> {
            Err(RepositoryError::NotFound {
                id: id.to_string(),
            })
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
            Err(RepositoryError::NotFound {
                id: id.to_string(),
            })
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

    struct StubVariantRepo;

    #[async_trait]
    impl VariantRepository for StubVariantRepo {
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

    struct StubSegmentRepo;

    #[async_trait]
    impl SegmentRepository for StubSegmentRepo {
        async fn find_by_id(&self, id: SegmentId) -> Result<Segment, RepositoryError> {
            Err(RepositoryError::NotFound {
                id: id.to_string(),
            })
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

        async fn find_with_list(
            &self,
            id: SegmentId,
        ) -> Result<ListBasedSegment, RepositoryError> {
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

    struct StubSdkKeyRepo;

    #[async_trait]
    impl SdkKeyRepository for StubSdkKeyRepo {
        async fn find_by_id(&self, id: SdkKeyId) -> Result<SdkKey, RepositoryError> {
            Err(RepositoryError::NotFound {
                id: id.to_string(),
            })
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

        async fn revoke(&self, id: SdkKeyId) -> Result<(), RepositoryError> {
            Err(RepositoryError::NotFound {
                id: id.to_string(),
            })
        }

        async fn find_active_by_environment(
            &self,
            _environment_id: EnvironmentId,
        ) -> Result<Vec<SdkKey>, RepositoryError> {
            Ok(Vec::new())
        }

        async fn find_active_by_hash(
            &self,
            key_hash: &str,
        ) -> Result<SdkKey, RepositoryError> {
            Err(RepositoryError::NotFound {
                id: key_hash.to_string(),
            })
        }
    }

    fn make_test_state() -> AppState {
        let db =
            sqlx::PgPool::connect_lazy("postgres://stitchd:stitchd@localhost:5432/stitchd_test")
                .expect("lazy pool");
        AppState {
            db,
            metrics_handle: PrometheusBuilder::new().build_recorder().handle(),
            segment_repo: Arc::new(StubSegmentRepo),
            flag_repo: Arc::new(StubFlagRepo),
            variant_repo: Arc::new(StubVariantRepo),
            sdk_key_repo: Arc::new(StubSdkKeyRepo),
        }
    }

    // ---------------------------------------------------------------------------
    // Smoke tests: router builds correctly and routes to correct handlers
    // ---------------------------------------------------------------------------

    #[tokio::test]
    async fn unknown_route_returns_404() {
        let app = build_api_router().with_state(make_test_state());

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/v1/does-not-exist")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn flags_list_route_is_registered() {
        let project_id = ProjectId::new();
        let app = build_api_router().with_state(make_test_state());

        let response = app
            .oneshot(
                Request::builder()
                    .uri(format!("/v1/projects/{project_id}/flags"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        // 200 OK (empty list) — route exists and is reachable
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn segments_list_route_is_registered() {
        let env_id = EnvironmentId::new();
        let app = build_api_router().with_state(make_test_state());

        let response = app
            .oneshot(
                Request::builder()
                    .uri(format!("/v1/environments/{env_id}/segments"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn method_not_allowed_returns_405() {
        let project_id = ProjectId::new();
        let flag_id = stitchd_core::id::FlagId::new();
        let app = build_api_router().with_state(make_test_state());

        // PATCH is not registered for flags endpoint
        let response = app
            .oneshot(
                Request::builder()
                    .method("PATCH")
                    .uri(format!("/v1/projects/{project_id}/flags/{flag_id}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
    }
}
