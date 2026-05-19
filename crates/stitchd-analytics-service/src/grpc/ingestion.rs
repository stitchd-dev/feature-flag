//! `TrackEvents` — SDK-facing batch event ingestion for `AnalyticsService`.
//!
//! # Protocol overview
//!
//! The gateway is the sole SDK trust boundary (see `patterns.md`: "Gateway is the
//! sole SDK trust boundary"). It forwards SDK requests to this RPC with a
//! trusted `x-env-id` gRPC metadata header. This handler therefore does NOT
//! validate SDK keys; it only validates that `x-env-id` is present and parses as
//! a UUID.
//!
//! For each [`TrackEvent`] in the batch:
//! 1. Resolve `event_key` against the PostgreSQL `event_definitions` table
//!    (with a 60-second in-process `moka` cache to avoid hammering PG on
//!    hot SDK paths).
//! 2. Reject unknown / soft-deleted keys with a per-event `RejectedEvent`
//!    (rejected events do **not** abort the batch).
//! 3. Validate the `MetricValue` matches the registered `EventValueType`.
//! 4. Build an [`EventV2Row`] and accumulate.
//! 5. After the loop, write all accepted rows to ClickHouse `events_v2` in
//!    a single batch INSERT.
//!
//! Failures from ClickHouse propagate as `tonic::Status::Internal`; per-event
//! validation rejections are surfaced via [`TrackEventsResponse::rejected`].

use std::sync::Arc;
use std::time::Duration;

use moka::future::Cache;
use serde::Serialize;
use tonic::{Request, Response, Status};
use tracing::instrument;
use uuid::Uuid;

use stitchd_core::event::{EventDefinition, EventValueType};
use stitchd_core::id::EnvironmentId;
use stitchd_db::{EventDefinitionRepository, RepositoryError};
use stitchd_proto::analytics::v1::{
    RejectedEvent, TrackEvent, TrackEventsRequest, TrackEventsResponse, metric_value,
};

/// Default TTL for the event-definition validation cache. 60 seconds matches
/// the `SdkKeyCache` precedent in `stitchd-auth-service` and bounds the
/// staleness window for archived/deleted events.
pub const EVENT_DEF_CACHE_TTL: Duration = Duration::from_secs(60);

/// Max distinct cache entries. Each entry is `(EnvironmentId, key) →
/// Arc<EventDefinition>`. 4_096 supports ~64 envs × 64 unique event keys —
/// plenty for typical deployments.
pub const EVENT_DEF_CACHE_CAPACITY: u64 = 4_096;

// ---------------------------------------------------------------------------
// Cache
// ---------------------------------------------------------------------------

/// Cache value: `None` is a cached negative result (unknown / archived key).
type CachedDef = Option<Arc<EventDefinition>>;

/// Inner moka cache type — extracted to keep the struct field signature
/// readable and to satisfy `clippy::type_complexity`.
type DefCacheMap = Cache<(EnvironmentId, String), CachedDef>;

/// In-process TTL cache for `(EnvironmentId, event_key) → Option<EventDefinition>`.
///
/// `None` is a cached negative result — a known-unknown — so we don't hammer
/// PG for repeatedly-rejected SDK keys. Both hits and negative hits age out
/// after [`EVENT_DEF_CACHE_TTL`]; archived event_definitions that get
/// resurrected and unknown keys that later get created will both surface
/// within ~60 seconds.
///
/// Cache misses fall through to the [`EventDefinitionRepository::find_by_key`]
/// loader, which (by repo contract) filters out soft-deleted rows — so a
/// `Some(_)` hit guarantees the definition is active at lookup time.
#[derive(Clone)]
pub struct EventDefinitionCache {
    inner: Arc<DefCacheMap>,
}

impl EventDefinitionCache {
    /// Build a new cache with default TTL + capacity.
    #[must_use]
    pub fn new() -> Self {
        Self::with_ttl(EVENT_DEF_CACHE_TTL)
    }

    /// Build a cache with a custom TTL — used by tests to exercise expiry.
    #[must_use]
    pub fn with_ttl(ttl: Duration) -> Self {
        let inner = Cache::builder()
            .max_capacity(EVENT_DEF_CACHE_CAPACITY)
            .time_to_live(ttl)
            .build();
        Self {
            inner: Arc::new(inner),
        }
    }

    /// Fetch an `EventDefinition` for `(env_id, key)` — on a miss, the
    /// supplied loader is invoked exactly once (concurrent callers coalesce).
    ///
    /// Returns `Ok(None)` when the loader signals `NotFound` (unknown OR
    /// archived event_key — the repo treats them identically).
    ///
    /// # Errors
    ///
    /// Propagates any non-`NotFound` repository error from the loader.
    pub async fn get_or_load<F, Fut>(
        &self,
        env_id: EnvironmentId,
        key: &str,
        loader: F,
    ) -> Result<CachedDef, RepositoryError>
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = Result<EventDefinition, RepositoryError>>,
    {
        // moka's `try_get_with` coalesces concurrent loaders for the same key.
        // We use a sentinel `Option<Arc<EventDefinition>>` representation —
        // mapping `RepositoryError::NotFound` into `Ok(None)` so callers can
        // distinguish "unknown" (cacheable negative) from real failures
        // (transient — uncached, retried next request).
        let cache_key = (env_id, key.to_string());

        // Try the cache first; on miss invoke the loader and cache the result.
        // We cache `Some(...)` for hits and propagate `NotFound` as
        // `Ok(None)` via a custom return-type wrapper.
        let result = self
            .inner
            .try_get_with(cache_key, async move {
                match loader().await {
                    Ok(def) => Ok(Some(Arc::new(def))),
                    Err(RepositoryError::NotFound { .. }) => Ok(None),
                    Err(other) => Err(other),
                }
            })
            .await;

        match result {
            Ok(some_or_none) => Ok(some_or_none),
            Err(arc_err) => Err(reconstruct_repo_error(&arc_err)),
        }
    }
}

impl Default for EventDefinitionCache {
    fn default() -> Self {
        Self::new()
    }
}

/// Recreate a [`RepositoryError`] from an `Arc<RepositoryError>` returned by
/// moka. Mirrors the pattern from `stitchd-auth-service::sdk_key_cache`.
fn reconstruct_repo_error(arc: &RepositoryError) -> RepositoryError {
    match arc {
        RepositoryError::NotFound { id } => RepositoryError::NotFound { id: id.clone() },
        RepositoryError::VersionConflict { expected, actual } => RepositoryError::VersionConflict {
            expected: *expected,
            actual: *actual,
        },
        RepositoryError::UniqueViolation { field } => RepositoryError::UniqueViolation {
            field: field.clone(),
        },
        RepositoryError::ForeignKeyViolation { constraint } => {
            RepositoryError::ForeignKeyViolation {
                constraint: constraint.clone(),
            }
        }
        RepositoryError::InvalidState { reason } => RepositoryError::InvalidState {
            reason: reason.clone(),
        },
        other => RepositoryError::Unexpected(anyhow::anyhow!("{other}")),
    }
}

// ---------------------------------------------------------------------------
// Row written to ClickHouse `events_v2`
// ---------------------------------------------------------------------------

/// A row in the `events_v2` ClickHouse table.
///
/// Mirrors the schema added in `events_metrics_20260519` Phase 1 (migration
/// `20260520000001_events_v2_properties`): the canonical ingestion table now
/// carries `properties` + `occurred_at` alongside the legacy columns.
#[derive(Debug, Serialize, clickhouse::Row)]
pub struct EventV2Row {
    /// Tenant scope.
    #[serde(with = "clickhouse::serde::uuid")]
    pub env_id: Uuid,
    /// Evaluation-time context pairs: `(type, key)`.
    pub contexts: Vec<(String, String)>,
    /// Pre-registered event key (FK → `event_definitions.key`).
    pub metric_key: String,
    /// Bool variant; populated iff registered type is `Bool`.
    pub value_bool: Option<bool>,
    /// Int variant; populated iff registered type is `Int`.
    pub value_int: Option<i64>,
    /// Double variant; populated iff registered type is `Double`.
    pub value_double: Option<f64>,
    /// Server-side ingestion timestamp (Unix millis).
    pub timestamp: i64,
    /// Arbitrary per-event metadata (filterable by metrics layer).
    ///
    /// Encoded as `Vec<(String, String)>` rather than `HashMap` because
    /// the `clickhouse` crate's RowBinary serializer only supports
    /// sequences for `Map(K, V)` columns — see clickhouse-rs#193.
    pub properties: Vec<(String, String)>,
    /// Client wall-clock time (Unix millis). Falls back to `timestamp` when
    /// the SDK omitted `occurred_at` in the request.
    pub occurred_at: i64,
}

// ---------------------------------------------------------------------------
// State + handler
// ---------------------------------------------------------------------------

/// State injected into [`handle_track_events`] — wired up in
/// `grpc::service::ServiceState`.
pub struct TrackEventsState {
    /// PG-backed `event_definitions` repository — used on cache miss only.
    pub event_def_repo: Arc<dyn EventDefinitionRepository>,
    /// 60s TTL validation cache.
    pub event_def_cache: EventDefinitionCache,
    /// ClickHouse client — used to write the batch INSERT into `events_v2`.
    pub ch_client: Arc<clickhouse::Client>,
}

/// Resolve the environment ID from a request.
///
/// Gateway-trusted: looks for `x-env-id` in gRPC metadata first; falls back
/// to `TrackEventsRequest.env_id` for direct callers / tests.
#[allow(clippy::result_large_err)] // tonic::Status is external; cannot box.
fn resolve_env_id(
    metadata: &tonic::metadata::MetadataMap,
    body_env_id: &str,
) -> Result<EnvironmentId, Status> {
    if let Some(env_id_str) = metadata.get("x-env-id").and_then(|v| v.to_str().ok()) {
        let uuid = env_id_str
            .parse::<Uuid>()
            .map_err(|_| Status::unauthenticated("x-env-id is not a valid UUID"))?;
        return Ok(EnvironmentId::from_uuid(uuid));
    }

    if !body_env_id.is_empty() {
        let uuid = body_env_id
            .parse::<Uuid>()
            .map_err(|_| Status::invalid_argument("env_id is not a valid UUID"))?;
        return Ok(EnvironmentId::from_uuid(uuid));
    }

    Err(Status::unauthenticated(
        "missing x-env-id metadata and request.env_id is empty",
    ))
}

/// Convert an SDK `TrackEvent` to an ingest-ready [`EventV2Row`], or return
/// a [`RejectedEvent`] with a machine-readable reason discriminant.
fn validate_event(
    event: &TrackEvent,
    def: &EventDefinition,
    env_id: EnvironmentId,
    now_millis: i64,
) -> Result<EventV2Row, RejectedEvent> {
    // ── value-type match ──────────────────────────────────────────────────
    let value = event.value.as_ref().and_then(|v| v.value.as_ref());

    let (value_bool, value_int, value_double) = match (def.value_type, value) {
        (EventValueType::Bool, Some(metric_value::Value::BoolValue(b))) => (Some(*b), None, None),
        (EventValueType::Int, Some(metric_value::Value::IntValue(i))) => (None, Some(*i), None),
        (EventValueType::Double, Some(metric_value::Value::DoubleValue(d))) => {
            (None, None, Some(*d))
        }
        (_, None) => {
            return Err(RejectedEvent {
                event_key: event.event_key.clone(),
                reason: "missing_value".to_string(),
            });
        }
        // Any other (type, present-value) combination is a mismatch.
        _ => {
            return Err(RejectedEvent {
                event_key: event.event_key.clone(),
                reason: "value_type_mismatch".to_string(),
            });
        }
    };

    // ── occurred_at: optional RFC 3339; default to ingestion time ─────────
    let occurred_at = if let Some(s) = event.occurred_at.as_deref() {
        match chrono::DateTime::parse_from_rfc3339(s) {
            Ok(dt) => dt.timestamp_millis(),
            Err(_) => {
                return Err(RejectedEvent {
                    event_key: event.event_key.clone(),
                    reason: "invalid_occurred_at".to_string(),
                });
            }
        }
    } else {
        now_millis
    };

    // Proto generates `properties` as `HashMap<String, String>`; the
    // `clickhouse` crate requires a sequence type for `Map(K, V)` columns.
    let properties: Vec<(String, String)> = event
        .properties
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();

    Ok(EventV2Row {
        env_id: env_id.as_uuid(),
        contexts: vec![(event.context_type.clone(), event.context_key.clone())],
        metric_key: event.event_key.clone(),
        value_bool,
        value_int,
        value_double,
        timestamp: now_millis,
        properties,
        occurred_at,
    })
}

/// Batch-write accepted rows to `events_v2`. Returns `Status::Internal` on
/// any ClickHouse failure — the whole batch is dropped in that case (callers
/// should retry).
#[allow(clippy::result_large_err)] // tonic::Status is external; cannot box.
async fn write_rows_to_clickhouse(
    ch_client: &clickhouse::Client,
    rows: &[EventV2Row],
) -> Result<(), Status> {
    if rows.is_empty() {
        return Ok(());
    }

    let mut insert = ch_client
        .insert("events_v2")
        .map_err(|e| Status::internal(format!("clickhouse insert init failed: {e}")))?;
    for row in rows {
        insert
            .write(row)
            .await
            .map_err(|e| Status::internal(format!("clickhouse row write failed: {e}")))?;
    }
    insert
        .end()
        .await
        .map_err(|e| Status::internal(format!("clickhouse insert end failed: {e}")))?;
    Ok(())
}

/// Handle a `TrackEvents` RPC — validate each event against PG, write the
/// accepted batch to ClickHouse `events_v2`.
///
/// # Errors
///
/// Returns `Unauthenticated` when `x-env-id` is missing/invalid,
/// `Internal` when the ClickHouse batch INSERT fails.
#[allow(clippy::result_large_err)] // tonic::Status is external; cannot box.
#[instrument(skip(state, request), name = "analytics.track_events")]
pub async fn handle_track_events(
    state: &TrackEventsState,
    request: Request<TrackEventsRequest>,
) -> Result<Response<TrackEventsResponse>, Status> {
    let metadata = request.metadata().clone();
    let inner = request.into_inner();

    let env_id = resolve_env_id(&metadata, &inner.env_id)?;

    // Short-circuit empty batches without touching PG / CH.
    if inner.events.is_empty() {
        return Ok(Response::new(TrackEventsResponse {
            accepted_count: 0,
            rejected: vec![],
        }));
    }

    let now_millis = chrono::Utc::now().timestamp_millis();

    let mut rows: Vec<EventV2Row> = Vec::with_capacity(inner.events.len());
    let mut rejected: Vec<RejectedEvent> = Vec::new();

    for event in &inner.events {
        // Cache-aware definition lookup. Missing key → reject; transient
        // DB errors → propagate as Internal (drops the whole batch on the
        // first failure — matches IngestEvent's behaviour).
        let def_opt = state
            .event_def_cache
            .get_or_load(env_id, &event.event_key, || {
                let repo = state.event_def_repo.clone();
                let key = event.event_key.clone();
                async move { repo.find_by_key(&key, env_id).await }
            })
            .await
            .map_err(|e| Status::internal(e.to_string()))?;

        let Some(def) = def_opt else {
            // PG `find_by_key` filters `deleted_at IS NULL`, so we cannot
            // distinguish "never existed" from "archived" here. Both map to
            // the same SDK-facing reason: "unknown_event_key".
            rejected.push(RejectedEvent {
                event_key: event.event_key.clone(),
                reason: "unknown_event_key".to_string(),
            });
            continue;
        };

        match validate_event(event, &def, env_id, now_millis) {
            Ok(row) => rows.push(row),
            Err(rej) => rejected.push(rej),
        }
    }

    write_rows_to_clickhouse(&state.ch_client, &rows).await?;

    // Cast is bounded by the request size (capped at the gateway).
    let accepted_count = u32::try_from(rows.len()).unwrap_or(u32::MAX);

    metrics::counter!("analytics_service.track_events.accepted")
        .increment(u64::from(accepted_count));
    metrics::counter!("analytics_service.track_events.rejected").increment(rejected.len() as u64);

    Ok(Response::new(TrackEventsResponse {
        accepted_count,
        rejected,
    }))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Mutex;

    use async_trait::async_trait;
    use chrono::Utc;
    use tonic::metadata::MetadataValue;

    use stitchd_core::{
        event::{EventDefinition, EventValueType},
        id::{EnvironmentId, EventDefinitionId},
    };
    use stitchd_db::{EventDefinitionRepository, RepositoryError};
    use stitchd_proto::analytics::v1::{MetricValue, RejectedEvent, TrackEvent, TrackEventsRequest};

    use super::*;

    // -----------------------------------------------------------------------
    // Mock: EventDefinitionRepository — accepts a list of (key, value_type,
    // archived?) tuples per env, returns NotFound for archived or missing.
    // -----------------------------------------------------------------------

    struct MockEventDefRepo {
        defs: Mutex<HashMap<(EnvironmentId, String), EventDefinition>>,
        call_count: std::sync::atomic::AtomicUsize,
    }

    impl MockEventDefRepo {
        fn new(env_id: EnvironmentId, defs: Vec<(&str, EventValueType, bool)>) -> Self {
            let now = Utc::now();
            let mut map: HashMap<(EnvironmentId, String), EventDefinition> = HashMap::new();
            for (key, value_type, archived) in defs {
                let def = EventDefinition {
                    id: EventDefinitionId::new(),
                    environment_id: env_id,
                    key: key.to_string(),
                    value_type,
                    created_at: now,
                    updated_at: now,
                    deleted_at: if archived { Some(now) } else { None },
                    version: 1,
                };
                map.insert((env_id, key.to_string()), def);
            }
            Self {
                defs: Mutex::new(map),
                call_count: std::sync::atomic::AtomicUsize::new(0),
            }
        }

        fn calls(&self) -> usize {
            self.call_count
                .load(std::sync::atomic::Ordering::SeqCst)
        }
    }

    #[async_trait]
    impl EventDefinitionRepository for MockEventDefRepo {
        async fn find_by_id(
            &self,
            _id: EventDefinitionId,
        ) -> Result<EventDefinition, RepositoryError> {
            Err(RepositoryError::NotFound { id: "mock".into() })
        }

        async fn find_by_key(
            &self,
            key: &str,
            environment_id: EnvironmentId,
        ) -> Result<EventDefinition, RepositoryError> {
            self.call_count
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let guard = self.defs.lock().unwrap();
            match guard.get(&(environment_id, key.to_string())) {
                Some(def) if def.deleted_at.is_none() => Ok(def.clone()),
                // Archived or missing both surface as NotFound — PG repo
                // filters `deleted_at IS NULL` in `find_by_key`.
                _ => Err(RepositoryError::NotFound {
                    id: format!("{key}@{environment_id}"),
                }),
            }
        }

        async fn list_by_environment(
            &self,
            _environment_id: EnvironmentId,
        ) -> Result<Vec<EventDefinition>, RepositoryError> {
            Ok(vec![])
        }

        async fn create(&self, _def: &EventDefinition) -> Result<(), RepositoryError> {
            Ok(())
        }

        async fn update(&self, def: &EventDefinition) -> Result<EventDefinition, RepositoryError> {
            Ok(def.clone())
        }

        async fn soft_delete(&self, _id: EventDefinitionId) -> Result<(), RepositoryError> {
            Ok(())
        }
    }

    // -----------------------------------------------------------------------
    // Helpers
    // -----------------------------------------------------------------------

    /// Build a `clickhouse::Client` pointed at the local ClickHouse container.
    /// All tests requiring ClickHouse use this — they share a database so
    /// each test scopes its writes to a fresh `env_id` to avoid cross-talk.
    fn make_ch_client() -> clickhouse::Client {
        clickhouse::Client::default()
            .with_url("http://localhost:8123")
            .with_user("stitchd")
            .with_password("stitchd")
            .with_database("stitchd")
    }

    /// Build a state suitable for tests that need real ClickHouse writes.
    fn make_state_with_ch(
        env_id: EnvironmentId,
        defs: Vec<(&str, EventValueType, bool)>,
    ) -> TrackEventsState {
        TrackEventsState {
            event_def_repo: Arc::new(MockEventDefRepo::new(env_id, defs)),
            event_def_cache: EventDefinitionCache::new(),
            ch_client: Arc::new(make_ch_client()),
        }
    }

    /// Build a request with `x-env-id` set in metadata.
    fn make_request(env_id: EnvironmentId, events: Vec<TrackEvent>) -> Request<TrackEventsRequest> {
        let mut req = Request::new(TrackEventsRequest {
            env_id: String::new(),
            events,
        });
        req.metadata_mut().insert(
            "x-env-id",
            MetadataValue::try_from(env_id.as_uuid().to_string()).expect("uuid is valid ascii"),
        );
        req
    }

    fn bool_event(key: &str, b: bool) -> TrackEvent {
        TrackEvent {
            event_key: key.to_string(),
            context_type: "user".into(),
            context_key: "u1".into(),
            value: Some(MetricValue {
                value: Some(metric_value::Value::BoolValue(b)),
            }),
            properties: HashMap::new(),
            occurred_at: None,
        }
    }

    fn int_event(key: &str, i: i64) -> TrackEvent {
        TrackEvent {
            event_key: key.to_string(),
            context_type: "user".into(),
            context_key: "u2".into(),
            value: Some(MetricValue {
                value: Some(metric_value::Value::IntValue(i)),
            }),
            properties: HashMap::new(),
            occurred_at: None,
        }
    }

    fn double_event(key: &str, d: f64) -> TrackEvent {
        TrackEvent {
            event_key: key.to_string(),
            context_type: "session".into(),
            context_key: "s1".into(),
            value: Some(MetricValue {
                value: Some(metric_value::Value::DoubleValue(d)),
            }),
            properties: HashMap::new(),
            occurred_at: None,
        }
    }

    async fn count_events_for_env(client: &clickhouse::Client, env_id: EnvironmentId) -> u64 {
        let sql = format!(
            "SELECT count() FROM events_v2 WHERE env_id = '{}'",
            env_id.as_uuid()
        );
        client.query(&sql).fetch_one::<u64>().await.unwrap_or(0)
    }

    async fn ensure_migrations() {
        let client = make_ch_client();
        stitchd_event_writer::migrations::run(&client)
            .await
            .expect("CH migrations must apply");
    }

    // -----------------------------------------------------------------------
    // env_id resolution
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn rejects_missing_env_id_metadata_and_body() {
        let state = TrackEventsState {
            event_def_repo: Arc::new(MockEventDefRepo::new(EnvironmentId::new(), vec![])),
            event_def_cache: EventDefinitionCache::new(),
            ch_client: Arc::new(make_ch_client()),
        };
        let req = Request::new(TrackEventsRequest {
            env_id: String::new(),
            events: vec![],
        });
        let err = handle_track_events(&state, req).await.unwrap_err();
        assert_eq!(err.code(), tonic::Code::Unauthenticated);
        assert!(err.message().contains("missing x-env-id"));
    }

    #[tokio::test]
    async fn rejects_invalid_env_id_metadata() {
        let state = TrackEventsState {
            event_def_repo: Arc::new(MockEventDefRepo::new(EnvironmentId::new(), vec![])),
            event_def_cache: EventDefinitionCache::new(),
            ch_client: Arc::new(make_ch_client()),
        };
        let mut req = Request::new(TrackEventsRequest {
            env_id: String::new(),
            events: vec![],
        });
        req.metadata_mut()
            .insert("x-env-id", MetadataValue::try_from("not-a-uuid").unwrap());
        let err = handle_track_events(&state, req).await.unwrap_err();
        assert_eq!(err.code(), tonic::Code::Unauthenticated);
    }

    #[tokio::test]
    async fn falls_back_to_body_env_id_when_metadata_missing() {
        let env_id = EnvironmentId::new();
        let state = make_state_with_ch(env_id, vec![("click_count", EventValueType::Int, false)]);
        let req = Request::new(TrackEventsRequest {
            env_id: env_id.as_uuid().to_string(),
            events: vec![],
        });
        let resp = handle_track_events(&state, req).await.unwrap().into_inner();
        assert_eq!(resp.accepted_count, 0);
        assert!(resp.rejected.is_empty());
    }

    // -----------------------------------------------------------------------
    // Empty batch
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_track_events_empty_batch() {
        let env_id = EnvironmentId::new();
        let state = make_state_with_ch(env_id, vec![]);
        let req = make_request(env_id, vec![]);
        let resp = handle_track_events(&state, req).await.unwrap().into_inner();
        assert_eq!(resp.accepted_count, 0);
        assert!(resp.rejected.is_empty());
    }

    // -----------------------------------------------------------------------
    // Valid batches → ClickHouse writes
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_track_events_accepts_valid_batch() {
        ensure_migrations().await;
        let env_id = EnvironmentId::new();
        let state = make_state_with_ch(
            env_id,
            vec![
                ("checkout_completed", EventValueType::Bool, false),
                ("revenue_usd", EventValueType::Double, false),
            ],
        );
        let events = vec![
            bool_event("checkout_completed", true),
            bool_event("checkout_completed", false),
            double_event("revenue_usd", 19.95),
        ];
        let req = make_request(env_id, events);
        let resp = handle_track_events(&state, req).await.unwrap().into_inner();

        assert_eq!(resp.accepted_count, 3);
        assert!(resp.rejected.is_empty());

        // Confirm the rows materialised in ClickHouse.
        let client = make_ch_client();
        let count = count_events_for_env(&client, env_id).await;
        assert_eq!(
            count, 3,
            "expected 3 rows in events_v2 for env {env_id}, got {count}"
        );
    }

    // -----------------------------------------------------------------------
    // Unknown / archived rejection
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_track_events_rejects_unknown_key() {
        let env_id = EnvironmentId::new();
        let state = make_state_with_ch(env_id, vec![("known", EventValueType::Int, false)]);
        let events = vec![int_event("totally_unknown", 1)];
        let req = make_request(env_id, events);
        let resp = handle_track_events(&state, req).await.unwrap().into_inner();

        assert_eq!(resp.accepted_count, 0);
        assert_eq!(resp.rejected.len(), 1);
        assert_eq!(resp.rejected[0].event_key, "totally_unknown");
        assert_eq!(resp.rejected[0].reason, "unknown_event_key");
    }

    #[tokio::test]
    async fn test_track_events_rejects_archived_key() {
        // Soft-deleted definitions surface from the PG repo as NotFound (the
        // SQL filters `deleted_at IS NULL`). Our handler maps both
        // missing-and-archived to "unknown_event_key" — the user-facing
        // distinction is moot from the SDK's perspective.
        let env_id = EnvironmentId::new();
        let state = make_state_with_ch(
            env_id,
            vec![
                ("active", EventValueType::Int, false),
                ("retired", EventValueType::Int, true),
            ],
        );
        let events = vec![int_event("retired", 99)];
        let req = make_request(env_id, events);
        let resp = handle_track_events(&state, req).await.unwrap().into_inner();

        assert_eq!(resp.accepted_count, 0);
        assert_eq!(resp.rejected.len(), 1);
        assert_eq!(resp.rejected[0].event_key, "retired");
        assert_eq!(resp.rejected[0].reason, "unknown_event_key");
    }

    // -----------------------------------------------------------------------
    // Partial batch — mix of valid + invalid; only valid rows reach CH.
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_track_events_partial_batch() {
        ensure_migrations().await;
        let env_id = EnvironmentId::new();
        let state = make_state_with_ch(
            env_id,
            vec![
                ("conversion", EventValueType::Bool, false),
                ("clicks", EventValueType::Int, false),
            ],
        );

        let events = vec![
            bool_event("conversion", true),                  // ok
            int_event("clicks", 5),                          // ok
            int_event("unknown_key", 1),                     // reject: unknown
            int_event("conversion", 7),                      // reject: type_mismatch
            TrackEvent {
                // reject: missing_value
                event_key: "clicks".into(),
                context_type: "user".into(),
                context_key: "u9".into(),
                value: None,
                properties: HashMap::new(),
                occurred_at: None,
            },
        ];
        let req = make_request(env_id, events);
        let resp = handle_track_events(&state, req).await.unwrap().into_inner();

        assert_eq!(resp.accepted_count, 2);
        assert_eq!(resp.rejected.len(), 3);

        let reasons: Vec<&str> = resp.rejected.iter().map(|r| r.reason.as_str()).collect();
        assert!(reasons.contains(&"unknown_event_key"));
        assert!(reasons.contains(&"value_type_mismatch"));
        assert!(reasons.contains(&"missing_value"));

        // Only the 2 valid rows should land in CH for this env.
        let client = make_ch_client();
        let count = count_events_for_env(&client, env_id).await;
        assert_eq!(count, 2, "expected exactly 2 valid rows in events_v2");
    }

    // -----------------------------------------------------------------------
    // occurred_at parsing
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn rejects_invalid_occurred_at() {
        let env_id = EnvironmentId::new();
        let state = make_state_with_ch(env_id, vec![("clicks", EventValueType::Int, false)]);
        let mut event = int_event("clicks", 1);
        event.occurred_at = Some("not-a-timestamp".to_string());
        let req = make_request(env_id, vec![event]);
        let resp = handle_track_events(&state, req).await.unwrap().into_inner();

        assert_eq!(resp.accepted_count, 0);
        assert_eq!(resp.rejected.len(), 1);
        assert_eq!(resp.rejected[0].reason, "invalid_occurred_at");
    }

    #[tokio::test]
    async fn accepts_valid_occurred_at() {
        ensure_migrations().await;
        let env_id = EnvironmentId::new();
        let state = make_state_with_ch(env_id, vec![("clicks", EventValueType::Int, false)]);
        let mut event = int_event("clicks", 1);
        event.occurred_at = Some("2026-05-01T12:34:56Z".to_string());
        let req = make_request(env_id, vec![event]);
        let resp = handle_track_events(&state, req).await.unwrap().into_inner();

        assert_eq!(resp.accepted_count, 1);
        assert!(resp.rejected.is_empty());
    }

    // -----------------------------------------------------------------------
    // Cache behaviour — repeated lookups for the same key hit cache once.
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn cache_coalesces_repeat_lookups_within_ttl() {
        ensure_migrations().await;
        let env_id = EnvironmentId::new();
        let repo = Arc::new(MockEventDefRepo::new(
            env_id,
            vec![("repeat_key", EventValueType::Int, false)],
        ));
        let state = TrackEventsState {
            event_def_repo: repo.clone(),
            event_def_cache: EventDefinitionCache::new(),
            ch_client: Arc::new(make_ch_client()),
        };

        // Two passes — second pass should be served entirely from cache.
        let events_pass = || {
            vec![
                int_event("repeat_key", 1),
                int_event("repeat_key", 2),
                int_event("repeat_key", 3),
            ]
        };
        let req1 = make_request(env_id, events_pass());
        handle_track_events(&state, req1).await.unwrap();

        let calls_after_first = repo.calls();
        let req2 = make_request(env_id, events_pass());
        handle_track_events(&state, req2).await.unwrap();

        let calls_after_second = repo.calls();
        assert_eq!(
            calls_after_first, calls_after_second,
            "cache should serve repeat lookups: pass-1={calls_after_first}, pass-2={calls_after_second}"
        );
        // And the first pass itself should coalesce its 3 events to ~1 lookup.
        assert!(
            calls_after_first <= 1,
            "expected ≤1 PG lookup across batch of 3, got {calls_after_first}"
        );
    }

    // -----------------------------------------------------------------------
    // Cache negative TTL — a NotFound is cached too, but expires.
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn cache_with_zero_ttl_propagates_repo_fixes() {
        // Custom TTL of 0 means each lookup misses; useful to verify the
        // cache doesn't pin a stale NotFound across calls.
        let env_id = EnvironmentId::new();
        let repo = Arc::new(MockEventDefRepo::new(env_id, vec![]));
        let state = TrackEventsState {
            event_def_repo: repo.clone(),
            event_def_cache: EventDefinitionCache::with_ttl(Duration::from_millis(1)),
            ch_client: Arc::new(make_ch_client()),
        };

        // First lookup: NotFound → rejected.
        let req1 = make_request(env_id, vec![int_event("future_key", 1)]);
        let resp1 = handle_track_events(&state, req1).await.unwrap().into_inner();
        assert_eq!(resp1.rejected.len(), 1);

        // Allow the TTL to elapse so the next lookup actually goes to the repo.
        tokio::time::sleep(Duration::from_millis(20)).await;

        // After TTL, a second lookup should still hit the repo at least once.
        let calls_after_first = repo.calls();
        let req2 = make_request(env_id, vec![int_event("future_key", 1)]);
        let _ = handle_track_events(&state, req2).await.unwrap();
        assert!(
            repo.calls() > calls_after_first,
            "TTL expiry should trigger fresh PG lookup"
        );
    }

    // -----------------------------------------------------------------------
    // Repo error propagation
    // -----------------------------------------------------------------------

    struct AlwaysFailingRepo;

    #[async_trait]
    impl EventDefinitionRepository for AlwaysFailingRepo {
        async fn find_by_id(
            &self,
            _id: EventDefinitionId,
        ) -> Result<EventDefinition, RepositoryError> {
            Err(RepositoryError::Unexpected(anyhow::anyhow!("db down")))
        }

        async fn find_by_key(
            &self,
            _key: &str,
            _environment_id: EnvironmentId,
        ) -> Result<EventDefinition, RepositoryError> {
            Err(RepositoryError::Unexpected(anyhow::anyhow!("db down")))
        }

        async fn list_by_environment(
            &self,
            _environment_id: EnvironmentId,
        ) -> Result<Vec<EventDefinition>, RepositoryError> {
            Err(RepositoryError::Unexpected(anyhow::anyhow!("db down")))
        }

        async fn create(&self, _def: &EventDefinition) -> Result<(), RepositoryError> {
            Ok(())
        }

        async fn update(&self, def: &EventDefinition) -> Result<EventDefinition, RepositoryError> {
            Ok(def.clone())
        }

        async fn soft_delete(&self, _id: EventDefinitionId) -> Result<(), RepositoryError> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn returns_internal_error_when_repo_fails() {
        let env_id = EnvironmentId::new();
        let state = TrackEventsState {
            event_def_repo: Arc::new(AlwaysFailingRepo),
            event_def_cache: EventDefinitionCache::new(),
            ch_client: Arc::new(make_ch_client()),
        };
        let req = make_request(env_id, vec![int_event("any_key", 1)]);
        let err = handle_track_events(&state, req).await.unwrap_err();
        assert_eq!(err.code(), tonic::Code::Internal);
        assert!(err.message().contains("db down"));
    }

    // -----------------------------------------------------------------------
    // RejectedEvent type — sanity check that reason discriminants compile.
    // (No assertions — just guards against accidental enum bag mistakes.)
    // -----------------------------------------------------------------------

    #[test]
    fn rejected_event_reasons_are_string_discriminants() {
        let r = RejectedEvent {
            event_key: "x".into(),
            reason: "unknown_event_key".into(),
        };
        assert_eq!(r.reason, "unknown_event_key");
    }
}
