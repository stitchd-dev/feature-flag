use crate::AppState;
use crate::api::{auth, event_definitions, events, experiments, flags, segments};
use axum::{
    Router,
    routing::{delete, get, post, put},
};

/// Build the API router.
pub fn build_api_router() -> Router<AppState> {
    Router::new()
        .nest("/auth", auth_routes())
        .nest("/v1/environments/{env_id}", environment_routes())
        .nest("/v1/projects/{project_id}/flags", flag_routes())
}

fn auth_routes() -> Router<AppState> {
    Router::new()
        .route("/login", post(auth::password::login))
        .route("/refresh", post(auth::password::refresh))
        .route("/logout", post(auth::password::logout))
        .route("/switch-org", post(auth::password::switch_org))
        .route(
            "/sessions",
            get(auth::sessions::list_sessions).delete(auth::sessions::revoke_all_sessions),
        )
        .route("/sessions/{token_id}", delete(auth::sessions::revoke_session))
}

fn environment_routes() -> Router<AppState> {
    Router::new()
        .nest("/segments", segment_routes())
        .route("/evaluate", post(flags::handlers::evaluate_all_flags))
        .nest("/event-definitions", event_definition_routes())
        .nest("/events", event_routes())
        .nest("/experiments", experiment_routes())
}

fn segment_routes() -> Router<AppState> {
    Router::new()
        .route(
            "/",
            get(segments::handlers::list_segments).post(segments::handlers::create_segment),
        )
        .route(
            "/{seg_id}",
            get(segments::handlers::get_segment)
                .put(segments::handlers::update_segment)
                .delete(segments::handlers::delete_segment),
        )
        .route(
            "/list-check",
            post(segments::handlers::list_check_membership),
        )
        .route(
            "/list-check/batch",
            post(segments::handlers::batch_list_check_membership),
        )
}

fn event_definition_routes() -> Router<AppState> {
    Router::new()
        .route(
            "/",
            get(event_definitions::handlers::list_event_definitions)
                .post(event_definitions::handlers::create_event_definition),
        )
        .route(
            "/{key}",
            delete(event_definitions::handlers::delete_event_definition),
        )
}

fn event_routes() -> Router<AppState> {
    Router::new()
        .route("/", post(events::handlers::ingest_single_event))
        .route("/batch", post(events::handlers::ingest_batch_events))
}

fn experiment_routes() -> Router<AppState> {
    use crate::experimentation::results_api;

    Router::new()
        .route(
            "/",
            get(experiments::handlers::list_experiments)
                .post(experiments::handlers::create_experiment),
        )
        .route(
            "/{id}",
            get(experiments::handlers::get_experiment)
                .patch(experiments::handlers::update_experiment)
                .delete(experiments::handlers::delete_experiment),
        )
        .route(
            "/{id}/transitions",
            post(experiments::handlers::transition_experiment),
        )
        .route(
            "/{id}/iterations",
            get(experiments::handlers::list_iterations),
        )
        .route("/{id}/results", get(results_api::get_latest_results))
        .route(
            "/{id}/iterations/{iter_id}/results",
            get(results_api::get_iteration_results),
        )
        .route(
            "/{id}/results/recompute",
            post(results_api::recompute_results),
        )
}

fn flag_routes() -> Router<AppState> {
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
        .route("/{flag_id}/rules", put(flags::handlers::update_rules))
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
    use std::{collections::HashMap, sync::Arc};
    use stitchd_core::{
        flag::Variant,
        id::{EnvironmentId, FlagId, ProjectId, SdkKeyId, SegmentId, VariantId},
        rule_engine::types::Rule,
        segment::{ListBasedSegment, RuleBasedSegment, Segment},
        tenant::SdkKey,
    };
    use stitchd_db::{
        ContextMembership, ExperimentRepository, FlagRepository, RepositoryError, SdkKeyRepository,
        SegmentRepository, VariantRepository,
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
            Err(RepositoryError::NotFound { id: id.to_string() })
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
            Err(RepositoryError::NotFound { id: id.to_string() })
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

        async fn revoke(&self, id: SdkKeyId) -> Result<(), RepositoryError> {
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

    struct StubExperimentRepo;

    #[async_trait]
    impl ExperimentRepository for StubExperimentRepo {
        async fn find_by_id(
            &self,
            id: stitchd_core::id::ExperimentId,
        ) -> Result<stitchd_core::experimentation::Experiment, RepositoryError> {
            Err(RepositoryError::NotFound { id: id.to_string() })
        }

        async fn list_by_environment(
            &self,
            _env_id: EnvironmentId,
            _status_filter: Option<stitchd_core::experimentation::ExperimentStatus>,
        ) -> Result<Vec<stitchd_core::experimentation::Experiment>, RepositoryError> {
            Ok(Vec::new())
        }

        async fn create(
            &self,
            _experiment: &stitchd_core::experimentation::Experiment,
        ) -> Result<(), RepositoryError> {
            Ok(())
        }

        async fn update(
            &self,
            experiment: &stitchd_core::experimentation::Experiment,
        ) -> Result<stitchd_core::experimentation::Experiment, RepositoryError> {
            Ok(experiment.clone())
        }

        async fn soft_delete(
            &self,
            id: stitchd_core::id::ExperimentId,
        ) -> Result<(), RepositoryError> {
            Err(RepositoryError::NotFound { id: id.to_string() })
        }

        async fn list_iterations(
            &self,
            _experiment_id: stitchd_core::id::ExperimentId,
        ) -> Result<Vec<stitchd_core::experimentation::ExperimentIteration>, RepositoryError>
        {
            Ok(Vec::new())
        }

        async fn apply_transition(
            &self,
            id: stitchd_core::id::ExperimentId,
            _to: stitchd_core::experimentation::ExperimentStatus,
            _actor_id: Option<stitchd_core::id::UserId>,
        ) -> Result<stitchd_core::experimentation::Experiment, RepositoryError> {
            Err(RepositoryError::NotFound { id: id.to_string() })
        }
    }

    struct StubEventDefinitionRepo;

    #[async_trait]
    impl stitchd_db::EventDefinitionRepository for StubEventDefinitionRepo {
        async fn find_by_id(
            &self,
            id: stitchd_core::id::EventDefinitionId,
        ) -> Result<stitchd_core::event::EventDefinition, RepositoryError> {
            Err(RepositoryError::NotFound { id: id.to_string() })
        }
        async fn find_by_key(
            &self,
            key: &str,
            _: EnvironmentId,
        ) -> Result<stitchd_core::event::EventDefinition, RepositoryError> {
            Err(RepositoryError::NotFound {
                id: key.to_string(),
            })
        }
        async fn list_by_environment(
            &self,
            _: EnvironmentId,
        ) -> Result<Vec<stitchd_core::event::EventDefinition>, RepositoryError> {
            Ok(Vec::new())
        }
        async fn create(
            &self,
            _: &stitchd_core::event::EventDefinition,
        ) -> Result<(), RepositoryError> {
            Ok(())
        }
        async fn update(
            &self,
            d: &stitchd_core::event::EventDefinition,
        ) -> Result<stitchd_core::event::EventDefinition, RepositoryError> {
            Ok(d.clone())
        }
        async fn soft_delete(
            &self,
            id: stitchd_core::id::EventDefinitionId,
        ) -> Result<(), RepositoryError> {
            Err(RepositoryError::NotFound { id: id.to_string() })
        }
    }

    struct StubResultsRepo;

    #[async_trait]
    impl stitchd_db::experiment_results::ExperimentResultsRepository for StubResultsRepo {
        async fn upsert(
            &self,
            _: &stitchd_db::experiment_results::UpsertResultRow,
        ) -> Result<stitchd_db::experiment_results::ExperimentResultRow, sqlx::Error> {
            Err(sqlx::Error::RowNotFound)
        }
        async fn fetch_latest(
            &self,
            _: uuid::Uuid,
        ) -> Result<Vec<stitchd_db::experiment_results::ExperimentResultRow>, sqlx::Error> {
            Ok(vec![])
        }
        async fn fetch_by_iteration(
            &self,
            _: uuid::Uuid,
            _: uuid::Uuid,
        ) -> Result<Vec<stitchd_db::experiment_results::ExperimentResultRow>, sqlx::Error> {
            Ok(vec![])
        }
        async fn is_stale(&self, _: uuid::Uuid, _: uuid::Uuid) -> Result<bool, sqlx::Error> {
            Ok(false)
        }
    }

    struct StubUserRepo;
    #[async_trait]
    impl stitchd_db::UserRepository for StubUserRepo {
        async fn find_by_id(&self, id: stitchd_core::id::UserId) -> Result<stitchd_core::auth::User, stitchd_db::RepositoryError> { Err(stitchd_db::RepositoryError::NotFound { id: id.to_string() }) }
        async fn find_by_email(&self, email: &str) -> Result<stitchd_core::auth::User, stitchd_db::RepositoryError> { Err(stitchd_db::RepositoryError::NotFound { id: email.to_string() }) }
        async fn list_by_organisation(&self, _: stitchd_core::id::OrganisationId) -> Result<Vec<stitchd_core::auth::User>, stitchd_db::RepositoryError> { Ok(vec![]) }
        async fn create(&self, _: &stitchd_core::auth::User) -> Result<(), stitchd_db::RepositoryError> { Ok(()) }
        async fn update(&self, u: &stitchd_core::auth::User) -> Result<stitchd_core::auth::User, stitchd_db::RepositoryError> { Ok(u.clone()) }
        async fn find_permissions_for_user(&self, _: stitchd_core::id::UserId, _: stitchd_core::id::ProjectId) -> Result<Vec<stitchd_core::user::Permission>, stitchd_db::RepositoryError> { Ok(vec![]) }
    }

    struct StubAuthUserRepo;
    #[async_trait]
    impl stitchd_db::AuthUserRepository for StubAuthUserRepo {
        async fn create(&self, email: &str, _: &str, _: Option<&str>) -> Result<stitchd_core::auth::User, stitchd_db::RepositoryError> { Err(stitchd_db::RepositoryError::NotFound { id: email.to_string() }) }
        async fn find_by_email(&self, _: &str) -> Result<Option<stitchd_core::auth::User>, stitchd_db::RepositoryError> { Ok(None) }
        async fn find_by_id(&self, _: stitchd_core::id::UserId) -> Result<Option<stitchd_core::auth::User>, stitchd_db::RepositoryError> { Ok(None) }
        async fn rotate_token_secret(&self, id: stitchd_core::id::UserId) -> Result<uuid::Uuid, stitchd_db::RepositoryError> { Err(stitchd_db::RepositoryError::NotFound { id: id.to_string() }) }
        async fn update_status(&self, id: stitchd_core::id::UserId, _: stitchd_core::auth::UserStatus) -> Result<(), stitchd_db::RepositoryError> { Err(stitchd_db::RepositoryError::NotFound { id: id.to_string() }) }
        async fn update_password_hash(&self, id: stitchd_core::id::UserId, _: &str) -> Result<(), stitchd_db::RepositoryError> { Err(stitchd_db::RepositoryError::NotFound { id: id.to_string() }) }
        async fn update_profile(&self, id: stitchd_core::id::UserId, _: &str, _: Option<&str>) -> Result<(), stitchd_db::RepositoryError> { Err(stitchd_db::RepositoryError::NotFound { id: id.to_string() }) }
    }

    struct StubMembershipRepo;
    #[async_trait]
    impl stitchd_db::OrgMembershipRepository for StubMembershipRepo {
        async fn add_member(&self, user_id: stitchd_core::id::UserId, _: stitchd_core::id::OrganisationId, _: stitchd_core::auth::OrgRole) -> Result<stitchd_core::auth::OrgMembership, stitchd_db::RepositoryError> { Err(stitchd_db::RepositoryError::NotFound { id: user_id.to_string() }) }
        async fn find_membership(&self, _: stitchd_core::id::UserId, _: stitchd_core::id::OrganisationId) -> Result<Option<stitchd_core::auth::OrgMembership>, stitchd_db::RepositoryError> { Ok(None) }
        async fn list_orgs_for_user(&self, _: stitchd_core::id::UserId) -> Result<Vec<stitchd_core::auth::OrgMembership>, stitchd_db::RepositoryError> { Ok(vec![]) }
        async fn remove_member(&self, _: stitchd_core::id::UserId, _: stitchd_core::id::OrganisationId) -> Result<(), stitchd_db::RepositoryError> { Ok(()) }
        async fn update_role(&self, user_id: stitchd_core::id::UserId, _: stitchd_core::id::OrganisationId, _: stitchd_core::auth::OrgRole) -> Result<(), stitchd_db::RepositoryError> { Err(stitchd_db::RepositoryError::NotFound { id: user_id.to_string() }) }
    }

    struct StubRefreshTokenRepo;
    #[async_trait]
    impl stitchd_db::RefreshTokenRepository for StubRefreshTokenRepo {
        async fn create(&self, user_id: stitchd_core::id::UserId, _: stitchd_core::id::OrganisationId, _: Option<&str>, _: i64) -> Result<(stitchd_core::auth::RefreshToken, String), stitchd_db::RepositoryError> { Err(stitchd_db::RepositoryError::NotFound { id: user_id.to_string() }) }
        async fn find_by_hash(&self, _: &str) -> Result<Option<stitchd_core::auth::RefreshToken>, stitchd_db::RepositoryError> { Ok(None) }
        async fn consume(&self, id: stitchd_core::id::RefreshTokenId) -> Result<Option<stitchd_core::auth::RefreshToken>, stitchd_db::RepositoryError> { Err(stitchd_db::RepositoryError::NotFound { id: id.to_string() }) }
        async fn revoke(&self, _: stitchd_core::id::RefreshTokenId) -> Result<(), stitchd_db::RepositoryError> { Ok(()) }
        async fn revoke_all_for_user(&self, _: stitchd_core::id::UserId) -> Result<(), stitchd_db::RepositoryError> { Ok(()) }
        async fn list_active(&self, _: stitchd_core::id::UserId) -> Result<Vec<stitchd_core::auth::RefreshToken>, stitchd_db::RepositoryError> { Ok(vec![]) }
    }

    fn make_test_state() -> AppState {
        let db =
            sqlx::PgPool::connect_lazy("postgres://stitchd:stitchd@localhost:5432/stitchd_test")
                .expect("lazy pool");
        AppState {
            db,
            metrics_handle: PrometheusBuilder::new().build_recorder().handle(),
            user_repo: Arc::new(StubUserRepo),
            auth_user_repo: Arc::new(StubAuthUserRepo),
            membership_repo: Arc::new(StubMembershipRepo),
            refresh_token_repo: Arc::new(StubRefreshTokenRepo),
            segment_repo: Arc::new(StubSegmentRepo),
            flag_repo: Arc::new(StubFlagRepo),
            variant_repo: Arc::new(StubVariantRepo),
            sdk_key_repo: Arc::new(StubSdkKeyRepo),
            event_definition_repo: Arc::new(StubEventDefinitionRepo),
            experiment_repo: Arc::new(StubExperimentRepo),
            results_repo: Arc::new(StubResultsRepo),
            ch_client: None,
            event_writer: None,
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
