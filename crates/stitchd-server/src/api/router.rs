use axum::{
    Router,
    routing::{get, post, put, delete},
};
use crate::AppState;
use crate::api::segments::handlers;

/// Build the API router.
pub fn build_api_router() -> Router<AppState> {
    Router::new()
        .nest("/v1/environments/:env_id/segments", 
            Router::new()
                .route("/", get(handlers::list_segments).post(handlers::create_segment))
                .route("/:seg_id", 
                    get(handlers::get_segment)
                    .put(handlers::update_segment)
                    .delete(handlers::delete_segment)
                )
        )
}
