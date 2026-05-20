//! Per-env event-quota middleware: gates `POST /v1/events/track` with a
//! token-bucket rate limit keyed on `environment_id`.
//!
//! ### Why
//! A misbehaving SDK key — runaway loop, retry storm, or a customer test
//! shipping a bad version to production — must NOT be able to overwhelm the
//! gateway → analytics path. We cap traffic per-environment (the smallest
//! billable unit) so noisy neighbours do not starve other tenants.
//!
//! ### Architecture
//! - In-memory only, per gateway pod. Each pod tracks its own counts; there
//!   is no Redis / shared store. Horizontal scale-out simply multiplies the
//!   effective ceiling by the pod count — acceptable for v1 because the
//!   downstream analytics ingest is itself horizontally scaled.
//! - Backed by [`governor::DefaultKeyedRateLimiter`] with the `DashMap`
//!   state store for lock-free per-key access under concurrent load.
//! - Token-bucket semantics via GCRA: bursts up to the per-second limit are
//!   absorbed, sustained rates above it are 429'd until tokens refill.
//!
//! ### Limit
//! Default: 1000 events/sec/env_id. Configurable via the
//! `STITCHD_EVENT_QUOTA_PER_SEC` environment variable (any positive
//! `u32`).
//!
//! ### Behaviour on missing context
//! If the upstream `sdk_auth_middleware` did not run (route misconfiguration),
//! [`SdkContext`] is absent from request extensions. We MUST NOT panic — the
//! quota layer falls through to the next layer, letting the route handler
//! return its own 500/misconfiguration error. This keeps the failure mode
//! observable rather than crashing the pod.

use std::num::NonZeroU32;
use std::sync::Arc;

use axum::{
    extract::Request,
    http::StatusCode,
    middleware::Next,
    response::{IntoResponse, Response},
};
use governor::{DefaultKeyedRateLimiter, Quota};

use crate::middleware::sdk_auth::SdkContext;

/// Environment variable name controlling the per-env quota.
pub const QUOTA_ENV_VAR: &str = "STITCHD_EVENT_QUOTA_PER_SEC";

/// Default events-per-second cap per environment, applied when
/// [`QUOTA_ENV_VAR`] is unset or unparseable.
pub const DEFAULT_QUOTA_PER_SEC: u32 = 1000;

/// Per-env keyed rate limiter. `String` keys are env UUIDs as serialised by
/// [`SdkContext::environment_id`].
///
/// `DefaultKeyedRateLimiter` resolves to the [`DashMap`]-backed state store
/// when governor is built with the `dashmap` feature (see workspace
/// `Cargo.toml`), which gives lock-free concurrent access on the hot path.
///
/// [`DashMap`]: dashmap::DashMap
pub type EnvKeyedRateLimiter = DefaultKeyedRateLimiter<String>;

/// Read the per-env quota from the environment, falling back to
/// [`DEFAULT_QUOTA_PER_SEC`].
///
/// A non-positive or unparseable value is also treated as "use the default"
/// — quota=0 would brick every SDK and is almost certainly a config typo.
#[must_use]
pub fn quota_per_sec_from_env() -> NonZeroU32 {
    let parsed = std::env::var(QUOTA_ENV_VAR)
        .ok()
        .and_then(|v| v.parse::<u32>().ok())
        .and_then(NonZeroU32::new);
    parsed.unwrap_or_else(|| {
        NonZeroU32::new(DEFAULT_QUOTA_PER_SEC).expect("DEFAULT_QUOTA_PER_SEC is non-zero")
    })
}

/// Build a fresh per-env rate limiter at the given per-second cap.
#[must_use]
pub fn build_limiter(per_sec: NonZeroU32) -> Arc<EnvKeyedRateLimiter> {
    let quota = Quota::per_second(per_sec);
    // `RateLimiter::keyed` uses the crate's default keyed state store,
    // which resolves to `DashMapStateStore` when the `dashmap` feature is
    // on (workspace Cargo.toml). Picking `keyed` over the explicit
    // `dashmap` constructor keeps the type aligned with
    // `DefaultKeyedRateLimiter<K>` so the public alias above remains the
    // single source of truth.
    Arc::new(governor::RateLimiter::keyed(quota))
}

/// Build a per-env rate limiter using the env-var-configured cap.
#[must_use]
pub fn build_limiter_from_env() -> Arc<EnvKeyedRateLimiter> {
    build_limiter(quota_per_sec_from_env())
}

/// 429 response with a machine-readable error code. Matches the shape of
/// `sdk_auth`'s 401 payload so SDK clients can handle them uniformly.
fn too_many_requests() -> Response {
    (
        StatusCode::TOO_MANY_REQUESTS,
        axum::Json(serde_json::json!({
            "error": "event_quota_exceeded",
            "message": "per-environment event rate quota exceeded; retry after a short backoff"
        })),
    )
        .into_response()
}

/// Axum middleware enforcing the per-env quota on the `POST /v1/events/track`
/// route.
///
/// Behaviour:
/// - If [`SdkContext`] is present in extensions and the env's quota is
///   exhausted → return `429 Too Many Requests`.
/// - If [`SdkContext`] is present and quota has room → pass through.
/// - If [`SdkContext`] is **absent** (upstream `sdk_auth_middleware` did not
///   run, or the route is misconfigured) → fall through to the next layer
///   without rate-limiting. The downstream `sdk_auth_middleware` returns
///   401 in that case; if it isn't applied either, the handler itself
///   surfaces a 500. Either way: do not panic.
pub async fn event_quota_middleware(
    axum::extract::State(limiter): axum::extract::State<Arc<EnvKeyedRateLimiter>>,
    req: Request,
    next: Next,
) -> Response {
    if let Some(ctx) = req.extensions().get::<SdkContext>()
        && limiter.check_key(&ctx.environment_id).is_err()
    {
        tracing::debug!(
            env_id = %ctx.environment_id,
            "event quota exceeded — returning 429"
        );
        return too_many_requests();
    }
    // No SdkContext (or quota has room) → forward to the next layer. We
    // intentionally do NOT return 401 here; that's `sdk_auth_middleware`'s
    // job, and faking it from the quota layer would mask a misconfiguration.
    next.run(req).await
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{Router, body::Body, middleware::from_fn_with_state, routing::post};
    use http::Request as HttpRequest;
    use tower::ServiceExt;

    /// Build a tiny axum app that mounts an `SdkContext`-injecting layer
    /// (if `env_id` is `Some`), then the quota layer, then a permissive
    /// `200 OK` handler. Mirrors how the real router wires sdk_auth →
    /// event_quota → handler.
    fn test_app(limiter: Arc<EnvKeyedRateLimiter>, env_id: Option<&'static str>) -> Router {
        let app = Router::new()
            .route("/v1/events/track", post(|| async { StatusCode::OK }))
            .layer(from_fn_with_state(limiter, event_quota_middleware));
        if let Some(id) = env_id {
            app.layer(axum::middleware::from_fn(
                move |mut req: Request, next: Next| async move {
                    req.extensions_mut().insert(SdkContext {
                        environment_id: id.to_string(),
                        organisation_id: "org".into(),
                        sdk_key_id: "key".into(),
                    });
                    next.run(req).await
                },
            ))
        } else {
            app
        }
    }

    fn track_request() -> Request {
        HttpRequest::builder()
            .method("POST")
            .uri("/v1/events/track")
            .body(Body::empty())
            .unwrap()
    }

    // ── quota_per_sec_from_env ──────────────────────────────────────────────
    //
    // `quota_per_sec_from_env` reads `STITCHD_EVENT_QUOTA_PER_SEC` — a
    // process-global. Cargo runs tests in parallel by default, so splitting
    // these into per-case `#[test]` functions causes races where one test
    // sees the env var set by another. Consolidating into a single
    // sequential test gives deterministic, reliable coverage of every case
    // without introducing a shared `Mutex` or `serial_test` dep.

    #[test]
    fn quota_per_sec_from_env_covers_all_branches() {
        // SAFETY: `std::env::{set_var, remove_var}` are `unsafe` in Rust 2024
        // because env mutations are not thread-safe. We accept the risk
        // because (a) we only touch a STITCHD-prefixed key no other test
        // reads or writes, and (b) the cases inside this single test run
        // sequentially.

        // 1. Unset → default.
        unsafe {
            std::env::remove_var(QUOTA_ENV_VAR);
        }
        assert_eq!(
            quota_per_sec_from_env().get(),
            DEFAULT_QUOTA_PER_SEC,
            "unset env var must yield default"
        );

        // 2. Valid u32 → parsed value.
        unsafe {
            std::env::set_var(QUOTA_ENV_VAR, "42");
        }
        assert_eq!(
            quota_per_sec_from_env().get(),
            42,
            "valid u32 must be parsed"
        );

        // 3. Unparseable → default.
        unsafe {
            std::env::set_var(QUOTA_ENV_VAR, "not-a-number");
        }
        assert_eq!(
            quota_per_sec_from_env().get(),
            DEFAULT_QUOTA_PER_SEC,
            "unparseable value must yield default"
        );

        // 4. Zero → default (zero would brick every SDK).
        unsafe {
            std::env::set_var(QUOTA_ENV_VAR, "0");
        }
        assert_eq!(
            quota_per_sec_from_env().get(),
            DEFAULT_QUOTA_PER_SEC,
            "zero must fall back to default (would otherwise brick every SDK)"
        );

        // Clean up so other tests don't observe leftover state.
        unsafe {
            std::env::remove_var(QUOTA_ENV_VAR);
        }
    }

    // ── Middleware behaviour ────────────────────────────────────────────────

    #[tokio::test]
    async fn test_first_request_under_quota_passes() {
        let limiter = build_limiter(NonZeroU32::new(10).unwrap());
        let app = test_app(limiter, Some("env-a"));
        let resp = app.oneshot(track_request()).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_burst_above_quota_returns_429() {
        // Quota=1/sec → first request passes (consuming the bucket), the
        // second request in the same tick gets 429.
        let limiter = build_limiter(NonZeroU32::new(1).unwrap());

        let first = test_app(Arc::clone(&limiter), Some("env-burst"))
            .oneshot(track_request())
            .await
            .unwrap();
        assert_eq!(first.status(), StatusCode::OK, "first request should pass");

        let second = test_app(Arc::clone(&limiter), Some("env-burst"))
            .oneshot(track_request())
            .await
            .unwrap();
        assert_eq!(
            second.status(),
            StatusCode::TOO_MANY_REQUESTS,
            "second request in same tick must be 429"
        );
    }

    #[tokio::test]
    async fn test_quota_per_env_isolated() {
        // env A exhausts its quota; env B still passes through. Critical
        // isolation property — one noisy tenant must not starve others.
        let limiter = build_limiter(NonZeroU32::new(1).unwrap());

        // Burn env A's bucket.
        let _ = test_app(Arc::clone(&limiter), Some("env-a"))
            .oneshot(track_request())
            .await
            .unwrap();
        let a2 = test_app(Arc::clone(&limiter), Some("env-a"))
            .oneshot(track_request())
            .await
            .unwrap();
        assert_eq!(a2.status(), StatusCode::TOO_MANY_REQUESTS);

        // env B is independent.
        let b1 = test_app(Arc::clone(&limiter), Some("env-b"))
            .oneshot(track_request())
            .await
            .unwrap();
        assert_eq!(
            b1.status(),
            StatusCode::OK,
            "env-b must not inherit env-a's exhausted bucket"
        );
    }

    #[tokio::test]
    async fn test_missing_sdk_context_falls_through_to_next_layer() {
        // No SdkContext injected → middleware should NOT panic and should
        // NOT rate-limit (sdk_auth would return 401 ahead of it in prod;
        // the quota layer's job is purely to gate when context exists).
        let limiter = build_limiter(NonZeroU32::new(1).unwrap());
        let app = test_app(limiter, None); // <-- no SdkContext injected
        let resp = app.oneshot(track_request()).await.unwrap();
        // The downstream handler returns 200; absence of context must not
        // produce 429 or panic.
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn missing_sdk_context_does_not_consume_quota() {
        // Belt-and-braces: confirm a no-context request did NOT decrement the
        // limiter for any key (otherwise an unauthenticated flood could
        // exhaust quota for legitimate envs via key collisions).
        let limiter = build_limiter(NonZeroU32::new(1).unwrap());

        let _ = test_app(Arc::clone(&limiter), None)
            .oneshot(track_request())
            .await
            .unwrap();

        // Now a real request with SdkContext: bucket should still be full.
        let ok = test_app(Arc::clone(&limiter), Some("env-x"))
            .oneshot(track_request())
            .await
            .unwrap();
        assert_eq!(ok.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn quota_response_body_has_machine_readable_error() {
        use axum::body::to_bytes;
        let limiter = build_limiter(NonZeroU32::new(1).unwrap());
        // Burn the bucket.
        let _ = test_app(Arc::clone(&limiter), Some("env-msg"))
            .oneshot(track_request())
            .await
            .unwrap();
        let resp = test_app(Arc::clone(&limiter), Some("env-msg"))
            .oneshot(track_request())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::TOO_MANY_REQUESTS);
        let bytes = to_bytes(resp.into_body(), 4096).await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["error"], "event_quota_exceeded");
    }
}
