//! Rate-limiting helpers for sensitive authentication endpoints.
//!
//! Three separate `GovernorLayer` instances are created so each endpoint
//! has its own independent token-bucket state:
//!
//! | Endpoint                          | Limit   | Burst | Period |
//! |------------------------------------|---------|-------|--------|
//! | POST /auth/login                  | 10/min  |  10   |  6 s   |
//! | POST /auth/mfa/verify             | 10/min  |  10   |  6 s   |
//! | POST /auth/password/reset-request |  5/min  |   5   | 12 s   |
//!
//! Rate limits are applied per client IP using `SmartIpKeyExtractor`, which
//! reads `x-forwarded-for` / `x-real-ip` / `forwarded` headers first and
//! falls back to the peer socket address. This works correctly behind a
//! reverse proxy.
//!
//! Exceeding the limit returns HTTP 429 Too Many Requests.

use std::sync::Arc;
use tower_governor::{
    GovernorLayer,
    governor::{GovernorConfig, GovernorConfigBuilder},
    key_extractor::SmartIpKeyExtractor,
};

type SmartGovernorLayer = GovernorLayer<SmartIpKeyExtractor, governor::middleware::NoOpMiddleware, axum::body::Body>;

fn build_smart_config(per_second: u64, burst_size: u32) -> Arc<GovernorConfig<SmartIpKeyExtractor, governor::middleware::NoOpMiddleware>> {
    let mut builder = GovernorConfigBuilder::default().key_extractor(SmartIpKeyExtractor);
    builder.per_second(per_second).burst_size(burst_size);
    Arc::new(builder.finish().expect("valid governor config"))
}

/// 10 requests/minute per IP — one token replenished every 6 seconds,
/// burst capacity 10.
#[must_use]
pub fn login_rate_limit_layer() -> SmartGovernorLayer {
    GovernorLayer::new(build_smart_config(6, 10))
}

/// Same 10/min limit for MFA verify.
#[must_use]
pub fn mfa_verify_rate_limit_layer() -> SmartGovernorLayer {
    GovernorLayer::new(build_smart_config(6, 10))
}

/// 5 requests/minute per IP — one token replenished every 12 seconds,
/// burst capacity 5.
#[must_use]
pub fn reset_request_rate_limit_layer() -> SmartGovernorLayer {
    GovernorLayer::new(build_smart_config(12, 5))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::Body,
        http::{Request, StatusCode},
        routing::post,
        Router,
    };
    use tower::ServiceExt as _;

    async fn noop_handler() -> StatusCode {
        StatusCode::OK
    }

    // Simulate a client IP via x-forwarded-for so SmartIpKeyExtractor can
    // extract a key in unit tests (no real socket peer addr available).
    fn make_request(uri: &str) -> Request<Body> {
        Request::builder()
            .method("POST")
            .uri(uri)
            .header("x-forwarded-for", "203.0.113.1") // TEST-NET, RFC 5737
            .body(Body::empty())
            .unwrap()
    }

    // ---------------------------------------------------------------------------
    // Login rate limit: 10 per minute (burst 10)
    // ---------------------------------------------------------------------------

    /// Burst of 10 requests all succeed; the 11th is rate-limited (429).
    #[tokio::test]
    async fn login_rate_limit_allows_burst_then_rejects() {
        let app: Router = Router::new()
            .route("/auth/login", post(noop_handler))
            .route_layer(login_rate_limit_layer());

        for _ in 0..10 {
            let resp = app
                .clone()
                .oneshot(make_request("/auth/login"))
                .await
                .unwrap();
            assert_eq!(
                resp.status(),
                StatusCode::OK,
                "expected 200 within burst"
            );
        }

        // 11th request should be rate-limited
        let resp = app
            .clone()
            .oneshot(make_request("/auth/login"))
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::TOO_MANY_REQUESTS,
            "11th request should be 429 Too Many Requests"
        );
    }

    // ---------------------------------------------------------------------------
    // MFA verify rate limit: 10 per minute (burst 10)
    // ---------------------------------------------------------------------------

    #[tokio::test]
    async fn mfa_verify_rate_limit_allows_burst_then_rejects() {
        let app: Router = Router::new()
            .route("/auth/mfa/verify", post(noop_handler))
            .route_layer(mfa_verify_rate_limit_layer());

        for _ in 0..10 {
            let resp = app
                .clone()
                .oneshot(make_request("/auth/mfa/verify"))
                .await
                .unwrap();
            assert_eq!(resp.status(), StatusCode::OK);
        }
        let resp = app
            .clone()
            .oneshot(make_request("/auth/mfa/verify"))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::TOO_MANY_REQUESTS);
    }

    // ---------------------------------------------------------------------------
    // Reset-request rate limit: 5 per minute (burst 5) — stricter
    // ---------------------------------------------------------------------------

    #[tokio::test]
    async fn reset_request_rate_limit_allows_burst_then_rejects() {
        let app: Router = Router::new()
            .route("/auth/password/reset-request", post(noop_handler))
            .route_layer(reset_request_rate_limit_layer());

        for _ in 0..5 {
            let resp = app
                .clone()
                .oneshot(make_request("/auth/password/reset-request"))
                .await
                .unwrap();
            assert_eq!(resp.status(), StatusCode::OK);
        }
        let resp = app
            .clone()
            .oneshot(make_request("/auth/password/reset-request"))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::TOO_MANY_REQUESTS);
    }

    // ---------------------------------------------------------------------------
    // Confirm independent state — login and MFA share no limiter state
    // ---------------------------------------------------------------------------

    #[tokio::test]
    async fn login_and_mfa_have_independent_limiters() {
        let login_app: Router = Router::new()
            .route("/auth/login", post(noop_handler))
            .route_layer(login_rate_limit_layer());

        // Exhaust login burst
        for _ in 0..10 {
            let _ = login_app
                .clone()
                .oneshot(make_request("/auth/login"))
                .await
                .unwrap();
        }

        // MFA verify has its own limiter — 10 fresh requests should all succeed
        let mfa_app: Router = Router::new()
            .route("/auth/mfa/verify", post(noop_handler))
            .route_layer(mfa_verify_rate_limit_layer());

        for _ in 0..10 {
            let resp = mfa_app
                .clone()
                .oneshot(make_request("/auth/mfa/verify"))
                .await
                .unwrap();
            assert_eq!(
                resp.status(),
                StatusCode::OK,
                "MFA limiter should be independent from login limiter"
            );
        }
    }

    // ---------------------------------------------------------------------------
    // Different IPs have independent quotas
    // ---------------------------------------------------------------------------

    #[tokio::test]
    async fn different_ips_have_independent_quotas() {
        let app: Router = Router::new()
            .route("/auth/login", post(noop_handler))
            .route_layer(login_rate_limit_layer());

        // Exhaust quota for IP A
        for _ in 0..10 {
            let _ = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method("POST")
                        .uri("/auth/login")
                        .header("x-forwarded-for", "203.0.113.1")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
        }

        // IP B should still have a full quota
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/auth/login")
                    .header("x-forwarded-for", "203.0.113.2")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::OK,
            "different IP should not be affected by other IP's rate limit"
        );
    }
}
