//! Idempotency-Key middleware (platform_hardening_20260608).
//!
//! Lets clients safely retry mutating requests without duplicating side effects.
//! A client attaches an `Idempotency-Key: <opaque>` header to a mutating request
//! (POST/PUT/PATCH/DELETE). The gateway:
//!
//! 1. **Fingerprints** the request: `scope` = a one-way hash of the
//!    `Authorization` header (so two actors reusing the same client-chosen key
//!    never collide), `request_hash` = a one-way hash of `(method, path, query,
//!    body)`. The raw body is **never stored** (privacy — NFR-1).
//! 2. **Claims** `(scope, key)` in the [`idempotency_keys`] ledger:
//!    - **fresh** → run the handler, then persist the 2xx response (or release
//!      the key on a non-2xx so the client may legitimately retry).
//!    - **completed** (same fingerprint) → **replay** the stored status + body
//!      verbatim with an `Idempotent-Replayed: true` header; the handler does
//!      NOT run again.
//!    - **fingerprint mismatch** (same key, different request) → `422
//!      idempotency_key_reuse`.
//!    - **in-flight** (a concurrent first request still running) → `409` so only
//!      one handler runs per key.
//!
//! Read methods, and any request without the header, pass straight through —
//! the feature is opt-in and fully backward compatible. Any **store error fails
//! open**: the request proceeds unprotected rather than 500ing the API.
//!
//! The store is a trait so the middleware is unit-testable without a database;
//! production uses [`PgIdempotencyStore`].

use std::sync::Arc;
use std::time::Duration;

use axum::{
    body::Body,
    extract::{Request, State},
    http::{
        Method, StatusCode,
        header::{AUTHORIZATION, CONTENT_TYPE},
    },
    middleware::Next,
    response::{IntoResponse, Response},
};
use sha2::{Digest, Sha256};

/// Request header carrying the client-chosen idempotency key.
pub const HEADER_KEY: &str = "idempotency-key";
/// Response header set to `true` on a replayed (deduplicated) response.
pub const HEADER_REPLAYED: &str = "idempotent-replayed";
/// Max request/response body size buffered for fingerprinting / replay (5 MiB).
const MAX_BODY: usize = 5 * 1024 * 1024;
/// Env var configuring the ledger TTL (seconds).
pub const TTL_ENV_VAR: &str = "STITCHD_GATEWAY_IDEMPOTENCY_TTL_SECS";
/// Default TTL: 24h.
pub const DEFAULT_TTL_SECS: u64 = 86_400;

/// A captured, replayable response.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoredResponse {
    /// HTTP status code.
    pub status: u16,
    /// Raw response body bytes.
    pub body: Vec<u8>,
    /// `Content-Type` of the captured response, if any.
    pub content_type: Option<String>,
}

/// Result of claiming a `(scope, key)`.
#[derive(Debug)]
pub enum Claim {
    /// This request owns the key — run the handler.
    Fresh,
    /// The key already completed with an identical fingerprint — replay it.
    Replay(StoredResponse),
    /// The key was used before with a DIFFERENT request fingerprint — misuse.
    Mismatch,
    /// A concurrent first request still holds the key in-flight.
    InFlight,
}

/// Persistence for the idempotency ledger. A trait so the middleware can be
/// unit-tested with an in-memory fake.
#[async_trait::async_trait]
pub trait IdempotencyStore: Send + Sync {
    /// Atomically claim `(scope, key)` for `request_hash`, reporting the prior
    /// state if the key already exists.
    async fn claim(
        &self,
        scope: &str,
        key: &str,
        request_hash: &str,
    ) -> Result<Claim, anyhow::Error>;

    /// Persist a captured 2xx response for future replays of this key.
    async fn complete(
        &self,
        scope: &str,
        key: &str,
        resp: &StoredResponse,
    ) -> Result<(), anyhow::Error>;

    /// Release a still-in-flight (uncompleted) claim so the client may retry.
    async fn release(&self, scope: &str, key: &str) -> Result<(), anyhow::Error>;
}

/// `true` for methods that mutate state and therefore benefit from idempotency.
fn is_mutating(method: &Method) -> bool {
    matches!(
        *method,
        Method::POST | Method::PUT | Method::PATCH | Method::DELETE
    )
}

/// One-way caller scope from the `Authorization` header (never stored raw).
fn scope_from(headers: &axum::http::HeaderMap) -> String {
    let auth = headers
        .get(AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if auth.is_empty() {
        return "anon".to_string();
    }
    let mut h = Sha256::new();
    h.update(auth.as_bytes());
    hex::encode(h.finalize())
}

/// One-way request fingerprint over `(method, path, query, body)`.
fn fingerprint(method: &Method, uri: &axum::http::Uri, body: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(method.as_str().as_bytes());
    h.update(b"\n");
    h.update(uri.path().as_bytes());
    if let Some(q) = uri.query() {
        h.update(b"?");
        h.update(q.as_bytes());
    }
    h.update(b"\n");
    h.update(body);
    hex::encode(h.finalize())
}

fn error_response(status: StatusCode, code: &str, message: &str) -> Response {
    (
        status,
        axum::Json(serde_json::json!({ "error": code, "message": message })),
    )
        .into_response()
}

/// Rebuild a stored response, tagging it as a replay.
fn replay_response(stored: StoredResponse) -> Response {
    let status = StatusCode::from_u16(stored.status).unwrap_or(StatusCode::OK);
    let mut resp = Response::builder()
        .status(status)
        .header(HEADER_REPLAYED, "true");
    if let Some(ct) = &stored.content_type {
        resp = resp.header(CONTENT_TYPE, ct);
    }
    resp.body(Body::from(stored.body))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

/// Axum middleware enforcing idempotency for mutating requests carrying an
/// `Idempotency-Key` header. See the module docs for the full contract.
pub async fn idempotency_middleware(
    State(store): State<Arc<dyn IdempotencyStore>>,
    req: Request,
    next: Next,
) -> Response {
    // Non-mutating, or no key → pass straight through (opt-in, backward-compat).
    if !is_mutating(req.method()) {
        return next.run(req).await;
    }
    let key = match req
        .headers()
        .get(HEADER_KEY)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned)
    {
        Some(k) if !k.is_empty() => k,
        _ => return next.run(req).await,
    };
    let scope = scope_from(req.headers());

    // Buffer the request body to fingerprint it, then reconstruct the request.
    let (parts, body) = req.into_parts();
    let bytes = match axum::body::to_bytes(body, MAX_BODY).await {
        Ok(b) => b,
        Err(_) => {
            // Body already consumed and too large to buffer — we must respond.
            return error_response(
                StatusCode::PAYLOAD_TOO_LARGE,
                "payload_too_large",
                "request body exceeds the idempotency buffering limit",
            );
        }
    };
    let request_hash = fingerprint(&parts.method, &parts.uri, &bytes);

    match store.claim(&scope, &key, &request_hash).await {
        Ok(Claim::Replay(stored)) => return replay_response(stored),
        Ok(Claim::Mismatch) => {
            return error_response(
                StatusCode::UNPROCESSABLE_ENTITY,
                "idempotency_key_reuse",
                "the Idempotency-Key was already used for a different request",
            );
        }
        Ok(Claim::InFlight) => {
            return error_response(
                StatusCode::CONFLICT,
                "idempotency_in_progress",
                "a request with this Idempotency-Key is already being processed",
            );
        }
        Ok(Claim::Fresh) => { /* we own the key — proceed */ }
        Err(e) => {
            // Fail open: dedup unavailable, but the request must still work.
            tracing::warn!("idempotency claim failed, proceeding without dedup: {e}");
            let req = Request::from_parts(parts, Body::from(bytes));
            return next.run(req).await;
        }
    }

    // Run the handler with the reconstructed request.
    let req = Request::from_parts(parts, Body::from(bytes));
    let resp = next.run(req).await;

    // Buffer the response so we can both capture and return it.
    let (rparts, rbody) = resp.into_parts();
    let rbytes = match axum::body::to_bytes(rbody, MAX_BODY).await {
        Ok(b) => b,
        Err(e) => {
            tracing::warn!("idempotency: response too large to capture, releasing key: {e}");
            let _ = store.release(&scope, &key).await;
            return Response::from_parts(rparts, Body::empty());
        }
    };

    if rparts.status.is_success() {
        let content_type = rparts
            .headers
            .get(CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned);
        let stored = StoredResponse {
            status: rparts.status.as_u16(),
            body: rbytes.to_vec(),
            content_type,
        };
        if let Err(e) = store.complete(&scope, &key, &stored).await {
            tracing::warn!("idempotency: failed to persist response: {e}");
        }
    } else {
        // Non-2xx: release the key so the client can legitimately retry (FR-1.7).
        if let Err(e) = store.release(&scope, &key).await {
            tracing::warn!("idempotency: failed to release key after non-2xx: {e}");
        }
    }

    Response::from_parts(rparts, Body::from(rbytes))
}

// ============================================================================
// Postgres-backed store
// ============================================================================

/// Production [`IdempotencyStore`] backed by the `idempotency_keys` table.
#[derive(Clone)]
pub struct PgIdempotencyStore {
    pool: sqlx::PgPool,
}

impl PgIdempotencyStore {
    /// Wrap a Postgres pool.
    #[must_use]
    pub fn new(pool: sqlx::PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait::async_trait]
impl IdempotencyStore for PgIdempotencyStore {
    async fn claim(
        &self,
        scope: &str,
        key: &str,
        request_hash: &str,
    ) -> Result<Claim, anyhow::Error> {
        // Try to atomically claim the key with an in-flight (NULL-status) row.
        let inserted = sqlx::query!(
            "INSERT INTO idempotency_keys (scope, idempotency_key, request_hash)
             VALUES ($1, $2, $3)
             ON CONFLICT (scope, idempotency_key) DO NOTHING",
            scope,
            key,
            request_hash,
        )
        .execute(&self.pool)
        .await?;
        if inserted.rows_affected() == 1 {
            return Ok(Claim::Fresh);
        }

        // The key already existed — inspect its state.
        let row = sqlx::query!(
            "SELECT request_hash, response_status, response_body, response_content_type
             FROM idempotency_keys
             WHERE scope = $1 AND idempotency_key = $2",
            scope,
            key,
        )
        .fetch_optional(&self.pool)
        .await?;

        match row {
            // Raced with a delete (sweep/release) between the INSERT and SELECT —
            // treat as fresh; `complete` upserts the response.
            None => Ok(Claim::Fresh),
            Some(r) if r.request_hash != request_hash => Ok(Claim::Mismatch),
            Some(r) => match r.response_status {
                Some(status) => Ok(Claim::Replay(StoredResponse {
                    status: u16::try_from(status).unwrap_or(200),
                    body: r.response_body.unwrap_or_default(),
                    content_type: r.response_content_type,
                })),
                None => Ok(Claim::InFlight),
            },
        }
    }

    async fn complete(
        &self,
        scope: &str,
        key: &str,
        resp: &StoredResponse,
    ) -> Result<(), anyhow::Error> {
        // Upsert: a row almost always exists (claimed), but tolerate the rare
        // delete-race by inserting if it vanished.
        sqlx::query!(
            "INSERT INTO idempotency_keys
                 (scope, idempotency_key, request_hash, response_status,
                  response_body, response_content_type)
             VALUES ($1, $2, '', $3, $4, $5)
             ON CONFLICT (scope, idempotency_key) DO UPDATE
                 SET response_status = EXCLUDED.response_status,
                     response_body = EXCLUDED.response_body,
                     response_content_type = EXCLUDED.response_content_type",
            scope,
            key,
            i32::from(resp.status),
            resp.body.as_slice(),
            resp.content_type.as_deref(),
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn release(&self, scope: &str, key: &str) -> Result<(), anyhow::Error> {
        // Only delete a still-in-flight claim — never a completed (replayable)
        // row, which a concurrent completer may have just written.
        sqlx::query!(
            "DELETE FROM idempotency_keys
             WHERE scope = $1 AND idempotency_key = $2 AND response_status IS NULL",
            scope,
            key,
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}

/// Read the configured TTL, falling back to [`DEFAULT_TTL_SECS`]. A zero or
/// unparseable value also yields the default (TTL=0 would evict instantly).
#[must_use]
pub fn ttl_from_env() -> Duration {
    // The literal is inlined (rather than `env::var(TTL_ENV_VAR)`) so the
    // `cargo xtask docs` env-var scraper, which matches `env::var("STITCHD_…")`
    // string literals, picks it up for docs/src/deployment/env-vars.md.
    debug_assert_eq!(TTL_ENV_VAR, "STITCHD_GATEWAY_IDEMPOTENCY_TTL_SECS");
    let secs = std::env::var("STITCHD_GATEWAY_IDEMPOTENCY_TTL_SECS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .filter(|s| *s > 0)
        .unwrap_or(DEFAULT_TTL_SECS);
    Duration::from_secs(secs)
}

/// Delete ledger rows older than `ttl`. Returns the number swept.
///
/// # Errors
/// Propagates any database error.
pub async fn sweep_expired(pool: &sqlx::PgPool, ttl: Duration) -> Result<u64, anyhow::Error> {
    let cutoff = chrono::Utc::now()
        - chrono::Duration::from_std(ttl).unwrap_or_else(|_| chrono::Duration::hours(24));
    let res = sqlx::query!(
        "DELETE FROM idempotency_keys WHERE created_at < $1",
        cutoff,
    )
    .execute(pool)
    .await?;
    Ok(res.rows_affected())
}

/// Spawn the background TTL sweeper. Runs once per `min(ttl, 1h)` and on each
/// tick deletes rows older than `ttl`.
pub fn spawn_sweeper(pool: sqlx::PgPool, ttl: Duration) -> tokio::task::JoinHandle<()> {
    // Sweep at most hourly, and at least once per TTL window.
    let period = ttl.min(Duration::from_secs(3600)).max(Duration::from_secs(60));
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(period);
        // Skip the immediate first tick so startup isn't a no-op DELETE storm.
        interval.tick().await;
        loop {
            interval.tick().await;
            match sweep_expired(&pool, ttl).await {
                Ok(n) if n > 0 => tracing::info!(swept = n, "idempotency TTL sweep"),
                Ok(_) => {}
                Err(e) => tracing::warn!("idempotency TTL sweep failed: {e}"),
            }
        }
    })
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{Router, body::to_bytes, http::Request as HttpRequest, routing::post};
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tower::ServiceExt;

    /// (scope, key) → (request_hash, completed-response).
    type MemRow = (String, Option<StoredResponse>);
    type MemRows = std::collections::HashMap<(String, String), MemRow>;

    /// In-memory store mirroring the Pg semantics for middleware tests.
    #[derive(Default)]
    struct MemStore {
        rows: Mutex<MemRows>,
    }
    #[async_trait::async_trait]
    impl IdempotencyStore for MemStore {
        async fn claim(
            &self,
            scope: &str,
            key: &str,
            request_hash: &str,
        ) -> Result<Claim, anyhow::Error> {
            let mut rows = self.rows.lock().unwrap();
            match rows.get(&(scope.into(), key.into())) {
                None => {
                    rows.insert((scope.into(), key.into()), (request_hash.into(), None));
                    Ok(Claim::Fresh)
                }
                Some((rh, _)) if rh != request_hash => Ok(Claim::Mismatch),
                Some((_, Some(stored))) => Ok(Claim::Replay(stored.clone())),
                Some((_, None)) => Ok(Claim::InFlight),
            }
        }
        async fn complete(
            &self,
            scope: &str,
            key: &str,
            resp: &StoredResponse,
        ) -> Result<(), anyhow::Error> {
            let mut rows = self.rows.lock().unwrap();
            rows.entry((scope.into(), key.into()))
                .or_insert_with(|| (String::new(), None))
                .1 = Some(resp.clone());
            Ok(())
        }
        async fn release(&self, scope: &str, key: &str) -> Result<(), anyhow::Error> {
            let mut rows = self.rows.lock().unwrap();
            if let Some((_, None)) = rows.get(&(scope.into(), key.into())) {
                rows.remove(&(scope.into(), key.into()));
            }
            Ok(())
        }
    }

    fn app(store: Arc<dyn IdempotencyStore>, calls: Arc<AtomicUsize>) -> Router {
        let fail_at = calls.clone();
        Router::new()
            .route(
                "/v1/things",
                post(move || {
                    let n = fail_at.fetch_add(1, Ordering::SeqCst);
                    async move {
                        // First call 200; used by tests asserting handler-run count.
                        (StatusCode::OK, format!("created-{n}"))
                    }
                }),
            )
            .layer(axum::middleware::from_fn_with_state(
                store,
                idempotency_middleware,
            ))
    }

    fn post_req(key: Option<&str>, body: &str) -> Request {
        let mut b = HttpRequest::builder().method("POST").uri("/v1/things");
        if let Some(k) = key {
            b = b.header(HEADER_KEY, k);
        }
        b.body(Body::from(body.to_owned())).unwrap()
    }

    #[tokio::test]
    async fn replays_same_key_same_body_without_rerunning_handler() {
        let store: Arc<dyn IdempotencyStore> = Arc::new(MemStore::default());
        let calls = Arc::new(AtomicUsize::new(0));

        let r1 = app(store.clone(), calls.clone())
            .oneshot(post_req(Some("k1"), "{}"))
            .await
            .unwrap();
        assert_eq!(r1.status(), StatusCode::OK);
        assert!(r1.headers().get(HEADER_REPLAYED).is_none());
        let b1 = to_bytes(r1.into_body(), MAX_BODY).await.unwrap();

        let r2 = app(store.clone(), calls.clone())
            .oneshot(post_req(Some("k1"), "{}"))
            .await
            .unwrap();
        assert_eq!(r2.status(), StatusCode::OK);
        assert_eq!(
            r2.headers().get(HEADER_REPLAYED).unwrap(),
            "true",
            "second call must be flagged as a replay"
        );
        let b2 = to_bytes(r2.into_body(), MAX_BODY).await.unwrap();

        assert_eq!(b1, b2, "replayed body is byte-identical");
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "handler ran exactly once across the two requests"
        );
    }

    #[tokio::test]
    async fn same_key_different_body_returns_422() {
        let store: Arc<dyn IdempotencyStore> = Arc::new(MemStore::default());
        let calls = Arc::new(AtomicUsize::new(0));

        let _ = app(store.clone(), calls.clone())
            .oneshot(post_req(Some("k1"), "{\"a\":1}"))
            .await
            .unwrap();
        let r2 = app(store.clone(), calls.clone())
            .oneshot(post_req(Some("k1"), "{\"a\":2}"))
            .await
            .unwrap();
        assert_eq!(r2.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let body = to_bytes(r2.into_body(), MAX_BODY).await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["error"], "idempotency_key_reuse");
    }

    #[tokio::test]
    async fn no_header_passes_through_each_time() {
        let store: Arc<dyn IdempotencyStore> = Arc::new(MemStore::default());
        let calls = Arc::new(AtomicUsize::new(0));

        for _ in 0..3 {
            let r = app(store.clone(), calls.clone())
                .oneshot(post_req(None, "{}"))
                .await
                .unwrap();
            assert_eq!(r.status(), StatusCode::OK);
        }
        assert_eq!(
            calls.load(Ordering::SeqCst),
            3,
            "without a key every request runs the handler"
        );
    }

    #[tokio::test]
    async fn non_2xx_releases_key_so_client_can_retry() {
        // Handler returns 500 the first time, 200 after — a released key must let
        // the retry reach the handler rather than replaying the failure.
        let store: Arc<dyn IdempotencyStore> = Arc::new(MemStore::default());
        let calls = Arc::new(AtomicUsize::new(0));
        let counter = calls.clone();
        let app = Router::new()
            .route(
                "/v1/things",
                post(move || {
                    let n = counter.fetch_add(1, Ordering::SeqCst);
                    async move {
                        if n == 0 {
                            StatusCode::INTERNAL_SERVER_ERROR.into_response()
                        } else {
                            (StatusCode::OK, "ok").into_response()
                        }
                    }
                }),
            )
            .layer(axum::middleware::from_fn_with_state(
                store.clone(),
                idempotency_middleware,
            ));

        let r1 = app
            .clone()
            .oneshot(post_req(Some("k1"), "{}"))
            .await
            .unwrap();
        assert_eq!(r1.status(), StatusCode::INTERNAL_SERVER_ERROR);

        let r2 = app.oneshot(post_req(Some("k1"), "{}")).await.unwrap();
        assert_eq!(
            r2.status(),
            StatusCode::OK,
            "retry after a released non-2xx must reach the handler"
        );
        assert_eq!(calls.load(Ordering::SeqCst), 2, "handler ran twice");
    }

    #[tokio::test]
    async fn get_method_is_ignored() {
        let store: Arc<dyn IdempotencyStore> = Arc::new(MemStore::default());
        let calls = Arc::new(AtomicUsize::new(0));
        let counter = calls.clone();
        let app = Router::new()
            .route(
                "/v1/things",
                axum::routing::get(move || {
                    counter.fetch_add(1, Ordering::SeqCst);
                    async { StatusCode::OK }
                }),
            )
            .layer(axum::middleware::from_fn_with_state(
                store,
                idempotency_middleware,
            ));
        for _ in 0..2 {
            let req = HttpRequest::builder()
                .method("GET")
                .uri("/v1/things")
                .header(HEADER_KEY, "k1")
                .body(Body::empty())
                .unwrap();
            let r = app.clone().oneshot(req).await.unwrap();
            assert_eq!(r.status(), StatusCode::OK);
        }
        assert_eq!(
            calls.load(Ordering::SeqCst),
            2,
            "GET is never deduplicated even with a key header"
        );
    }

    #[test]
    fn fingerprint_is_stable_and_sensitive() {
        let m = Method::POST;
        let u: axum::http::Uri = "/v1/things?x=1".parse().unwrap();
        let a = fingerprint(&m, &u, b"{}");
        let b = fingerprint(&m, &u, b"{}");
        assert_eq!(a, b, "same inputs → same fingerprint");
        assert_ne!(
            a,
            fingerprint(&m, &u, b"{\"y\":2}"),
            "different body → different fingerprint"
        );
        let u2: axum::http::Uri = "/v1/things?x=2".parse().unwrap();
        assert_ne!(
            a,
            fingerprint(&m, &u2, b"{}"),
            "different query → different fingerprint"
        );
    }

    #[test]
    fn scope_distinguishes_actors_and_anon() {
        let mut h1 = axum::http::HeaderMap::new();
        h1.insert(AUTHORIZATION, "Bearer aaa".parse().unwrap());
        let mut h2 = axum::http::HeaderMap::new();
        h2.insert(AUTHORIZATION, "Bearer bbb".parse().unwrap());
        let empty = axum::http::HeaderMap::new();
        assert_ne!(scope_from(&h1), scope_from(&h2));
        assert_eq!(scope_from(&empty), "anon");
        assert_ne!(scope_from(&h1), "anon");
    }

    // ── Postgres-backed store (live PG via sqlx::test) ──────────────────────

    #[sqlx::test(migrations = "../stitchd-db/migrations")]
    async fn pg_store_claim_complete_replay_lifecycle(pool: sqlx::PgPool) {
        let store = PgIdempotencyStore::new(pool.clone());

        // 1. First claim is fresh.
        assert!(matches!(
            store.claim("s", "k", "h1").await.unwrap(),
            Claim::Fresh
        ));
        // 2. Re-claim before completion → in-flight.
        assert!(matches!(
            store.claim("s", "k", "h1").await.unwrap(),
            Claim::InFlight
        ));
        // 3. Complete with a 201 body.
        let stored = StoredResponse {
            status: 201,
            body: b"{\"id\":1}".to_vec(),
            content_type: Some("application/json".into()),
        };
        store.complete("s", "k", &stored).await.unwrap();
        // 4. Re-claim same hash → replay the exact stored response.
        match store.claim("s", "k", "h1").await.unwrap() {
            Claim::Replay(r) => assert_eq!(r, stored),
            other => panic!("expected Replay, got {other:?}"),
        }
        // 5. Re-claim with a DIFFERENT hash → mismatch.
        assert!(matches!(
            store.claim("s", "k", "h2").await.unwrap(),
            Claim::Mismatch
        ));
        // 6. A different scope with the same key is independent.
        assert!(matches!(
            store.claim("other", "k", "h1").await.unwrap(),
            Claim::Fresh
        ));
    }

    #[sqlx::test(migrations = "../stitchd-db/migrations")]
    async fn pg_store_release_lets_key_be_reclaimed(pool: sqlx::PgPool) {
        let store = PgIdempotencyStore::new(pool);
        assert!(matches!(
            store.claim("s", "k", "h").await.unwrap(),
            Claim::Fresh
        ));
        // Release the in-flight claim (non-2xx path) → key is free again.
        store.release("s", "k").await.unwrap();
        assert!(matches!(
            store.claim("s", "k", "h").await.unwrap(),
            Claim::Fresh
        ));

        // Release must NOT delete a COMPLETED row (a concurrent completer's win).
        let stored = StoredResponse {
            status: 200,
            body: b"ok".to_vec(),
            content_type: None,
        };
        store.complete("s", "k", &stored).await.unwrap();
        store.release("s", "k").await.unwrap();
        assert!(
            matches!(store.claim("s", "k", "h").await.unwrap(), Claim::Replay(_)),
            "release must not evict a completed (replayable) row"
        );
    }

    #[sqlx::test(migrations = "../stitchd-db/migrations")]
    async fn pg_store_sweep_deletes_expired_rows(pool: sqlx::PgPool) {
        let store = PgIdempotencyStore::new(pool.clone());
        store.claim("s", "k", "h").await.unwrap();
        // Backdate the row well past any TTL.
        sqlx::query!(
            "UPDATE idempotency_keys SET created_at = now() - interval '48 hours'
             WHERE scope = 's' AND idempotency_key = 'k'"
        )
        .execute(&pool)
        .await
        .unwrap();

        let swept = sweep_expired(&pool, Duration::from_secs(DEFAULT_TTL_SECS))
            .await
            .unwrap();
        assert_eq!(swept, 1, "the backdated row is swept");
        // Gone → a re-claim is fresh.
        assert!(matches!(
            store.claim("s", "k", "h").await.unwrap(),
            Claim::Fresh
        ));
    }

    #[test]
    fn ttl_from_env_defaults_and_parses() {
        // SAFETY: STITCHD_-prefixed key, mutated sequentially within one test.
        unsafe { std::env::remove_var(TTL_ENV_VAR) };
        assert_eq!(ttl_from_env(), Duration::from_secs(DEFAULT_TTL_SECS));
        unsafe { std::env::set_var(TTL_ENV_VAR, "120") };
        assert_eq!(ttl_from_env(), Duration::from_secs(120));
        unsafe { std::env::set_var(TTL_ENV_VAR, "0") };
        assert_eq!(
            ttl_from_env(),
            Duration::from_secs(DEFAULT_TTL_SECS),
            "zero falls back to default"
        );
        unsafe { std::env::remove_var(TTL_ENV_VAR) };
    }
}
