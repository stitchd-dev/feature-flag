//! `SdkClient` — the public entry point for the Rust SDK.
//!
//! Phases 5 Tasks 7, 8, 9 of `sdk_rewrite_20260516`:
//!
//! - **Task 7** (`init`): validates config, makes first definition sync, spawns
//!   the three background tasks (poll, LRU refresh, event flush), returns
//!   `Arc<SdkClient>`.
//! - **Task 8** (`evaluate` / `evaluate_with_reasoning`): uses the local
//!   definition snapshot + LRU cache to evaluate one or more flag requests
//!   in-process, fetching list-segment membership on LRU miss, emitting
//!   one event per evaluated flag.
//! - **Task 9** (`shutdown`): signals all background tasks to stop and drains
//!   the event buffer before returning.
//!
//! ## Network implementations
//!
//! Three concrete transport impls live here alongside `SdkClient`:
//!
//! - [`GrpcDefinitionFetcher`] — calls `SdkService::SyncDefinitions` via tonic.
//! - [`HttpMembershipFetcher`] — calls `POST /v1/sdk/segments/list:batch` via reqwest.
//! - [`HttpEventSink`] — calls `POST /v1/sdk/events:batch` via reqwest.
//!
//! They are `pub(crate)` to let integration tests inject stubs; the stable
//! public API is `SdkClient::init(SdkConfig)`.

use async_trait::async_trait;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::warn;
use uuid::Uuid;

use stitchd_core::context::Context;
use stitchd_core::hashing::calculate_allocation;
use stitchd_core::id::SegmentId;
use stitchd_core::rule_engine::condition::Condition;
use stitchd_core::rule_engine::eval_expr::evaluate_expr;
use stitchd_core::rule_engine::types::{ConditionExpr, EvaluationInput, Rule};
use stitchd_core::segment::RuleBasedSegment;

use stitchd_proto::flags::v1::FeatureFlag;
use stitchd_proto::sdk::v1::SyncDefinitionsRequest;
use stitchd_proto::sdk::v1::sdk_service_client::SdkServiceClient;

use crate::config::SdkConfig;
use crate::error::{SdkError, TrackError};
use crate::event_buffer::{BufferedEvent, EventBuffer, EventBufferConfig, TypedValue};
use crate::events::{EventQueue, EventSink, FlagEvaluationEvent, FlushTask, ParameterValue};
use crate::lru::{ContextKey, MembershipCache, MembershipMap};
use crate::polling::{DefinitionFetcher, PollTask};
use crate::refresh::{MembershipBatchFetcher, RefreshTask};
use crate::snapshot::{DefinitionSnapshot, DefinitionStore, EventValueType};

// ============================================================================
// Public output types (Tasks 8)
// ============================================================================

/// A single flag-evaluation request.
pub struct EvalRequest {
    /// The flag's string key (e.g. `"checkout-flow"`).
    pub flag_key: String,
    /// Evaluation context (one `(type, key, params)` tuple per request).
    pub context: Context,
}

/// Outcome category of a completed evaluation.
#[derive(Debug, Clone, PartialEq)]
pub enum EvalOutcome {
    /// A targeting rule matched — the rule's 0-based index is included.
    Matched { rule_index: usize },
    /// No targeting rule matched; the flag's default rule fired.
    DefaultRule,
    /// Flag exists but is disabled; default variant returned.
    Disabled,
    /// Flag key not found in the current snapshot.
    FlagNotFound,
}

impl EvalOutcome {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Matched { .. } => "matched",
            Self::DefaultRule => "default_rule",
            Self::Disabled => "disabled",
            Self::FlagNotFound => "flag_not_found",
        }
    }
}

/// Result of a single flag evaluation (no reasoning).
pub struct EvalResult {
    pub flag_key: String,
    pub variant_key: String,
    pub variant_value: serde_json::Value,
    pub outcome: EvalOutcome,
}

/// Which rule matched and how (included in `evaluate_with_reasoning`).
#[derive(Debug, Clone)]
pub struct ReasoningTrace {
    pub outcome: EvalOutcome,
    /// 0-based index of the matched rule in `flag.rules`; `None` when
    /// outcome is `DefaultRule`, `Disabled`, or `FlagNotFound`.
    pub matched_rule_index: Option<usize>,
    /// Human-readable name of the matched rule (from `FlagRule.name`);
    /// `None` when no rule matched or the rule has no name.
    pub matched_rule_name: Option<String>,
}

/// Result of a single flag evaluation with full reasoning.
pub struct EvalResultWithReasoning {
    pub flag_key: String,
    pub variant_key: String,
    pub variant_value: serde_json::Value,
    pub outcome: EvalOutcome,
    pub reasoning: ReasoningTrace,
}

// ============================================================================
// GrpcDefinitionFetcher (Task 7)
// ============================================================================

/// Fetches definition snapshots from the gateway via tonic gRPC.
///
/// Used by both `SdkClient::init` (first sync) and `PollTask` (periodic
/// refreshes). Clones cheaply — the underlying tonic `Channel` is arc-backed.
pub(crate) struct GrpcDefinitionFetcher {
    channel: tonic::transport::Channel,
    sdk_key: tonic::metadata::MetadataValue<tonic::metadata::Ascii>,
}

impl GrpcDefinitionFetcher {
    pub(crate) fn new(channel: tonic::transport::Channel, sdk_key: &str) -> Result<Self, SdkError> {
        let meta = tonic::metadata::MetadataValue::try_from(sdk_key)
            .map_err(|_| SdkError::Config("sdk_key contains non-ASCII characters".into()))?;
        Ok(Self {
            channel,
            sdk_key: meta,
        })
    }
}

#[async_trait]
impl DefinitionFetcher for GrpcDefinitionFetcher {
    async fn fetch(&self) -> Result<DefinitionSnapshot, SdkError> {
        let mut client = SdkServiceClient::new(self.channel.clone());
        let mut req = tonic::Request::new(SyncDefinitionsRequest {});
        req.metadata_mut().insert("x-sdk-key", self.sdk_key.clone());
        let resp = client.sync_definitions(req).await.map_err(|s| {
            if s.code() == tonic::Code::Unauthenticated {
                SdkError::Auth(s.message().to_string())
            } else {
                SdkError::Network(format!("SyncDefinitions gRPC: {s}"))
            }
        })?;
        Ok(DefinitionSnapshot::from_proto(resp.into_inner()))
    }
}

// ============================================================================
// HttpMembershipFetcher (Task 7)
// ============================================================================

/// HTTP-backed list-segment membership fetcher.
///
/// Calls `POST /v1/sdk/segments/list:batch` on the gateway.
pub(crate) struct HttpMembershipFetcher {
    endpoint: String,
    sdk_key: String,
    client: reqwest::Client,
}

impl HttpMembershipFetcher {
    pub(crate) fn new(base_url: &str, sdk_key: impl Into<String>, client: reqwest::Client) -> Self {
        let endpoint = format!("{base_url}/v1/sdk/segments/list:batch");
        Self {
            endpoint,
            sdk_key: sdk_key.into(),
            client,
        }
    }
}

#[derive(Serialize)]
struct BatchMembershipBody {
    queries: Vec<BatchMembershipQuery>,
}

#[derive(Serialize)]
struct BatchMembershipQuery {
    context_type: String,
    context_key: String,
    segment_ids: Vec<String>,
}

#[derive(Deserialize)]
struct BatchMembershipResponse {
    results: Vec<MembershipResult>,
}

#[derive(Deserialize)]
struct MembershipResult {
    context_type: String,
    context_key: String,
    memberships: HashMap<String, bool>,
}

#[async_trait]
impl MembershipBatchFetcher for HttpMembershipFetcher {
    async fn fetch(
        &self,
        contexts: Vec<ContextKey>,
        segment_ids: Vec<String>,
    ) -> Result<Vec<MembershipMap>, SdkError> {
        let queries: Vec<BatchMembershipQuery> = contexts
            .iter()
            .map(|(ctx_type, ctx_key)| BatchMembershipQuery {
                context_type: ctx_type.clone(),
                context_key: ctx_key.clone(),
                segment_ids: segment_ids.clone(),
            })
            .collect();

        let body = BatchMembershipBody { queries };

        let resp = self
            .client
            .post(&self.endpoint)
            .header("x-sdk-key", &self.sdk_key)
            .json(&body)
            .send()
            .await
            .map_err(|e| SdkError::Network(format!("membership batch request: {e}")))?;

        if !resp.status().is_success() {
            return Err(SdkError::Network(format!(
                "membership batch: HTTP {}",
                resp.status()
            )));
        }

        let parsed: BatchMembershipResponse = resp
            .json()
            .await
            .map_err(|e| SdkError::Network(format!("membership batch parse: {e}")))?;

        // Re-order results to match the input `contexts` order.
        let mut result_map: HashMap<ContextKey, MembershipMap> = parsed
            .results
            .into_iter()
            .map(|r| ((r.context_type, r.context_key), r.memberships))
            .collect();

        Ok(contexts
            .into_iter()
            .map(|k| result_map.remove(&k).unwrap_or_default())
            .collect())
    }
}

// ============================================================================
// HttpEventSink (Task 7)
// ============================================================================

/// HTTP-backed event sink.
///
/// Calls `POST /v1/sdk/events:batch` on the gateway (202 Accepted, no body).
pub(crate) struct HttpEventSink {
    endpoint: String,
    sdk_key: String,
    client: reqwest::Client,
}

impl HttpEventSink {
    pub(crate) fn new(base_url: &str, sdk_key: impl Into<String>, client: reqwest::Client) -> Self {
        let endpoint = format!("{base_url}/v1/sdk/events:batch");
        Self {
            endpoint,
            sdk_key: sdk_key.into(),
            client,
        }
    }
}

/// Wire-format event body for `POST /v1/sdk/events:batch`.
#[derive(Serialize)]
struct EventBatchBody<'a> {
    events: Vec<EventBodyItem<'a>>,
}

#[derive(Serialize)]
struct EventBodyItem<'a> {
    flag_key: &'a str,
    #[serde(skip_serializing_if = "str::is_empty")]
    flag_id: &'a str,
    variant_key: &'a str,
    context_type: &'a str,
    context_key: &'a str,
    evaluated_at: &'a str,
    #[serde(skip_serializing_if = "str::is_empty")]
    matched_rule_id: &'a str,
    outcome: &'a str,
    reasoning_included: bool,
    context_parameters: HashMap<String, serde_json::Value>,
}

fn param_to_json(p: &ParameterValue) -> serde_json::Value {
    match p {
        ParameterValue::Bool(b) => serde_json::Value::Bool(*b),
        ParameterValue::Int(i) => serde_json::json!(i),
        ParameterValue::Double(d) => serde_json::json!(d),
        ParameterValue::String(s) => serde_json::Value::String(s.clone()),
        ParameterValue::Semver(s) => serde_json::Value::String(s.clone()),
    }
}

#[async_trait]
impl EventSink for HttpEventSink {
    async fn flush(&self, batch: Vec<FlagEvaluationEvent>) -> Result<(), SdkError> {
        let items: Vec<EventBodyItem<'_>> = batch
            .iter()
            .map(|e| EventBodyItem {
                flag_key: &e.flag_key,
                flag_id: &e.flag_id,
                variant_key: &e.variant_key,
                context_type: &e.context_type,
                context_key: &e.context_key,
                evaluated_at: &e.evaluated_at,
                matched_rule_id: &e.matched_rule_id,
                outcome: &e.outcome,
                reasoning_included: e.reasoning_included,
                context_parameters: e
                    .context_parameters
                    .iter()
                    .map(|(k, v)| (k.clone(), param_to_json(v)))
                    .collect(),
            })
            .collect();

        let body = EventBatchBody { events: items };

        let resp = self
            .client
            .post(&self.endpoint)
            .header("x-sdk-key", &self.sdk_key)
            .json(&body)
            .send()
            .await
            .map_err(|e| SdkError::Network(format!("event flush: {e}")))?;

        if resp.status().is_success() || resp.status().as_u16() == 202 {
            Ok(())
        } else {
            Err(SdkError::Network(format!(
                "event flush: HTTP {}",
                resp.status()
            )))
        }
    }
}

// ============================================================================
// SdkClient (Tasks 7, 8, 9)
// ============================================================================

/// The main SDK entry point. Construct via [`SdkClient::init`].
///
/// Cheaply cloneable via `Arc<SdkClient>`. All public methods take `&self`.
pub struct SdkClient {
    definition_store: DefinitionStore,
    membership_cache: MembershipCache,
    event_queue: EventQueue,
    /// On-demand membership fetcher used by `evaluate()` on LRU miss.
    /// Shared with `RefreshTask` (same channel/auth).
    membership_fetcher: Arc<dyn MembershipBatchFetcher>,
    /// Client-side track-event buffer (Phase 5). `None` when the
    /// `SdkConfig` is built without a gateway URL we can POST to (e.g.
    /// pure in-memory test fixtures via the `test-util` feature). In
    /// production builds — i.e. anything coming through `SdkClient::init`
    /// — this is `Some(_)` and powers `Client::track()`.
    event_buffer: Option<Arc<EventBuffer>>,
    poll_task: Mutex<Option<PollTask>>,
    refresh_task: Mutex<Option<RefreshTask>>,
    flush_task: Mutex<Option<FlushTask>>,
}

impl std::fmt::Debug for SdkClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SdkClient")
            .field("definition_store", &self.definition_store)
            .field(
                "membership_cache_entries",
                &self.membership_cache.entry_count(),
            )
            .field("event_queue_len", &self.event_queue.len())
            .finish_non_exhaustive()
    }
}

impl SdkClient {
    // ── init (Task 7) ────────────────────────────────────────────────────────

    /// Bootstrap the SDK client.
    ///
    /// 1. Validates `config`.
    /// 2. Connects to the gateway gRPC port and calls `SyncDefinitions` once
    ///    synchronously (blocks until success or returns an error — never
    ///    returns with an empty snapshot).
    /// 3. Spawns the three background tasks (poll, LRU refresh, event flush).
    /// 4. Returns `Arc<SdkClient>`.
    ///
    /// # Errors
    ///
    /// - [`SdkError::Config`] — config validation failed.
    /// - [`SdkError::Network`] — couldn't connect or first sync failed.
    /// - [`SdkError::Auth`] — SDK key rejected by the gateway.
    pub async fn init(config: SdkConfig) -> Result<Arc<Self>, SdkError> {
        config.validate()?;

        // ── Build shared HTTP client ──────────────────────────────────────
        let http_client = reqwest::Client::builder()
            .timeout(config.request_timeout)
            .build()
            .map_err(|e| SdkError::Config(format!("HTTP client build: {e}")))?;

        // ── Build gRPC channel ────────────────────────────────────────────
        let grpc_uri = grpc_uri_from_config(&config)?;
        let channel = tonic::transport::Channel::from_shared(grpc_uri)
            .map_err(|e| SdkError::Config(format!("invalid gRPC URI: {e}")))?
            .connect()
            .await
            .map_err(|e| SdkError::Network(format!("gRPC connect: {e}")))?;

        let fetcher: Arc<GrpcDefinitionFetcher> =
            Arc::new(GrpcDefinitionFetcher::new(channel, &config.sdk_key)?);

        // ── First sync (fail-fast) ────────────────────────────────────────
        let initial_snapshot = fetcher.fetch().await?;
        let definition_store = DefinitionStore::from_snapshot(initial_snapshot);

        // ── LRU cache ─────────────────────────────────────────────────────
        let membership_cache = MembershipCache::new(config.lru_max_entries);

        // ── Membership fetcher (shared: on-demand + refresh task) ─────────
        let membership_fetcher: Arc<dyn MembershipBatchFetcher> = Arc::new(
            HttpMembershipFetcher::new(&config.gateway_url, &config.sdk_key, http_client.clone()),
        );

        // ── Event queue + flush task ──────────────────────────────────────
        let event_queue = EventQueue::new(config.event_buffer_capacity, config.event_batch_size);
        let sink: Arc<dyn EventSink> = Arc::new(HttpEventSink::new(
            &config.gateway_url,
            &config.sdk_key,
            http_client.clone(),
        ));
        let flush_task = FlushTask::spawn(event_queue.clone(), sink, config.event_flush_interval);

        // ── Track-event buffer (Phase 5) ──────────────────────────────────
        //
        // Distinct from the flag-evaluation event_queue above — that one
        // ships `FlagEvaluationEvent` to `/v1/sdk/events:batch`; this one
        // ships caller-supplied `BufferedEvent` to `/v1/events/track`.
        let event_buffer_config = EventBufferConfig {
            flush_at_size: config.event_batch_size,
            flush_interval: config.event_flush_interval,
            max_retries: 3,
            backoff_base: std::time::Duration::from_millis(200),
            gateway_base_url: config.gateway_url.clone(),
            sdk_key: config.sdk_key.clone(),
        };
        let event_buffer = EventBuffer::with_client(event_buffer_config, http_client.clone());

        // ── Background poll task ──────────────────────────────────────────
        let poll_task = PollTask::spawn(
            fetcher as Arc<dyn DefinitionFetcher>,
            definition_store.clone(),
            config.definition_poll_interval,
        );

        // ── Background LRU refresh task ───────────────────────────────────
        let refresh_task = RefreshTask::spawn(
            Arc::clone(&membership_fetcher),
            membership_cache.clone(),
            definition_store.clone(),
            config.list_segment_refresh_interval,
        );

        Ok(Arc::new(Self {
            definition_store,
            membership_cache,
            event_queue,
            membership_fetcher,
            event_buffer: Some(event_buffer),
            poll_task: Mutex::new(Some(poll_task)),
            refresh_task: Mutex::new(Some(refresh_task)),
            flush_task: Mutex::new(Some(flush_task)),
        }))
    }

    // ── evaluate (Task 8) ────────────────────────────────────────────────────

    /// Evaluate a batch of flags using the local definition snapshot.
    ///
    /// Each request is independent — no cross-request state. On LRU miss for a
    /// list-segment, the SDK makes a synchronous HTTP call to the gateway to
    /// fetch membership, inserts the result into the LRU, then continues
    /// evaluation. The call is fast on LRU hit.
    pub async fn evaluate(&self, requests: &[EvalRequest]) -> Vec<EvalResult> {
        let snapshot = self.definition_store.load();
        let mut results = Vec::with_capacity(requests.len());
        for req in requests {
            let (variant_key, variant_value, outcome, _trace) = self
                .evaluate_inner(&snapshot, &req.flag_key, &req.context, false)
                .await;
            let event = build_event(
                &req.flag_key,
                find_flag_id(&snapshot, &req.flag_key),
                &variant_key,
                outcome.as_str(),
                "",
                false,
                &req.context,
            );
            self.event_queue.send(event);
            results.push(EvalResult {
                flag_key: req.flag_key.clone(),
                variant_key,
                variant_value,
                outcome,
            });
        }
        results
    }

    /// Evaluate a batch of flags with full reasoning traces.
    pub async fn evaluate_with_reasoning(
        &self,
        requests: &[EvalRequest],
    ) -> Vec<EvalResultWithReasoning> {
        let snapshot = self.definition_store.load();
        let mut results = Vec::with_capacity(requests.len());
        for req in requests {
            let (variant_key, variant_value, outcome, trace) = self
                .evaluate_inner(&snapshot, &req.flag_key, &req.context, true)
                .await;
            let reasoning = trace.unwrap_or_else(|| ReasoningTrace {
                outcome: outcome.clone(),
                matched_rule_index: None,
                matched_rule_name: None,
            });
            let event = build_event(
                &req.flag_key,
                find_flag_id(&snapshot, &req.flag_key),
                &variant_key,
                outcome.as_str(),
                "",
                true,
                &req.context,
            );
            self.event_queue.send(event);
            results.push(EvalResultWithReasoning {
                flag_key: req.flag_key.clone(),
                variant_key,
                variant_value,
                outcome,
                reasoning,
            });
        }
        results
    }

    // ── track (Phase 5 Task 5.2) ─────────────────────────────────────────────

    /// Enqueue one track-event for delivery to the gateway's
    /// `/v1/events/track` endpoint.
    ///
    /// **Validation (per spec F2.4):** before the event is enqueued, the
    /// SDK checks the locally cached `event_definitions`:
    ///
    /// - If `event_key` is not registered → emit `tracing::warn!`, do
    ///   nothing, return `Ok(())`. Rejections at this layer are
    ///   fire-and-forget; they NEVER propagate as `Err`.
    /// - If `value` is `Some(_)` and its variant doesn't match the
    ///   registered `EventValueType` → same warn + skip.
    /// - Otherwise → assemble a [`BufferedEvent`] and call
    ///   `event_buffer.enqueue()`.
    ///
    /// **Buffer absent.** A client constructed via the `test-util`
    /// helpers (or any future builder path that doesn't supply an event
    /// buffer) silently no-ops. Production clients built via
    /// `SdkClient::init` always have a buffer.
    ///
    /// # Errors
    ///
    /// - [`TrackError::State`] — placeholder for post-shutdown calls.
    ///   The current implementation never returns this; it's reserved
    ///   so the signature is stable across Phase 5 Task 5.3
    ///   (`Client::shutdown()` integration).
    pub async fn track(
        &self,
        event_key: &str,
        context: &Context,
        value: Option<TypedValue>,
        properties: Option<HashMap<String, String>>,
    ) -> Result<(), TrackError> {
        // Look up the cached event-definition. If absent or mismatched,
        // warn + skip (per spec F2.4).
        let snapshot = self.definition_store.load();
        match snapshot.event_definition(event_key) {
            None => {
                warn!(
                    event_key,
                    "track: unknown event_key; skipping (no matching event definition in local snapshot)"
                );
                return Ok(());
            }
            Some(registered) => {
                if let Some(ref v) = value {
                    let actual = EventValueType::of(v);
                    if actual != registered {
                        warn!(
                            event_key,
                            ?registered,
                            ?actual,
                            "track: value type does not match registered event definition; skipping"
                        );
                        return Ok(());
                    }
                }
            }
        }

        // No buffer (test-util fixtures) — accept and drop silently.
        let Some(buffer) = self.event_buffer.as_ref() else {
            return Ok(());
        };

        let buffered = BufferedEvent {
            event_key: event_key.to_string(),
            context_type: context.context_type.clone(),
            context_key: context.key.clone(),
            value,
            properties,
            // SDK-local clock; gateway re-stamps if absent.
            occurred_at: Some(Utc::now()),
        };
        buffer.enqueue(buffered);
        Ok(())
    }

    /// Synchronous predicate — does the locally cached snapshot contain
    /// an event definition for `event_key`?
    ///
    /// Implements spec F2.5. Useful for pre-flight checks before
    /// constructing a value, e.g. in branchy code paths where the value
    /// is expensive to compute.
    #[must_use]
    pub fn is_event_registered(&self, event_key: &str) -> bool {
        self.definition_store
            .load()
            .event_definition(event_key)
            .is_some()
    }

    // ── shutdown (Task 9) ────────────────────────────────────────────────────

    /// Gracefully shut down the SDK client.
    ///
    /// Stops the three background tasks and drains the event buffer before
    /// returning. After this call the `SdkClient` should be dropped.
    pub async fn shutdown(self: Arc<Self>) {
        // Stop poll task first (no more snapshot swaps after this).
        if let Some(task) = self.poll_task.lock().await.take() {
            task.shutdown().await;
        }
        // Stop LRU refresh.
        if let Some(task) = self.refresh_task.lock().await.take() {
            task.shutdown().await;
        }
        // Flush task last — drains the event buffer before exiting.
        if let Some(task) = self.flush_task.lock().await.take() {
            task.shutdown().await;
        }
    }

    // ── Internal evaluation (Task 8) ─────────────────────────────────────────

    /// Core evaluation path.
    ///
    /// Returns `(variant_key, variant_value, outcome, Option<ReasoningTrace>)`.
    /// `build_reasoning` controls whether the third return value is `Some`.
    async fn evaluate_inner(
        &self,
        snapshot: &DefinitionSnapshot,
        flag_key: &str,
        context: &Context,
        build_reasoning: bool,
    ) -> (
        String,
        serde_json::Value,
        EvalOutcome,
        Option<ReasoningTrace>,
    ) {
        let Some(flag) = snapshot.flag(flag_key) else {
            let trace = build_reasoning.then_some(ReasoningTrace {
                outcome: EvalOutcome::FlagNotFound,
                matched_rule_index: None,
                matched_rule_name: None,
            });
            return (
                String::new(),
                serde_json::Value::Null,
                EvalOutcome::FlagNotFound,
                trace,
            );
        };

        if flag.archived {
            let (key, val) = default_variant(flag);
            let trace = build_reasoning.then_some(ReasoningTrace {
                outcome: EvalOutcome::FlagNotFound,
                matched_rule_index: None,
                matched_rule_name: None,
            });
            return (key, val, EvalOutcome::FlagNotFound, trace);
        }

        if !flag.enabled {
            let (key, val) = default_variant(flag);
            let trace = build_reasoning.then_some(ReasoningTrace {
                outcome: EvalOutcome::Disabled,
                matched_rule_index: None,
                matched_rule_name: None,
            });
            return (key, val, EvalOutcome::Disabled, trace);
        }

        // ── Parse rule conditions ─────────────────────────────────────────
        let parsed_rules: Vec<(usize, ConditionExpr, &stitchd_proto::flags::v1::FlagRule)> = flag
            .rules
            .iter()
            .enumerate()
            .filter_map(|(i, rule)| {
                let cond: ConditionExpr = serde_json::from_slice(&rule.rule_payload).ok()?;
                Some((i, cond, rule))
            })
            .collect();

        // ── Collect all referenced segment IDs ───────────────────────────
        let mut all_seg_ids: HashSet<SegmentId> = HashSet::new();
        for (_, cond, _) in &parsed_rules {
            collect_segment_ids(cond, &mut all_seg_ids);
        }

        // ── Resolve segment membership ────────────────────────────────────
        let resolved_segments = self.resolve_segments(snapshot, context, &all_seg_ids).await;

        // ── Evaluate rules ────────────────────────────────────────────────
        let input = EvaluationInput {
            contexts: std::slice::from_ref(context),
            resolved_segments,
            evaluated_flags: HashMap::new(),
        };

        let env_id = snapshot.environment_id();

        for (idx, cond, rule) in &parsed_rules {
            match evaluate_expr(cond, &input) {
                Ok(true) => {
                    use stitchd_proto::flags::v1::flag_rule::Output;
                    let (variant_key, variant_value) = match &rule.output {
                        Some(Output::VariantKey(k)) => {
                            let val = lookup_variant_value(flag, k);
                            (k.clone(), val)
                        }
                        Some(Output::Allocation(alloc)) => {
                            let key = compute_allocation(alloc, context, env_id, flag_key);
                            let val = lookup_variant_value(flag, &key);
                            (key, val)
                        }
                        None => continue,
                    };
                    let rule_name = if rule.name.is_empty() {
                        None
                    } else {
                        Some(rule.name.clone())
                    };
                    let outcome = EvalOutcome::Matched { rule_index: *idx };
                    let trace = build_reasoning.then_some(ReasoningTrace {
                        outcome: outcome.clone(),
                        matched_rule_index: Some(*idx),
                        matched_rule_name: rule_name,
                    });
                    return (variant_key, variant_value, outcome, trace);
                }
                Ok(false) => {}
                Err(e) => {
                    warn!(rule_index = idx, error = %e, flag_key, "rule evaluation error; skipping rule");
                }
            }
        }

        // ── Default rule ──────────────────────────────────────────────────
        let (key, val) = default_variant(flag);
        let trace = build_reasoning.then_some(ReasoningTrace {
            outcome: EvalOutcome::DefaultRule,
            matched_rule_index: None,
            matched_rule_name: None,
        });
        (key, val, EvalOutcome::DefaultRule, trace)
    }

    /// Resolve membership for `segment_ids` given a single evaluation context.
    ///
    /// - Rule-based segments: evaluated in-process from the snapshot.
    /// - List-based segments: checked in LRU; fetched from gateway on miss.
    async fn resolve_segments(
        &self,
        snapshot: &DefinitionSnapshot,
        context: &Context,
        segment_ids: &HashSet<SegmentId>,
    ) -> HashSet<SegmentId> {
        if segment_ids.is_empty() {
            return HashSet::new();
        }

        let mut resolved = HashSet::with_capacity(segment_ids.len());
        let mut list_ids_to_fetch: Vec<String> = Vec::new();

        // First pass: classify each segment and handle rule-based ones immediately.
        for &seg_id in segment_ids {
            let id_str = seg_id.as_uuid().to_string();

            if let Some(rule_seg) = snapshot.rule_segment(&id_str) {
                // Evaluate the rule-based segment in-process.
                let rules: Vec<Rule> =
                    serde_json::from_slice(&rule_seg.rule_payload).unwrap_or_default();
                let seg = RuleBasedSegment { id: seg_id, rules };
                if seg
                    .evaluate(std::slice::from_ref(context))
                    .map(|r| r.matched)
                    .unwrap_or(false)
                {
                    resolved.insert(seg_id);
                }
            } else if snapshot.list_segment(&id_str).is_some() {
                // Check LRU first.
                if let Some(map) = self
                    .membership_cache
                    .get(&context.context_type, &context.key)
                {
                    if *map.get(&id_str).unwrap_or(&false) {
                        resolved.insert(seg_id);
                    }
                } else {
                    // LRU miss — need to fetch.
                    list_ids_to_fetch.push(id_str);
                }
            }
        }

        // Second pass: batch-fetch any list-segment IDs not in LRU.
        if !list_ids_to_fetch.is_empty() {
            let ctx_key: ContextKey = (context.context_type.clone(), context.key.clone());
            match self
                .membership_fetcher
                .fetch(vec![ctx_key.clone()], list_ids_to_fetch.clone())
                .await
            {
                Ok(mut maps) if !maps.is_empty() => {
                    let membership = maps.remove(0);
                    // Populate resolved set from fetched memberships.
                    for id_str in &list_ids_to_fetch {
                        if *membership.get(id_str).unwrap_or(&false) {
                            if let Ok(uuid) = Uuid::parse_str(id_str) {
                                resolved.insert(SegmentId::from_uuid(uuid));
                            }
                        }
                    }
                    // Insert into LRU for future evaluations.
                    self.membership_cache
                        .insert(&context.context_type, &context.key, membership);
                }
                Ok(_) => {}
                Err(e) => {
                    warn!(
                        error = %e,
                        context_type = context.context_type,
                        context_key = context.key,
                        "on-demand list-segment fetch failed; treating as non-member"
                    );
                }
            }
        }

        resolved
    }
}

// ============================================================================
// Helpers
// ============================================================================

/// Build the gRPC URI (scheme + host + grpc_port) from `SdkConfig`.
fn grpc_uri_from_config(config: &SdkConfig) -> Result<String, SdkError> {
    // Parse the base URL to extract the scheme and host.
    let url = reqwest::Url::parse(&config.gateway_url)
        .map_err(|e| SdkError::Config(format!("invalid gateway_url: {e}")))?;
    let scheme = url.scheme();
    let host = url
        .host_str()
        .ok_or_else(|| SdkError::Config("gateway_url has no host".into()))?;
    Ok(format!("{scheme}://{host}:{}", config.gateway_grpc_port))
}

/// Recursively collect all segment IDs referenced in a condition expression.
fn collect_segment_ids(expr: &ConditionExpr, ids: &mut HashSet<SegmentId>) {
    match expr {
        ConditionExpr::Leaf(Condition::InSegment(id)) => {
            ids.insert(*id);
        }
        ConditionExpr::Leaf(Condition::NotInSegment(id)) => {
            ids.insert(*id);
        }
        ConditionExpr::Leaf(_) => {}
        ConditionExpr::And(children) | ConditionExpr::Or(children) => {
            for child in children {
                collect_segment_ids(child, ids);
            }
        }
        ConditionExpr::Not(inner) => {
            collect_segment_ids(inner, ids);
        }
    }
}

/// Return `(variant_key, variant_value)` for the flag's default variant.
/// Returns `("", null)` if no default is configured.
fn default_variant(flag: &FeatureFlag) -> (String, serde_json::Value) {
    if flag.default_variant_key.is_empty() {
        return (String::new(), serde_json::Value::Null);
    }
    let val = lookup_variant_value(flag, &flag.default_variant_key);
    (flag.default_variant_key.clone(), val)
}

/// Look up a variant's JSON value by key. Returns `null` if not found.
fn lookup_variant_value(flag: &FeatureFlag, variant_key: &str) -> serde_json::Value {
    flag.variants
        .iter()
        .find(|v| v.key == variant_key)
        .and_then(|v| v.value.as_ref())
        .map(proto_variant_value_to_json)
        .unwrap_or(serde_json::Value::Null)
}

/// Convert a proto `VariantValue` to `serde_json::Value`.
fn proto_variant_value_to_json(v: &stitchd_proto::flags::v1::VariantValue) -> serde_json::Value {
    use stitchd_proto::flags::v1::variant_value::Value;
    match &v.value {
        Some(Value::BoolValue(b)) => serde_json::Value::Bool(*b),
        Some(Value::IntValue(i)) => serde_json::json!(i),
        Some(Value::DoubleValue(d)) => serde_json::json!(d),
        Some(Value::StringValue(s)) => serde_json::Value::String(s.clone()),
        Some(Value::JsonValue(s)) => {
            serde_json::from_str(s).unwrap_or_else(|_| serde_json::Value::String(s.clone()))
        }
        None => serde_json::Value::Null,
    }
}

/// Compute percentage allocation variant key.
///
/// Replicates the hash algorithm from `stitchd-core::hashing::calculate_allocation`:
/// `hash(flag_key + env_id + sorted_targets)`, mapped to bucket 0–999.
fn compute_allocation(
    alloc: &stitchd_proto::flags::v1::PercentageAllocation,
    context: &Context,
    env_id: &str,
    flag_key: &str,
) -> String {
    // Build target values, sorted by context_type for determinism.
    let mut specs: Vec<(&String, &stitchd_proto::flags::v1::ContextHashSpec)> =
        alloc.context_hash_specs.iter().collect();
    specs.sort_by_key(|(k, _)| k.as_str());

    let mut target_values: Vec<String> = Vec::new();
    for (ctx_type, spec) in specs {
        // Only apply this spec if the request context matches the type.
        if context.context_type != *ctx_type {
            continue;
        }
        if spec.parameter_names.is_empty() {
            target_values.push(format!("{}{}", ctx_type, context.key));
        } else {
            for param in &spec.parameter_names {
                if let Some(val) = context.parameters.get(param) {
                    target_values.push(format!("{}{}{}", ctx_type, param, val));
                }
            }
        }
    }

    let percentage = calculate_allocation(flag_key, env_id, &target_values);
    let bucket = ((percentage * 10.0).floor() as u32).min(999);

    let mut cumulative: u32 = 0;
    for b in &alloc.buckets {
        cumulative += b.weight_milli;
        if bucket < cumulative {
            return b.variant_key.clone();
        }
    }

    // Fallback: first bucket (handles rounding edge cases)
    alloc
        .buckets
        .first()
        .map(|b| b.variant_key.clone())
        .unwrap_or_default()
}

/// Find the flag_id UUID for a given flag_key in the snapshot.
fn find_flag_id<'a>(snapshot: &'a DefinitionSnapshot, flag_key: &str) -> &'a str {
    snapshot
        .flag(flag_key)
        .map(|f| f.flag_id.as_str())
        .unwrap_or("")
}

/// Build a `FlagEvaluationEvent` from evaluation output.
fn build_event(
    flag_key: &str,
    flag_id: &str,
    variant_key: &str,
    outcome: &str,
    matched_rule_id: &str,
    reasoning_included: bool,
    context: &Context,
) -> FlagEvaluationEvent {
    let evaluated_at = Utc::now().format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string();

    // Build public (non-private) context parameters for the event.
    let params: HashMap<String, ParameterValue> = context
        .parameters
        .iter()
        .filter(|(k, _)| !context.is_private(k))
        .map(|(k, v)| {
            let pv = match v {
                stitchd_core::context::ParameterValue::Bool(b) => ParameterValue::Bool(*b),
                stitchd_core::context::ParameterValue::Int(i) => ParameterValue::Int(*i),
                stitchd_core::context::ParameterValue::Double(d) => ParameterValue::Double(*d),
                stitchd_core::context::ParameterValue::SemVer(s) => {
                    ParameterValue::Semver(s.to_string())
                }
                stitchd_core::context::ParameterValue::Str(s) => ParameterValue::String(s.clone()),
            };
            (k.clone(), pv)
        })
        .collect();

    FlagEvaluationEvent {
        flag_key: flag_key.to_string(),
        flag_id: flag_id.to_string(),
        variant_key: variant_key.to_string(),
        context_type: context.context_type.clone(),
        context_key: context.key.clone(),
        evaluated_at,
        matched_rule_id: matched_rule_id.to_string(),
        outcome: outcome.to_string(),
        reasoning_included,
        context_parameters: params,
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex as StdMutex;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    use stitchd_core::context::ParameterValue as CoreParam;
    use stitchd_core::id::RuleId;
    use stitchd_core::rule_engine::condition::Condition;
    use stitchd_core::rule_engine::types::{ConditionExpr, Rule, RuleOutput};
    use stitchd_proto::flags::v1::flag_rule::Output;
    use stitchd_proto::flags::v1::variant_value::Value as VVal;
    use stitchd_proto::flags::v1::{FeatureFlag, FlagRule as ProtoFlagRule, Variant, VariantValue};
    use stitchd_proto::sdk::v1::SyncDefinitionsResponse;
    use stitchd_proto::segments::v1::{ListSegmentMeta, RuleSegment};

    // ── Helpers ──────────────────────────────────────────────────────────────

    fn bool_variant(key: &str, value: bool) -> Variant {
        Variant {
            key: key.to_string(),
            value: Some(VariantValue {
                value: Some(VVal::BoolValue(value)),
            }),
        }
    }

    fn string_variant(key: &str, value: &str) -> Variant {
        Variant {
            key: key.to_string(),
            value: Some(VariantValue {
                value: Some(VVal::StringValue(value.to_string())),
            }),
        }
    }

    fn simple_rule(condition: ConditionExpr, variant_key: &str) -> ProtoFlagRule {
        ProtoFlagRule {
            rule_payload: serde_json::to_vec(&condition).unwrap(),
            output: Some(Output::VariantKey(variant_key.to_string())),
            name: String::new(),
        }
    }

    fn disabled_flag(key: &str) -> FeatureFlag {
        FeatureFlag {
            key: key.to_string(),
            enabled: false,
            variants: vec![bool_variant("false", false)],
            default_variant_key: "false".to_string(),
            ..Default::default()
        }
    }

    fn simple_bool_flag(key: &str) -> FeatureFlag {
        FeatureFlag {
            key: key.to_string(),
            enabled: true,
            variants: vec![bool_variant("true", true), bool_variant("false", false)],
            default_variant_key: "true".to_string(),
            ..Default::default()
        }
    }

    fn sdk_client_with_snapshot(snapshot: DefinitionSnapshot) -> Arc<SdkClient> {
        let definition_store = DefinitionStore::from_snapshot(snapshot);
        let membership_cache = MembershipCache::new(100);
        let event_queue = EventQueue::new(1000, 100);

        let sink: Arc<dyn EventSink> = Arc::new(NoopSink);
        let flush_task = FlushTask::spawn(event_queue.clone(), sink, Duration::from_secs(60));

        let membership_fetcher: Arc<dyn MembershipBatchFetcher> = Arc::new(NoopMembershipFetcher);
        let poll_fetcher: Arc<dyn DefinitionFetcher> = Arc::new(NoopFetcher);
        let poll_task = PollTask::spawn(
            poll_fetcher,
            definition_store.clone(),
            Duration::from_secs(60),
        );
        let refresh_task = RefreshTask::spawn(
            Arc::clone(&membership_fetcher),
            membership_cache.clone(),
            definition_store.clone(),
            Duration::from_secs(60),
        );

        Arc::new(SdkClient {
            definition_store,
            membership_cache,
            event_queue,
            membership_fetcher,
            event_buffer: None,
            poll_task: Mutex::new(Some(poll_task)),
            refresh_task: Mutex::new(Some(refresh_task)),
            flush_task: Mutex::new(Some(flush_task)),
        })
    }

    fn sdk_client_with_membership_fetcher(
        snapshot: DefinitionSnapshot,
        fetcher: Arc<dyn MembershipBatchFetcher>,
    ) -> Arc<SdkClient> {
        let definition_store = DefinitionStore::from_snapshot(snapshot);
        let membership_cache = MembershipCache::new(100);
        let event_queue = EventQueue::new(1000, 100);

        let sink: Arc<dyn EventSink> = Arc::new(NoopSink);
        let flush_task = FlushTask::spawn(event_queue.clone(), sink, Duration::from_secs(60));

        let poll_fetcher: Arc<dyn DefinitionFetcher> = Arc::new(NoopFetcher);
        let poll_task = PollTask::spawn(
            poll_fetcher,
            definition_store.clone(),
            Duration::from_secs(60),
        );
        let refresh_task = RefreshTask::spawn(
            Arc::clone(&fetcher),
            membership_cache.clone(),
            definition_store.clone(),
            Duration::from_secs(60),
        );

        Arc::new(SdkClient {
            definition_store,
            membership_cache,
            event_queue,
            membership_fetcher: fetcher,
            event_buffer: None,
            poll_task: Mutex::new(Some(poll_task)),
            refresh_task: Mutex::new(Some(refresh_task)),
            flush_task: Mutex::new(Some(flush_task)),
        })
    }

    // ── Stub impls ────────────────────────────────────────────────────────────

    struct NoopSink;
    #[async_trait]
    impl EventSink for NoopSink {
        async fn flush(&self, _batch: Vec<FlagEvaluationEvent>) -> Result<(), SdkError> {
            Ok(())
        }
    }

    struct NoopFetcher;
    #[async_trait]
    impl DefinitionFetcher for NoopFetcher {
        async fn fetch(&self) -> Result<DefinitionSnapshot, SdkError> {
            Ok(DefinitionSnapshot::default())
        }
    }

    struct NoopMembershipFetcher;
    #[async_trait]
    impl MembershipBatchFetcher for NoopMembershipFetcher {
        async fn fetch(
            &self,
            contexts: Vec<ContextKey>,
            _segment_ids: Vec<String>,
        ) -> Result<Vec<MembershipMap>, SdkError> {
            Ok(contexts.iter().map(|_| HashMap::new()).collect())
        }
    }

    /// Recording membership fetcher — returns programmed memberships and counts calls.
    struct RecordingMembershipFetcher {
        calls: AtomicUsize,
        memberships: StdMutex<HashMap<String, bool>>,
    }

    impl RecordingMembershipFetcher {
        fn new(memberships: HashMap<String, bool>) -> Arc<Self> {
            Arc::new(Self {
                calls: AtomicUsize::new(0),
                memberships: StdMutex::new(memberships),
            })
        }
        fn call_count(&self) -> usize {
            self.calls.load(Ordering::SeqCst)
        }
    }

    #[async_trait]
    impl MembershipBatchFetcher for RecordingMembershipFetcher {
        async fn fetch(
            &self,
            contexts: Vec<ContextKey>,
            _segment_ids: Vec<String>,
        ) -> Result<Vec<MembershipMap>, SdkError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let m = self.memberships.lock().unwrap().clone();
            Ok(contexts.iter().map(|_| m.clone()).collect())
        }
    }

    // ── collect_segment_ids ──────────────────────────────────────────────────

    #[test]
    fn collect_segment_ids_from_leaf() {
        let seg = SegmentId::new();
        let expr = ConditionExpr::Leaf(Condition::InSegment(seg));
        let mut ids = HashSet::new();
        collect_segment_ids(&expr, &mut ids);
        assert!(ids.contains(&seg));
    }

    #[test]
    fn collect_segment_ids_from_nested_and_or() {
        let s1 = SegmentId::new();
        let s2 = SegmentId::new();
        let expr = ConditionExpr::And(vec![
            ConditionExpr::Leaf(Condition::InSegment(s1)),
            ConditionExpr::Or(vec![ConditionExpr::Leaf(Condition::NotInSegment(s2))]),
        ]);
        let mut ids = HashSet::new();
        collect_segment_ids(&expr, &mut ids);
        assert!(ids.contains(&s1));
        assert!(ids.contains(&s2));
    }

    #[test]
    fn collect_segment_ids_non_segment_condition_empty() {
        let expr = ConditionExpr::Leaf(Condition::Eq {
            context_type: "user".into(),
            param: "plan".into(),
            value: stitchd_core::context::ParameterValue::Str("pro".into()),
        });
        let mut ids = HashSet::new();
        collect_segment_ids(&expr, &mut ids);
        assert!(ids.is_empty());
    }

    // ── grpc_uri_from_config ─────────────────────────────────────────────────

    #[test]
    fn grpc_uri_replaces_port() {
        let cfg = SdkConfig {
            gateway_url: "http://localhost:8081".into(),
            sdk_key: "12345678".into(),
            gateway_grpc_port: 50050,
            ..SdkConfig::new("http://localhost:8081", "12345678")
        };
        assert_eq!(
            grpc_uri_from_config(&cfg).unwrap(),
            "http://localhost:50050"
        );
    }

    #[test]
    fn grpc_uri_https() {
        let cfg = SdkConfig::new("https://gateway.example.com:443", "testkey1");
        let mut cfg = cfg;
        cfg.gateway_grpc_port = 50051;
        assert_eq!(
            grpc_uri_from_config(&cfg).unwrap(),
            "https://gateway.example.com:50051"
        );
    }

    // ── Task 7: init — config validation ────────────────────────────────────

    #[tokio::test]
    async fn init_returns_config_error_on_bad_config() {
        let bad = SdkConfig::new("not_a_url", "key");
        let err = SdkClient::init(bad).await.unwrap_err();
        assert!(
            matches!(err, SdkError::Config(_)),
            "expected Config error, got {err}"
        );
    }

    // ── Task 8: evaluate — flag not found ────────────────────────────────────

    #[tokio::test]
    async fn evaluate_flag_not_found() {
        let snap = DefinitionSnapshot::default();
        let client = sdk_client_with_snapshot(snap);
        let ctx = Context::new("user", "alice");
        let results = client
            .evaluate(&[EvalRequest {
                flag_key: "no-such-flag".into(),
                context: ctx,
            }])
            .await;
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].outcome, EvalOutcome::FlagNotFound);
        assert!(results[0].variant_key.is_empty());
    }

    // ── Task 8: evaluate — disabled flag ────────────────────────────────────

    #[tokio::test]
    async fn evaluate_disabled_flag_returns_default() {
        let flag = disabled_flag("feature-x");
        let snap = DefinitionSnapshot::from_proto(SyncDefinitionsResponse {
            flags: vec![flag],
            rule_segments: vec![],
            list_segments: vec![],
            server_timestamp_ms: 0,
            environment_id: "env-1".into(),
        });
        let client = sdk_client_with_snapshot(snap);
        let ctx = Context::new("user", "alice");
        let results = client
            .evaluate(&[EvalRequest {
                flag_key: "feature-x".into(),
                context: ctx,
            }])
            .await;
        assert_eq!(results[0].outcome, EvalOutcome::Disabled);
        assert_eq!(results[0].variant_key, "false");
    }

    // ── Task 8: evaluate — bool flag default rule ────────────────────────────

    #[tokio::test]
    async fn evaluate_bool_flag_default_rule_returns_default_variant() {
        let flag = simple_bool_flag("show-banner");
        let snap = DefinitionSnapshot::from_proto(SyncDefinitionsResponse {
            flags: vec![flag],
            rule_segments: vec![],
            list_segments: vec![],
            server_timestamp_ms: 0,
            environment_id: "env-1".into(),
        });
        let client = sdk_client_with_snapshot(snap);
        let ctx = Context::new("user", "alice");
        let results = client
            .evaluate(&[EvalRequest {
                flag_key: "show-banner".into(),
                context: ctx,
            }])
            .await;
        assert_eq!(results[0].outcome, EvalOutcome::DefaultRule);
        assert_eq!(results[0].variant_key, "true");
        assert_eq!(results[0].variant_value, serde_json::Value::Bool(true));
    }

    // ── Task 8: evaluate — string flag with Eq rule match ───────────────────

    #[tokio::test]
    async fn evaluate_string_flag_eq_rule_matches() {
        let cond = ConditionExpr::Leaf(Condition::Eq {
            context_type: "user".into(),
            param: "plan".into(),
            value: CoreParam::Str("pro".into()),
        });
        let rule = simple_rule(cond, "new-checkout");
        let flag = FeatureFlag {
            key: "checkout-flow".into(),
            enabled: true,
            variants: vec![
                string_variant("new-checkout", "v2"),
                string_variant("old-checkout", "v1"),
            ],
            default_variant_key: "old-checkout".into(),
            rules: vec![rule],
            ..Default::default()
        };
        let snap = DefinitionSnapshot::from_proto(SyncDefinitionsResponse {
            flags: vec![flag],
            rule_segments: vec![],
            list_segments: vec![],
            server_timestamp_ms: 0,
            environment_id: "env-1".into(),
        });
        let client = sdk_client_with_snapshot(snap);

        // Context WITH plan=pro → should match rule
        let ctx_pro =
            Context::new("user", "alice").with_parameter("plan", CoreParam::Str("pro".into()));
        let res = client
            .evaluate(&[EvalRequest {
                flag_key: "checkout-flow".into(),
                context: ctx_pro,
            }])
            .await;
        assert_eq!(res[0].outcome, EvalOutcome::Matched { rule_index: 0 });
        assert_eq!(res[0].variant_key, "new-checkout");

        // Context WITHOUT plan → no match, default
        let ctx_free = Context::new("user", "bob");
        let res2 = client
            .evaluate(&[EvalRequest {
                flag_key: "checkout-flow".into(),
                context: ctx_free,
            }])
            .await;
        assert_eq!(res2[0].outcome, EvalOutcome::DefaultRule);
        assert_eq!(res2[0].variant_key, "old-checkout");
    }

    // ── Task 8: evaluate — rule-based segment ───────────────────────────────

    #[tokio::test]
    async fn evaluate_flag_with_rule_based_segment_member() {
        let seg_id = SegmentId::new();
        let seg_id_str = seg_id.as_uuid().to_string();

        // Segment rule: user.plan == "pro"
        let seg_rule = Rule {
            id: RuleId::new(),
            name: None,
            condition: ConditionExpr::Leaf(Condition::Eq {
                context_type: "user".into(),
                param: "plan".into(),
                value: CoreParam::Str("pro".into()),
            }),
            output: RuleOutput::Variant(stitchd_core::id::VariantId::new()),
        };
        let seg_payload = serde_json::to_vec(&vec![seg_rule]).unwrap();

        let rule_seg = RuleSegment {
            id: seg_id_str.clone(),
            key: "pro-users".into(),
            context_type: String::new(),
            rule_payload: seg_payload,
        };

        // Flag rule: InSegment(seg_id) → treatment
        let flag_cond = ConditionExpr::Leaf(Condition::InSegment(seg_id));
        let flag_rule = simple_rule(flag_cond, "treatment");

        let flag = FeatureFlag {
            key: "feature-flag".into(),
            enabled: true,
            variants: vec![
                string_variant("treatment", "on"),
                string_variant("control", "off"),
            ],
            default_variant_key: "control".into(),
            rules: vec![flag_rule],
            ..Default::default()
        };

        let snap = DefinitionSnapshot::from_proto(SyncDefinitionsResponse {
            flags: vec![flag],
            rule_segments: vec![rule_seg],
            list_segments: vec![],
            server_timestamp_ms: 0,
            environment_id: "env-1".into(),
        });
        let client = sdk_client_with_snapshot(snap);

        // pro user → member of segment → treatment
        let ctx_pro =
            Context::new("user", "alice").with_parameter("plan", CoreParam::Str("pro".into()));
        let res = client
            .evaluate(&[EvalRequest {
                flag_key: "feature-flag".into(),
                context: ctx_pro,
            }])
            .await;
        assert_eq!(res[0].outcome, EvalOutcome::Matched { rule_index: 0 });
        assert_eq!(res[0].variant_key, "treatment");

        // free user → not member → control
        let ctx_free = Context::new("user", "bob");
        let res2 = client
            .evaluate(&[EvalRequest {
                flag_key: "feature-flag".into(),
                context: ctx_free,
            }])
            .await;
        assert_eq!(res2[0].outcome, EvalOutcome::DefaultRule);
        assert_eq!(res2[0].variant_key, "control");
    }

    // ── Task 8: evaluate — list-segment LRU hit ──────────────────────────────

    #[tokio::test]
    async fn evaluate_list_segment_lru_hit_no_fetch() {
        let seg_id = SegmentId::new();
        let seg_id_str = seg_id.as_uuid().to_string();

        let list_seg = ListSegmentMeta {
            id: seg_id_str.clone(),
            key: "beta-testers".into(),
            context_type: "user".into(),
        };

        let flag_cond = ConditionExpr::Leaf(Condition::InSegment(seg_id));
        let flag_rule = simple_rule(flag_cond, "beta");

        let flag = FeatureFlag {
            key: "new-ui".into(),
            enabled: true,
            variants: vec![bool_variant("beta", true), bool_variant("stable", false)],
            default_variant_key: "stable".into(),
            rules: vec![flag_rule],
            ..Default::default()
        };

        let snap = DefinitionSnapshot::from_proto(SyncDefinitionsResponse {
            flags: vec![flag],
            rule_segments: vec![],
            list_segments: vec![list_seg],
            server_timestamp_ms: 0,
            environment_id: "env-1".into(),
        });

        let recording_fetcher = RecordingMembershipFetcher::new(HashMap::new());
        let client = sdk_client_with_membership_fetcher(
            snap,
            Arc::clone(&recording_fetcher) as Arc<dyn MembershipBatchFetcher>,
        );

        // Pre-populate LRU for (user, alice) — member of seg_id
        client.membership_cache.insert(
            "user",
            "alice",
            std::iter::once((seg_id_str.clone(), true)).collect(),
        );

        let ctx = Context::new("user", "alice");
        let res = client
            .evaluate(&[EvalRequest {
                flag_key: "new-ui".into(),
                context: ctx,
            }])
            .await;

        assert_eq!(res[0].outcome, EvalOutcome::Matched { rule_index: 0 });
        assert_eq!(res[0].variant_key, "beta");
        // No HTTP fetch should have happened — LRU hit.
        assert_eq!(recording_fetcher.call_count(), 0);
    }

    // ── Task 8: evaluate — list-segment LRU miss → on-demand fetch ───────────

    #[tokio::test]
    async fn evaluate_list_segment_lru_miss_triggers_fetch_and_inserts_into_lru() {
        let seg_id = SegmentId::new();
        let seg_id_str = seg_id.as_uuid().to_string();

        let list_seg = ListSegmentMeta {
            id: seg_id_str.clone(),
            key: "vip-users".into(),
            context_type: "user".into(),
        };

        let flag_cond = ConditionExpr::Leaf(Condition::InSegment(seg_id));
        let flag_rule = simple_rule(flag_cond, "vip");

        let flag = FeatureFlag {
            key: "vip-feature".into(),
            enabled: true,
            variants: vec![
                string_variant("vip", "special"),
                string_variant("default", "normal"),
            ],
            default_variant_key: "default".into(),
            rules: vec![flag_rule],
            ..Default::default()
        };

        let snap = DefinitionSnapshot::from_proto(SyncDefinitionsResponse {
            flags: vec![flag],
            rule_segments: vec![],
            list_segments: vec![list_seg],
            server_timestamp_ms: 0,
            environment_id: "env-1".into(),
        });

        // Fetcher returns: alice IS a member of seg_id
        let memberships: HashMap<String, bool> =
            std::iter::once((seg_id_str.clone(), true)).collect();
        let recording_fetcher = RecordingMembershipFetcher::new(memberships);
        let client = sdk_client_with_membership_fetcher(
            snap,
            Arc::clone(&recording_fetcher) as Arc<dyn MembershipBatchFetcher>,
        );

        // LRU is empty — miss will trigger fetch
        assert!(client.membership_cache.get("user", "alice").is_none());

        let ctx = Context::new("user", "alice");
        let res = client
            .evaluate(&[EvalRequest {
                flag_key: "vip-feature".into(),
                context: ctx,
            }])
            .await;

        // Fetch should have been called once
        assert_eq!(
            recording_fetcher.call_count(),
            1,
            "expected 1 on-demand fetch"
        );
        // Variant should be "vip" (alice is a member)
        assert_eq!(res[0].variant_key, "vip");
        assert_eq!(res[0].outcome, EvalOutcome::Matched { rule_index: 0 });
        // LRU should now have an entry for (user, alice)
        assert!(
            client.membership_cache.get("user", "alice").is_some(),
            "LRU should be populated after on-demand fetch"
        );
    }

    #[tokio::test]
    async fn evaluate_list_segment_second_call_uses_lru_not_fetcher() {
        let seg_id = SegmentId::new();
        let seg_id_str = seg_id.as_uuid().to_string();

        let list_seg = ListSegmentMeta {
            id: seg_id_str.clone(),
            key: "vip-users".into(),
            context_type: "user".into(),
        };

        let flag = FeatureFlag {
            key: "vip-feature".into(),
            enabled: true,
            variants: vec![
                string_variant("vip", "special"),
                string_variant("default", "normal"),
            ],
            default_variant_key: "default".into(),
            rules: vec![simple_rule(
                ConditionExpr::Leaf(Condition::InSegment(seg_id)),
                "vip",
            )],
            ..Default::default()
        };

        let snap = DefinitionSnapshot::from_proto(SyncDefinitionsResponse {
            flags: vec![flag],
            rule_segments: vec![],
            list_segments: vec![list_seg],
            server_timestamp_ms: 0,
            environment_id: "env-1".into(),
        });

        let memberships: HashMap<String, bool> =
            std::iter::once((seg_id_str.clone(), true)).collect();
        let recording_fetcher = RecordingMembershipFetcher::new(memberships);
        let client = sdk_client_with_membership_fetcher(
            snap,
            Arc::clone(&recording_fetcher) as Arc<dyn MembershipBatchFetcher>,
        );

        let ctx = || Context::new("user", "alice");

        // First call → miss → fetch
        client
            .evaluate(&[EvalRequest {
                flag_key: "vip-feature".into(),
                context: ctx(),
            }])
            .await;
        assert_eq!(recording_fetcher.call_count(), 1);

        // Second call → LRU hit → no fetch
        client
            .evaluate(&[EvalRequest {
                flag_key: "vip-feature".into(),
                context: ctx(),
            }])
            .await;
        assert_eq!(
            recording_fetcher.call_count(),
            1,
            "second call must use LRU, not fetcher"
        );
    }

    // ── Task 8: evaluate — reasoning trace ───────────────────────────────────

    #[tokio::test]
    async fn evaluate_with_reasoning_includes_trace() {
        let cond = ConditionExpr::Leaf(Condition::Eq {
            context_type: "user".into(),
            param: "plan".into(),
            value: CoreParam::Str("pro".into()),
        });
        let mut rule = simple_rule(cond, "treatment");
        rule.name = "pro-user-rule".into();

        let flag = FeatureFlag {
            key: "upgrade-cta".into(),
            enabled: true,
            variants: vec![
                string_variant("treatment", "show-cta"),
                string_variant("control", "hide-cta"),
            ],
            default_variant_key: "control".into(),
            rules: vec![rule],
            ..Default::default()
        };

        let snap = DefinitionSnapshot::from_proto(SyncDefinitionsResponse {
            flags: vec![flag],
            rule_segments: vec![],
            list_segments: vec![],
            server_timestamp_ms: 0,
            environment_id: "env-1".into(),
        });
        let client = sdk_client_with_snapshot(snap);
        let ctx =
            Context::new("user", "alice").with_parameter("plan", CoreParam::Str("pro".into()));

        let results = client
            .evaluate_with_reasoning(&[EvalRequest {
                flag_key: "upgrade-cta".into(),
                context: ctx,
            }])
            .await;
        assert_eq!(results.len(), 1);
        let r = &results[0];
        assert_eq!(r.variant_key, "treatment");
        assert_eq!(r.outcome, EvalOutcome::Matched { rule_index: 0 });
        assert_eq!(r.reasoning.matched_rule_index, Some(0));
        assert_eq!(
            r.reasoning.matched_rule_name.as_deref(),
            Some("pro-user-rule")
        );
    }

    #[tokio::test]
    async fn evaluate_with_reasoning_flag_not_found_trace() {
        let snap = DefinitionSnapshot::default();
        let client = sdk_client_with_snapshot(snap);
        let ctx = Context::new("user", "alice");
        let results = client
            .evaluate_with_reasoning(&[EvalRequest {
                flag_key: "nonexistent".into(),
                context: ctx,
            }])
            .await;
        assert_eq!(results[0].reasoning.outcome, EvalOutcome::FlagNotFound);
        assert!(results[0].reasoning.matched_rule_index.is_none());
    }

    // ── Task 9: shutdown ─────────────────────────────────────────────────────

    #[tokio::test(flavor = "current_thread", start_paused = true)]
    async fn shutdown_stops_all_tasks() {
        let snap = DefinitionSnapshot::default();
        let client = sdk_client_with_snapshot(snap);

        // Enqueue a few events to verify flush task drains them.
        for _ in 0..5 {
            client.event_queue.send(FlagEvaluationEvent {
                flag_key: "test".into(),
                flag_id: String::new(),
                variant_key: "on".into(),
                context_type: "user".into(),
                context_key: "x".into(),
                evaluated_at: "2026-05-16T00:00:00.000Z".into(),
                matched_rule_id: String::new(),
                outcome: "matched".into(),
                reasoning_included: false,
                context_parameters: HashMap::new(),
            });
        }

        // shutdown should complete without hanging.
        tokio::time::timeout(Duration::from_secs(5), client.shutdown())
            .await
            .expect("shutdown must not hang");
    }

    #[tokio::test]
    async fn shutdown_is_safe_when_already_stopped() {
        let snap = DefinitionSnapshot::default();
        let client = sdk_client_with_snapshot(snap);
        // Double-shutdown should not panic.
        let client2 = Arc::clone(&client);
        client.shutdown().await;
        client2.shutdown().await;
    }

    // ── events emitted on evaluate ────────────────────────────────────────────

    #[tokio::test]
    async fn evaluate_emits_event_for_each_request() {
        let flag = simple_bool_flag("test-flag");
        let snap = DefinitionSnapshot::from_proto(SyncDefinitionsResponse {
            flags: vec![flag],
            rule_segments: vec![],
            list_segments: vec![],
            server_timestamp_ms: 0,
            environment_id: "env-1".into(),
        });
        let client = sdk_client_with_snapshot(snap);
        let ctx = Context::new("user", "alice");

        assert_eq!(client.event_queue.len(), 0);
        client
            .evaluate(&[
                EvalRequest {
                    flag_key: "test-flag".into(),
                    context: ctx.clone(),
                },
                EvalRequest {
                    flag_key: "test-flag".into(),
                    context: ctx,
                },
            ])
            .await;
        assert_eq!(client.event_queue.len(), 2, "one event per EvalRequest");
    }

    // ── Phase 5 Task 5.2: track() + is_event_registered ─────────────────────

    /// Build a snapshot with the supplied event definitions registered.
    fn snapshot_with_event_defs(defs: Vec<(&str, EventValueType)>) -> DefinitionSnapshot {
        let map: HashMap<String, EventValueType> =
            defs.into_iter().map(|(k, v)| (k.to_string(), v)).collect();
        DefinitionSnapshot::from_proto(SyncDefinitionsResponse {
            flags: vec![],
            rule_segments: vec![],
            list_segments: vec![],
            server_timestamp_ms: 0,
            environment_id: "env-1".into(),
        })
        .with_event_definitions(map)
    }

    /// Build a SdkClient bound to the supplied snapshot WITH a real
    /// `EventBuffer` that POSTs to `gateway_url`. The buffer's interval
    /// is set to 60s so tests opt-in to flushing via `enqueue` size
    /// triggers or by inspecting `EventBuffer::flush()` directly.
    fn sdk_client_with_track_buffer(
        snapshot: DefinitionSnapshot,
        gateway_url: &str,
    ) -> Arc<SdkClient> {
        let definition_store = DefinitionStore::from_snapshot(snapshot);
        let membership_cache = MembershipCache::new(100);
        let event_queue = EventQueue::new(1000, 100);

        let sink: Arc<dyn EventSink> = Arc::new(NoopSink);
        let flush_task = FlushTask::spawn(event_queue.clone(), sink, Duration::from_secs(60));

        let membership_fetcher: Arc<dyn MembershipBatchFetcher> = Arc::new(NoopMembershipFetcher);
        let poll_fetcher: Arc<dyn DefinitionFetcher> = Arc::new(NoopFetcher);
        let poll_task = PollTask::spawn(
            poll_fetcher,
            definition_store.clone(),
            Duration::from_secs(60),
        );
        let refresh_task = RefreshTask::spawn(
            Arc::clone(&membership_fetcher),
            membership_cache.clone(),
            definition_store.clone(),
            Duration::from_secs(60),
        );

        let buffer = EventBuffer::new(EventBufferConfig {
            flush_at_size: 100,
            flush_interval: Duration::from_secs(60),
            max_retries: 0,
            backoff_base: Duration::from_millis(1),
            gateway_base_url: gateway_url.to_string(),
            sdk_key: "test-sdk-key".to_string(),
        });

        Arc::new(SdkClient {
            definition_store,
            membership_cache,
            event_queue,
            membership_fetcher,
            event_buffer: Some(buffer),
            poll_task: Mutex::new(Some(poll_task)),
            refresh_task: Mutex::new(Some(refresh_task)),
            flush_task: Mutex::new(Some(flush_task)),
        })
    }

    #[tokio::test]
    async fn test_is_event_registered_reflects_cache() {
        let snap = snapshot_with_event_defs(vec![
            ("checkout_completed", EventValueType::Bool),
            ("revenue", EventValueType::Double),
        ]);
        let client = sdk_client_with_snapshot(snap);
        assert!(client.is_event_registered("checkout_completed"));
        assert!(client.is_event_registered("revenue"));
        assert!(!client.is_event_registered("not_a_real_event"));
    }

    #[tokio::test]
    async fn test_track_with_registered_event_enqueues() {
        // wiremock server that captures the body so we can assert the
        // event was indeed enqueued + flushed.
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/events/track"))
            .respond_with(ResponseTemplate::new(202).set_body_json(serde_json::json!({
                "accepted_count": 1,
                "rejected": []
            })))
            .expect(1)
            .mount(&server)
            .await;

        let snap = snapshot_with_event_defs(vec![("checkout_completed", EventValueType::Bool)]);
        let client = sdk_client_with_track_buffer(snap, &server.uri());

        let ctx = Context::new("user", "alice");
        client
            .track(
                "checkout_completed",
                &ctx,
                Some(TypedValue::Bool(true)),
                None,
            )
            .await
            .expect("track must succeed for registered event");

        // Force a flush so the wiremock assertion holds at the end of test.
        let buffer = client.event_buffer.as_ref().unwrap();
        let report = buffer.flush().await.expect("flush must succeed");
        assert_eq!(report.accepted, 1);
        assert_eq!(report.rejected, 0);
    }

    #[tokio::test]
    async fn test_track_with_unknown_event_warns_and_skips() {
        // No mock — if track tries to POST anything, wiremock complains.
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/events/track"))
            .respond_with(ResponseTemplate::new(202))
            .expect(0)
            .mount(&server)
            .await;

        // Empty event-definitions cache.
        let snap = snapshot_with_event_defs(vec![]);
        let client = sdk_client_with_track_buffer(snap, &server.uri());

        let ctx = Context::new("user", "alice");
        // Per spec F2.4: unknown event → Ok(()) (warn + skip, NOT Err).
        client
            .track("ghost_event", &ctx, Some(TypedValue::Int(1)), None)
            .await
            .expect("track must NOT propagate errors for unknown event_key");

        // Buffer must still be empty — flush should be a no-op.
        let buffer = client.event_buffer.as_ref().unwrap();
        let report = buffer.flush().await.expect("empty flush is ok");
        assert_eq!(report.accepted, 0);
        assert_eq!(report.rejected, 0);
    }

    #[tokio::test]
    async fn test_track_with_mismatched_value_type_warns_and_skips() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/events/track"))
            .respond_with(ResponseTemplate::new(202))
            .expect(0)
            .mount(&server)
            .await;

        // Registered as Bool, caller supplies Int → mismatch.
        let snap = snapshot_with_event_defs(vec![("conversion", EventValueType::Bool)]);
        let client = sdk_client_with_track_buffer(snap, &server.uri());

        let ctx = Context::new("user", "alice");
        client
            .track("conversion", &ctx, Some(TypedValue::Int(42)), None)
            .await
            .expect("track must NOT propagate errors for type mismatch");

        let buffer = client.event_buffer.as_ref().unwrap();
        let report = buffer.flush().await.expect("empty flush is ok");
        assert_eq!(report.accepted, 0);
        assert_eq!(report.rejected, 0);
    }

    #[tokio::test]
    async fn test_track_with_no_value_skips_type_check() {
        // value=None is legal (pure occurrence marker) — no type check.
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/events/track"))
            .respond_with(ResponseTemplate::new(202).set_body_json(serde_json::json!({
                "accepted_count": 1,
                "rejected": []
            })))
            .expect(1)
            .mount(&server)
            .await;

        let snap = snapshot_with_event_defs(vec![("page_view", EventValueType::Int)]);
        let client = sdk_client_with_track_buffer(snap, &server.uri());

        let ctx = Context::new("user", "alice");
        client
            .track("page_view", &ctx, None, None)
            .await
            .expect("track must accept value=None regardless of registered type");

        let buffer = client.event_buffer.as_ref().unwrap();
        let report = buffer.flush().await.expect("flush must succeed");
        assert_eq!(report.accepted, 1);
    }

    #[tokio::test]
    async fn test_track_without_event_buffer_returns_ok() {
        // SdkClient without an event_buffer (test-util construction path).
        // track() must be a silent no-op — Ok(()) and no panic.
        let snap = snapshot_with_event_defs(vec![("event", EventValueType::Bool)]);
        let client = sdk_client_with_snapshot(snap);
        assert!(client.event_buffer.is_none());

        let ctx = Context::new("user", "alice");
        client
            .track("event", &ctx, Some(TypedValue::Bool(true)), None)
            .await
            .expect("track must be a no-op when buffer is absent");
    }

    #[tokio::test]
    async fn test_track_round_trips_context_and_properties() {
        // Verify the buffered event carries context_type/key + properties
        // intact through to the POST body.
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

        let server = MockServer::start().await;
        let captured: Arc<std::sync::Mutex<Option<serde_json::Value>>> =
            Arc::new(std::sync::Mutex::new(None));

        struct Capture {
            slot: Arc<std::sync::Mutex<Option<serde_json::Value>>>,
        }
        impl Respond for Capture {
            fn respond(&self, req: &Request) -> ResponseTemplate {
                *self.slot.lock().unwrap() =
                    Some(serde_json::from_slice(&req.body).unwrap());
                ResponseTemplate::new(202).set_body_json(serde_json::json!({
                    "accepted_count": 1,
                    "rejected": []
                }))
            }
        }

        Mock::given(method("POST"))
            .and(path("/v1/events/track"))
            .respond_with(Capture {
                slot: Arc::clone(&captured),
            })
            .expect(1)
            .mount(&server)
            .await;

        let snap = snapshot_with_event_defs(vec![("purchase", EventValueType::Double)]);
        let client = sdk_client_with_track_buffer(snap, &server.uri());

        let ctx = Context::new("user", "u42");
        let mut props = HashMap::new();
        props.insert("currency".to_string(), "USD".to_string());

        client
            .track(
                "purchase",
                &ctx,
                Some(TypedValue::Double(19.99)),
                Some(props),
            )
            .await
            .expect("track must succeed");

        let buffer = client.event_buffer.as_ref().unwrap();
        buffer.flush().await.expect("flush must succeed");

        let body = captured.lock().unwrap().clone().expect("body captured");
        let ev0 = &body["events"][0];
        assert_eq!(ev0["event_key"], "purchase");
        assert_eq!(ev0["context_type"], "user");
        assert_eq!(ev0["context_key"], "u42");
        assert_eq!(ev0["value"], serde_json::json!({"double": 19.99}));
        assert_eq!(ev0["properties"]["currency"], "USD");
        // occurred_at is SDK-stamped from `Utc::now()` — just confirm presence.
        assert!(ev0.get("occurred_at").is_some_and(|v| v.is_string()));
    }
}

// ============================================================================
// test-util — helpers for integration / conformance tests
// ============================================================================

/// Test helpers exposed under `--features test-util`. Allows integration
/// tests to construct `SdkClient` from an in-memory snapshot without a real
/// network connection.
#[cfg(feature = "test-util")]
pub mod testing {
    use super::*;
    use std::time::Duration;

    struct NoopSink;
    #[async_trait]
    impl EventSink for NoopSink {
        async fn flush(&self, _batch: Vec<FlagEvaluationEvent>) -> Result<(), SdkError> {
            Ok(())
        }
    }

    struct NoopDefinitionFetcher;
    #[async_trait]
    impl DefinitionFetcher for NoopDefinitionFetcher {
        async fn fetch(&self) -> Result<DefinitionSnapshot, SdkError> {
            Ok(DefinitionSnapshot::default())
        }
    }

    /// A `MembershipBatchFetcher` that always returns empty membership maps.
    pub struct NoopMembershipFetcher;
    #[async_trait]
    impl MembershipBatchFetcher for NoopMembershipFetcher {
        async fn fetch(
            &self,
            contexts: Vec<ContextKey>,
            _segment_ids: Vec<String>,
        ) -> Result<Vec<MembershipMap>, SdkError> {
            Ok(contexts.iter().map(|_| HashMap::new()).collect())
        }
    }

    /// Construct an `Arc<SdkClient>` from an in-memory snapshot.
    ///
    /// - `snapshot`: the definition snapshot to serve evaluations from.
    /// - `membership_fetcher`: called on list-segment LRU miss.
    /// - `preseed`: `(context_type, context_key, memberships)` tuples pre-loaded into LRU.
    pub fn sdk_client_with_snapshot_and_lru(
        snapshot: DefinitionSnapshot,
        membership_fetcher: Arc<dyn MembershipBatchFetcher>,
        preseed: Vec<(String, String, MembershipMap)>,
    ) -> Arc<SdkClient> {
        let definition_store = DefinitionStore::from_snapshot(snapshot);
        let membership_cache = MembershipCache::new(1000);

        for (ctx_type, ctx_key, memberships) in preseed {
            membership_cache.insert(&ctx_type, &ctx_key, memberships);
        }

        let event_queue = EventQueue::new(1000, 100);
        let sink: Arc<dyn EventSink> = Arc::new(NoopSink);
        let flush_task = FlushTask::spawn(event_queue.clone(), sink, Duration::from_secs(60));

        let poll_fetcher: Arc<dyn DefinitionFetcher> = Arc::new(NoopDefinitionFetcher);
        let poll_task = PollTask::spawn(
            poll_fetcher,
            definition_store.clone(),
            Duration::from_secs(60),
        );
        let refresh_task = RefreshTask::spawn(
            Arc::clone(&membership_fetcher),
            membership_cache.clone(),
            definition_store.clone(),
            Duration::from_secs(60),
        );

        Arc::new(SdkClient {
            definition_store,
            membership_cache,
            event_queue,
            membership_fetcher,
            event_buffer: None,
            poll_task: Mutex::new(Some(poll_task)),
            refresh_task: Mutex::new(Some(refresh_task)),
            flush_task: Mutex::new(Some(flush_task)),
        })
    }

    /// Simpler variant with a no-op membership fetcher and empty LRU.
    pub fn sdk_client_simple(snapshot: DefinitionSnapshot) -> Arc<SdkClient> {
        sdk_client_with_snapshot_and_lru(snapshot, Arc::new(NoopMembershipFetcher), vec![])
    }

    /// Construct an `Arc<SdkClient>` with a real `EventBuffer` pointing at
    /// `gateway_base_url` (typically a wiremock server). Used by tests
    /// that exercise the full `track()` → buffer → POST pipeline.
    pub fn sdk_client_with_track_buffer(
        snapshot: DefinitionSnapshot,
        gateway_base_url: impl Into<String>,
        sdk_key: impl Into<String>,
    ) -> Arc<SdkClient> {
        let definition_store = DefinitionStore::from_snapshot(snapshot);
        let membership_cache = MembershipCache::new(1000);
        let event_queue = EventQueue::new(1000, 100);

        let sink: Arc<dyn EventSink> = Arc::new(NoopSink);
        let flush_task = FlushTask::spawn(event_queue.clone(), sink, Duration::from_secs(60));

        let poll_fetcher: Arc<dyn DefinitionFetcher> = Arc::new(NoopDefinitionFetcher);
        let poll_task = PollTask::spawn(
            poll_fetcher,
            definition_store.clone(),
            Duration::from_secs(60),
        );

        let membership_fetcher: Arc<dyn MembershipBatchFetcher> = Arc::new(NoopMembershipFetcher);
        let refresh_task = RefreshTask::spawn(
            Arc::clone(&membership_fetcher),
            membership_cache.clone(),
            definition_store.clone(),
            Duration::from_secs(60),
        );

        // Long flush interval — tests opt-in to explicit flushes or size
        // triggers; otherwise the interval never fires within a test.
        let buffer_cfg = EventBufferConfig {
            flush_at_size: 100,
            flush_interval: Duration::from_secs(60),
            max_retries: 0,
            backoff_base: Duration::from_millis(1),
            gateway_base_url: gateway_base_url.into(),
            sdk_key: sdk_key.into(),
        };
        let event_buffer = EventBuffer::new(buffer_cfg);

        Arc::new(SdkClient {
            definition_store,
            membership_cache,
            event_queue,
            membership_fetcher,
            event_buffer: Some(event_buffer),
            poll_task: Mutex::new(Some(poll_task)),
            refresh_task: Mutex::new(Some(refresh_task)),
            flush_task: Mutex::new(Some(flush_task)),
        })
    }
}
