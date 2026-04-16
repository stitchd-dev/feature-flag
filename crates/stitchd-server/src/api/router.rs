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
