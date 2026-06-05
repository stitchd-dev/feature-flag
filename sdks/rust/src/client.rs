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
//! - `GrpcDefinitionFetcher` — calls `SdkService::SyncDefinitions` via tonic.
//! - `HttpMembershipFetcher` — calls `POST /v1/sdk/segments/list:batch` via reqwest.
//! - `HttpEventSink` — calls `POST /v1/sdk/events:batch` via reqwest.
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
use stitchd_core::evaluation::{
    EvalOutcome as CoreEvalOutcome, EvaluationTrace, HashInputSpec, HashSelector,
    ListMembershipIndex, TraceLevel, evaluate_flag, evaluate_flag_with_prerequisites,
};
use stitchd_core::flag::{Flag, FlagRecord, FlagRule as CoreFlagRule, Variant as CoreVariant};
use stitchd_core::id::{EnvironmentId, FlagId, FlagKey, ProjectId, RuleId, SegmentId, VariantId};
use stitchd_core::prerequisite::{FlagPrerequisite as CorePrerequisite, PrerequisiteGate};
use stitchd_core::rule_engine::condition::Condition;
use stitchd_core::rule_engine::orchestrator::{
    evaluate_flags_with_prerequisites, fold_prerequisite_fallbacks,
};
use stitchd_core::rule_engine::types::{
    ConditionExpr, EvaluationInput, ExclusionGate, PercentageTarget, Rule, RuleOutput, TargetField,
};
use stitchd_core::segment::{RuleBasedSegment, SegmentDefinition};
use stitchd_core::variants::{FlagValueType, VariantValue as CoreVariantValue};

use stitchd_proto::flags::v1::{
    FeatureFlag, FlagRule as ProtoFlagRule, PercentageAllocation, flag_rule::Output as ProtoOutput,
    hash_selector::Selector as ProtoHashSelectorOneof,
};
use stitchd_proto::sdk::v1::SyncDefinitionsRequest;
use stitchd_proto::sdk::v1::sdk_service_client::SdkServiceClient;

use crate::config::SdkConfig;
use crate::error::{SdkError, TrackError};
use crate::event_buffer::{BufferedEvent, EventBuffer, EventBufferConfig, FlushReport, TypedValue};
use crate::events::{EventQueue, EventSink, FlagEvaluationEvent, FlushTask, ParameterValue};
use crate::lru::{ContextKey, MembershipCache, MembershipMap};
use crate::polling::{DefinitionFetcher, PollTask};
use crate::refresh::{MembershipBatchFetcher, RefreshTask};
use crate::snapshot::{DefinitionSnapshot, DefinitionStore, EventValueType};

// ============================================================================
// Public output types (Tasks 8)
// ============================================================================

/// A single flag-evaluation request.
///
/// Carries a bundle of [`Context`] values — typically one entry per context
/// type the flag's rules may reference (e.g. `user`, `device`, `application`).
/// Cross-context percentage hashing draws from the FULL bundle via the
/// flag's `HashInputSpec`, so callers must include every context type the
/// rule selectors reference for hash stability.
///
/// **Single-context callers:** when a flag only references one context type,
/// `contexts` is a single-element vec — see [`EvalRequest::single`] for a
/// terse helper.
pub struct EvalRequest {
    /// The flag's string key (e.g. `"checkout-flow"`).
    pub flag_key: String,
    /// Evaluation context bundle. One entry per `(type, key, params)` tuple
    /// the flag's rules may inspect. Cross-context percentage hashing draws
    /// from the full bundle.
    pub contexts: Vec<Context>,
}

impl EvalRequest {
    /// Convenience constructor for the single-context case.
    ///
    /// Equivalent to `EvalRequest { flag_key, contexts: vec![context] }`.
    pub fn single(flag_key: impl Into<String>, context: Context) -> Self {
        Self {
            flag_key: flag_key.into(),
            contexts: vec![context],
        }
    }
}

/// Outcome category of a completed evaluation.
///
/// Phase 6 of `flag_eval_unify_20260522`: this is the SDK-facing outcome
/// taxonomy. It mirrors [`stitchd_core::evaluation::EvalOutcome`] plus the
/// two SDK-only variants (`Disabled` collapses into `FlagDisabled`,
/// `FlagNotFound` is SDK-only because the core engine never sees a missing
/// flag — the SDK short-circuits before calling `evaluate_flag`).
#[derive(Debug, Clone, PartialEq)]
pub enum EvalOutcome {
    /// A targeting rule matched — the rule's 0-based index is included.
    Matched { rule_index: usize },
    /// No targeting rule matched; the flag's default rule fired.
    DefaultRule,
    /// No targeting rule matched; a variant was selected from the flag's
    /// `default_rule_distribution` via hash-based assignment.
    DefaultRuleDistribution,
    /// Flag exists but is disabled; default variant returned.
    Disabled,
    /// A prerequisite flag did not resolve to its required variant (or was
    /// disabled / absent); the flag's configured fallback variant was
    /// returned and its rules were skipped.
    PrerequisiteFailed,
    /// Flag key not found in the current snapshot.
    FlagNotFound,
}

impl EvalOutcome {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Matched { .. } => "matched",
            Self::DefaultRule => "default_rule",
            Self::DefaultRuleDistribution => "default_rule_distribution",
            Self::Disabled => "disabled",
            Self::PrerequisiteFailed => "prerequisite_failed",
            Self::FlagNotFound => "flag_not_found",
        }
    }
}

/// Result of a single flag evaluation, optionally carrying a full
/// [`EvaluationTrace`] when the caller requested [`TraceLevel::Full`].
///
/// Phase 6 of `flag_eval_unify_20260522` collapses the legacy
/// `evaluate` / `evaluate_with_reasoning` split into a single
/// [`SdkClient::evaluate`] entry — the `trace` field replaces the old
/// `EvalResultWithReasoning.reasoning` shape and carries the rich trace
/// bundle from `stitchd-core` verbatim.
#[derive(Debug, Clone)]
pub struct EvalResult {
    /// Echo of the request's `flag_key`.
    pub flag_key: String,
    /// The variant `key` that was selected.
    pub variant_key: String,
    /// The variant `value` payload as a JSON value.
    pub variant_value: serde_json::Value,
    /// Outcome classification.
    pub outcome: EvalOutcome,
    /// Per-context-index of the entry within the request's `contexts` bundle.
    /// Useful when a request bundles multiple subject contexts and the
    /// caller wants to correlate results back to the input ordering.
    pub context_index: usize,
    /// Rich evaluation trace — only `Some` when the caller passed
    /// [`TraceLevel::Full`] to [`SdkClient::evaluate`]. Always `None` on the
    /// hot path ([`TraceLevel::Off`]).
    pub trace: Option<EvaluationTrace>,
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

    // ── evaluate (Task 8 + Phase 6 of flag_eval_unify_20260522) ──────────────

    /// Evaluate a batch of flag requests using the local definition snapshot.
    ///
    /// Phase 6 of `flag_eval_unify_20260522` collapses the prior
    /// `evaluate` / `evaluate_with_reasoning` pair into a single entry point
    /// gated by the [`TraceLevel`] argument:
    ///
    /// - [`TraceLevel::Off`] — hot-path mode. `EvalResult.trace` is always
    ///   `None` and no trace structures are allocated.
    /// - [`TraceLevel::Full`] — preview / debug mode. `EvalResult.trace`
    ///   carries the rich [`EvaluationTrace`] (rule traces + rollout debug).
    ///
    /// The orchestration is delegated to
    /// [`stitchd_core::evaluation::evaluate_flag`] — the SDK only assembles
    /// the inputs (flag, contexts, rule-based segments, list-segment
    /// membership index) and emits one [`FlagEvaluationEvent`] per result.
    ///
    /// Each request carries an N-context bundle (`contexts: Vec<Context>`);
    /// cross-context percentage hashing draws from the full bundle. The
    /// returned `Vec<EvalResult>` has one entry per `(request, context)` —
    /// i.e. if a request bundles 3 contexts, the result vec gains 3 entries
    /// for that request (each carrying its `context_index`).
    ///
    /// On LRU miss for a list-segment, the SDK makes a synchronous HTTP call
    /// to the gateway to fetch membership, inserts the result into the LRU,
    /// then continues evaluation. The call is fast on LRU hit.
    pub async fn evaluate(&self, requests: &[EvalRequest], trace: TraceLevel) -> Vec<EvalResult> {
        let snapshot = self.definition_store.load();
        // Capacity guess — most requests are single-context, so this is a
        // good first approximation. The vec grows naturally for multi-
        // context requests.
        let mut results = Vec::with_capacity(requests.len());
        let env_id = parse_env_id(snapshot.environment_id());
        // Project id is reserved for future hash-salt extensions
        // (cf. evaluate_flag rustdoc). Today the hash salt is
        // `(flag_key, env_id, target_values)` — the project_id is unused,
        // but the core API requires one so we pass a fresh placeholder.
        let project_id = ProjectId::new();

        for req in requests {
            let request_results = self
                .evaluate_request(&snapshot, req, trace, env_id, project_id)
                .await;
            results.extend(request_results);
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

    // ── flush + shutdown (Phase 5 Task 5.3) ──────────────────────────────────

    /// Force the client-side track-event buffer to drain immediately.
    ///
    /// Returns a [`FlushReport`] describing how many events the gateway
    /// accepted / rejected and how many retries the underlying POST
    /// needed. Calling `flush()` on a buffer-less client (the
    /// `test-util` construction path) returns an empty report — there's
    /// nothing to flush.
    ///
    /// # Errors
    ///
    /// - [`TrackError::Network`] — all retries exhausted; events were
    ///   dropped.
    /// - [`TrackError::Permanent`] — gateway returned a non-retryable
    ///   4xx; events were dropped.
    pub async fn flush(&self) -> Result<FlushReport, TrackError> {
        let Some(buf) = &self.event_buffer else {
            return Ok(FlushReport::default());
        };
        buf.flush().await.map_err(TrackError::from)
    }

    // ── shutdown (Task 9 + Phase 5 Task 5.3) ─────────────────────────────────

    /// Gracefully shut down the SDK client.
    ///
    /// Steps performed, in order:
    ///
    /// 1. Stops the three background tasks (poll, LRU refresh,
    ///    flag-evaluation flush). No more snapshot swaps and no more
    ///    evaluation events after this point.
    /// 2. Drains the client-side track-event buffer with one final
    ///    flush bounded by `timeout`. Any events still pending after
    ///    the timeout fires are dropped with a `tracing::warn!` and
    ///    counted via the `stitchd_sdk_events_dropped_total{reason="shutdown_timeout"}`
    ///    counter.
    ///
    /// After this call returns the `SdkClient` should be dropped — its
    /// background tasks are gone and its buffer's interval flusher is
    /// aborted.
    ///
    /// Returns the [`FlushReport`] from the final track-event flush.
    /// Clients constructed without an event buffer (the `test-util`
    /// path) yield an empty report.
    ///
    /// # Errors
    ///
    /// Same as [`Self::flush`].
    pub async fn shutdown(
        self: Arc<Self>,
        timeout: std::time::Duration,
    ) -> Result<FlushReport, TrackError> {
        // Stop poll task first (no more snapshot swaps after this).
        if let Some(task) = self.poll_task.lock().await.take() {
            task.shutdown().await;
        }
        // Stop LRU refresh.
        if let Some(task) = self.refresh_task.lock().await.take() {
            task.shutdown().await;
        }
        // Flag-evaluation flush task — drains its own queue before exit.
        if let Some(task) = self.flush_task.lock().await.take() {
            task.shutdown().await;
        }
        // Track-event buffer: one final flush bounded by `timeout`.
        // `EventBuffer::shutdown()` already aborts the interval flusher
        // and drops overflow on timeout — we just attempt the final
        // flush first so the caller gets a real `FlushReport`.
        let Some(buf) = &self.event_buffer else {
            return Ok(FlushReport::default());
        };
        let report = match tokio::time::timeout(timeout, buf.flush()).await {
            Ok(Ok(r)) => r,
            Ok(Err(e)) => {
                // Flush failed (retries exhausted / permanent 4xx) — still
                // run buffer-shutdown so the interval task is stopped.
                buf.shutdown(timeout).await;
                return Err(TrackError::from(e));
            }
            Err(_) => {
                // Final flush timed out — buf.shutdown will drop overflow.
                FlushReport::default()
            }
        };
        buf.shutdown(timeout).await;
        Ok(report)
    }

    // ── Internal evaluation (Phase 6) ────────────────────────────────────────

    /// Evaluate a single [`EvalRequest`] against the snapshot.
    ///
    /// Implements Phase 6 of `flag_eval_unify_20260522` — orchestration is
    /// delegated to [`stitchd_core::evaluation::evaluate_flag`]; this method
    /// only assembles the inputs (proto → core flag conversion, rule-based
    /// segment definitions, list-segment membership index via the
    /// existing LRU + REST fetcher) and emits one event per result.
    ///
    /// Returns one [`EvalResult`] per context in `req.contexts` — when the
    /// flag is missing or disabled, the result vec is the same length as
    /// `req.contexts` and every entry shares the same default-variant
    /// payload (semantics mirror the core engine).
    async fn evaluate_request(
        &self,
        snapshot: &DefinitionSnapshot,
        req: &EvalRequest,
        trace: TraceLevel,
        env_id: EnvironmentId,
        project_id: ProjectId,
    ) -> Vec<EvalResult> {
        let flag_id_str = find_flag_id(snapshot, &req.flag_key).to_string();
        let want_trace = trace == TraceLevel::Full;

        // ── Flag missing or archived → short-circuit (per existing SDK
        //    semantics — core engine never sees a missing flag) ─────────
        let Some(proto_flag) = snapshot.flag(&req.flag_key) else {
            return req
                .contexts
                .iter()
                .enumerate()
                .map(|(idx, ctx)| {
                    let event = build_event(
                        &req.flag_key,
                        &flag_id_str,
                        "",
                        EvalOutcome::FlagNotFound.as_str(),
                        "",
                        want_trace,
                        ctx,
                    );
                    self.event_queue.send(event);
                    EvalResult {
                        flag_key: req.flag_key.clone(),
                        variant_key: String::new(),
                        variant_value: serde_json::Value::Null,
                        outcome: EvalOutcome::FlagNotFound,
                        context_index: idx,
                        trace: None,
                    }
                })
                .collect();
        };

        if proto_flag.archived {
            // Archived flags behave like missing flags at the SDK boundary.
            let (default_key, default_value) = default_variant(proto_flag);
            return req
                .contexts
                .iter()
                .enumerate()
                .map(|(idx, ctx)| {
                    let event = build_event(
                        &req.flag_key,
                        &flag_id_str,
                        &default_key,
                        EvalOutcome::FlagNotFound.as_str(),
                        "",
                        want_trace,
                        ctx,
                    );
                    self.event_queue.send(event);
                    EvalResult {
                        flag_key: req.flag_key.clone(),
                        variant_key: default_key.clone(),
                        variant_value: default_value.clone(),
                        outcome: EvalOutcome::FlagNotFound,
                        context_index: idx,
                        trace: None,
                    }
                })
                .collect();
        }

        // ── Collect all referenced segment IDs across all rules ──────────
        let mut all_seg_ids: HashSet<SegmentId> = HashSet::new();
        for rule in &proto_flag.rules {
            if let Ok(cond) = serde_json::from_slice::<ConditionExpr>(&rule.rule_payload) {
                collect_segment_ids(&cond, &mut all_seg_ids);
            }
        }

        // ── Resolve list-segment + rule-based-segment definitions ────────
        // Partition the referenced segment IDs into rule-based + list-based.
        // Build a Vec<SegmentDefinition> for the rule-based ones (core
        // engine evaluates them in-process), and a list of list-segment IDs
        // for the LRU + REST lookup.
        let mut rule_based_segments: Vec<SegmentDefinition> = Vec::new();
        let mut list_segment_ids: Vec<String> = Vec::new();
        for seg_id in &all_seg_ids {
            let id_str = seg_id.as_uuid().to_string();
            if let Some(rule_seg) = snapshot.rule_segment(&id_str) {
                let rules: Vec<Rule> =
                    serde_json::from_slice(&rule_seg.rule_payload).unwrap_or_default();
                rule_based_segments.push(SegmentDefinition::RuleBased(RuleBasedSegment {
                    id: *seg_id,
                    rules,
                }));
            } else if snapshot.list_segment(&id_str).is_some() {
                list_segment_ids.push(id_str);
            }
        }

        // ── Resolve list-segment membership for every context in the
        //    bundle, via the LRU + on-demand REST fetcher. Aggregates into
        //    a single ListMembershipIndex consumed by evaluate_flag. ─────
        let memberships = self
            .resolve_list_memberships(&req.contexts, &list_segment_ids)
            .await;

        // ── Convert proto FeatureFlag → core Flag and call evaluate_flag ─
        let Some(core_flag) = convert_proto_flag_to_core(proto_flag) else {
            // Conversion failed (e.g. unknown rule output) — return the
            // proto-side default variant for every context. This branch
            // mirrors the prior `Output::None` skip path in the legacy
            // evaluate_inner.
            let (default_key, default_value) = default_variant(proto_flag);
            return req
                .contexts
                .iter()
                .enumerate()
                .map(|(idx, ctx)| {
                    let event = build_event(
                        &req.flag_key,
                        &flag_id_str,
                        &default_key,
                        EvalOutcome::DefaultRule.as_str(),
                        "",
                        want_trace,
                        ctx,
                    );
                    self.event_queue.send(event);
                    EvalResult {
                        flag_key: req.flag_key.clone(),
                        variant_key: default_key.clone(),
                        variant_value: default_value.clone(),
                        outcome: EvalOutcome::DefaultRule,
                        context_index: idx,
                        trace: None,
                    }
                })
                .collect();
        };

        // ── Evaluate, gating on prerequisites resolved from the snapshot ─
        //
        // When the flag has NO prerequisites, the cheap batch path
        // (`evaluate_flag` over the whole context bundle) is used unchanged.
        //
        // When it DOES, prerequisites are resolved LOCALLY + TRANSITIVELY
        // from the snapshot (the SDK holds every flag definition). Resolution
        // is per-context — a prerequisite flag may itself resolve to a
        // different variant for a different subject — so we evaluate each
        // context with its own pre-resolved `evaluated_flags` map and call
        // `evaluate_flag_with_prerequisites`. A prerequisite flag missing from
        // the snapshot is simply absent from the map ⇒ unmet ⇒ the engine
        // returns the fallback variant (fail-closed), matching the preview
        // path (Phase 4).
        let core_results = if core_flag.prerequisites.is_empty() {
            evaluate_flag(
                &core_flag,
                &req.contexts,
                &rule_based_segments,
                &memberships,
                env_id,
                project_id,
                trace,
            )
        } else {
            let mut acc = Vec::with_capacity(req.contexts.len());
            for ctx in &req.contexts {
                let single = std::slice::from_ref(ctx);
                let evaluated_flags = self
                    .resolve_prerequisite_map(snapshot, &core_flag, single, env_id, project_id)
                    .await;
                let mut one = evaluate_flag_with_prerequisites(
                    &core_flag,
                    single,
                    &rule_based_segments,
                    &memberships,
                    &evaluated_flags,
                    env_id,
                    project_id,
                    trace,
                );
                acc.append(&mut one);
            }
            acc
        };

        // ── Map core results → SDK results, emit one event per entry ────
        core_results
            .into_iter()
            .enumerate()
            .map(|(idx, core_res)| {
                let outcome = match core_res.outcome {
                    CoreEvalOutcome::RuleMatch { rule_index } => {
                        EvalOutcome::Matched { rule_index }
                    }
                    CoreEvalOutcome::DefaultFallthrough => EvalOutcome::DefaultRule,
                    CoreEvalOutcome::DefaultRuleDistribution => {
                        EvalOutcome::DefaultRuleDistribution
                    }
                    CoreEvalOutcome::FlagDisabled => EvalOutcome::Disabled,
                    CoreEvalOutcome::PrerequisiteFailed { .. } => EvalOutcome::PrerequisiteFailed,
                };
                let variant_value_json = core_variant_value_to_json(&core_res.variant_value);
                let matched_rule_id = core_res
                    .trace
                    .as_ref()
                    .and_then(|t| t.fired_rule_id.as_ref().map(|r| r.to_string()))
                    .unwrap_or_default();
                let ctx = req
                    .contexts
                    .get(idx)
                    .expect("evaluate_flag returns one result per context");
                let event = build_event(
                    &req.flag_key,
                    &flag_id_str,
                    &core_res.variant_key,
                    outcome.as_str(),
                    &matched_rule_id,
                    want_trace,
                    ctx,
                );
                self.event_queue.send(event);
                EvalResult {
                    flag_key: req.flag_key.clone(),
                    variant_key: core_res.variant_key,
                    variant_value: variant_value_json,
                    outcome,
                    context_index: idx,
                    trace: core_res.trace,
                }
            })
            .collect()
    }

    /// Resolve list-segment memberships for the request's context bundle.
    ///
    /// For each `(context_type, context_key)` in `contexts`, consults the
    /// LRU first; on miss, batch-fetches via the SDK's existing
    /// [`MembershipBatchFetcher`] and writes the result into the LRU.
    /// The returned [`ListMembershipIndex`] is keyed by `(type, key)` so
    /// `evaluate_flag` can fold it into its segment-resolution loop.
    async fn resolve_list_memberships(
        &self,
        contexts: &[Context],
        list_segment_ids: &[String],
    ) -> ListMembershipIndex {
        let mut index = ListMembershipIndex::new();
        if list_segment_ids.is_empty() || contexts.is_empty() {
            return index;
        }

        // Collect (context_type, context_key) tuples that miss the LRU and
        // must be batch-fetched. Pre-populate the index from any LRU hits
        // we find along the way.
        let mut on_miss: Vec<ContextKey> = Vec::new();
        for ctx in contexts {
            if let Some(map) = self.membership_cache.get(&ctx.context_type, &ctx.key) {
                let mut set: HashSet<SegmentId> = HashSet::new();
                for id_str in list_segment_ids {
                    if *map.get(id_str).unwrap_or(&false)
                        && let Ok(uuid) = Uuid::parse_str(id_str)
                    {
                        set.insert(SegmentId::from_uuid(uuid));
                    }
                }
                index.insert(ctx.context_type.clone(), ctx.key.clone(), set);
            } else {
                on_miss.push((ctx.context_type.clone(), ctx.key.clone()));
            }
        }

        if !on_miss.is_empty() {
            match self
                .membership_fetcher
                .fetch(on_miss.clone(), list_segment_ids.to_vec())
                .await
            {
                Ok(maps) => {
                    for ((ctx_type, ctx_key), membership) in on_miss.iter().zip(maps) {
                        let mut set: HashSet<SegmentId> = HashSet::new();
                        for id_str in list_segment_ids {
                            if *membership.get(id_str).unwrap_or(&false)
                                && let Ok(uuid) = Uuid::parse_str(id_str)
                            {
                                set.insert(SegmentId::from_uuid(uuid));
                            }
                        }
                        index.insert(ctx_type.clone(), ctx_key.clone(), set);
                        // Populate LRU for future evaluations.
                        self.membership_cache.insert(ctx_type, ctx_key, membership);
                    }
                }
                Err(e) => {
                    warn!(
                        error = %e,
                        "on-demand list-segment fetch failed; treating as non-member"
                    );
                    // Leave missing contexts absent from the index — the
                    // engine treats absent entries as "no list segments
                    // matched" (equivalent to an empty set).
                }
            }
        }

        index
    }

    /// Resolve the cross-flag `evaluated_flags` map for `flag`'s prerequisite
    /// gate, transitively, from the local snapshot.
    ///
    /// Mirrors the flag-service preview path
    /// ([`stitchd_flag_service`]'s `resolve_prerequisite_variant_map`) but
    /// reads every flag definition from the SDK's in-memory snapshot rather
    /// than a database:
    ///
    /// 1. BFS the transitive prerequisite-flag closure (by key) starting from
    ///    `flag`'s own prerequisites; the dependent flag itself is excluded.
    /// 2. Convert each closure flag to a core `Flag` (deterministic IDs), and
    ///    gather the union of segments they reference.
    /// 3. Resolve those segments' rule-based + list-segment membership for the
    ///    context bundle (reusing the LRU + on-demand fetcher).
    /// 4. Run [`evaluate_flags_with_prerequisites`] over the closure to get the
    ///    dependency-ordered resolved-variant map.
    ///
    /// A prerequisite flag missing from the snapshot is silently dropped from
    /// the closure ⇒ absent from the returned map ⇒ the engine's gate treats it
    /// as **unmet** (fail-closed → fallback). A prerequisite *cycle* makes the
    /// orchestrator return an error; we treat that conservatively as an empty
    /// map (every prerequisite unmet → fallback) and log a warning.
    async fn resolve_prerequisite_map(
        &self,
        snapshot: &DefinitionSnapshot,
        flag: &Flag,
        contexts: &[Context],
        env_id: EnvironmentId,
        project_id: ProjectId,
    ) -> HashMap<FlagId, Option<VariantId>> {
        // ── 1. BFS the prerequisite-flag closure by KEY ──────────────────
        //
        // The core gate carries IDs only; the BFS walks on KEYS, which live on
        // the proto `FeatureFlag.prerequisites`. Seed from the target flag's
        // proto prerequisite keys, then walk transitively.
        let own_key = flag.record.key.as_str().to_string();
        let mut closure_keys: HashSet<String> = HashSet::new();
        let mut queue: std::collections::VecDeque<String> = std::collections::VecDeque::new();
        if let Some(proto) = snapshot.flag(&own_key) {
            for p in &proto.prerequisites {
                queue.push_back(p.prerequisite_flag_key.clone());
            }
        }
        while let Some(key) = queue.pop_front() {
            if key == own_key || key.is_empty() || !closure_keys.insert(key.clone()) {
                continue;
            }
            // Missing-from-snapshot prerequisite ⇒ leave it out of the closure
            // (absent ⇒ unmet at the gate). Enqueue its own prerequisites.
            if let Some(proto) = snapshot.flag(&key) {
                for p in &proto.prerequisites {
                    if !closure_keys.contains(&p.prerequisite_flag_key) {
                        queue.push_back(p.prerequisite_flag_key.clone());
                    }
                }
            }
        }

        if closure_keys.is_empty() {
            return HashMap::new();
        }

        // ── 2. Convert closure flags + gather referenced segments ─────────
        let mut flag_entries: Vec<(FlagId, Vec<Rule>, Vec<CorePrerequisite>)> =
            Vec::with_capacity(closure_keys.len());
        // Per closure-flag gate, retained to FOLD the prerequisite fallback into
        // the resolved map after the orchestrator runs (see step 4) — the
        // orchestrator resolves variants from rules ONLY and does not itself
        // apply the gate, so transitive fallback must be folded by the caller.
        let mut closure_gates: HashMap<FlagId, PrerequisiteGate> = HashMap::new();
        let mut all_seg_ids: HashSet<SegmentId> = HashSet::new();
        for key in &closure_keys {
            let Some(proto) = snapshot.flag(key) else {
                continue;
            };
            // Skip disabled flags: a disabled flag resolves to NO variant, so
            // it's correctly absent from the map ⇒ any gate requiring it is
            // unmet. (Including it would risk its rules resolving a variant.)
            if !proto.enabled || proto.archived {
                continue;
            }
            let Some(core) = convert_proto_flag_to_core(proto) else {
                continue;
            };
            for rule in &proto.rules {
                if let Ok(cond) = serde_json::from_slice::<ConditionExpr>(&rule.rule_payload) {
                    collect_segment_ids(&cond, &mut all_seg_ids);
                }
            }
            let rules: Vec<Rule> = core.rules.iter().map(|r| r.rule.clone()).collect();
            let prereqs = core.prerequisites.prerequisites.clone();
            // Key the closure flag by its DETERMINISTIC-from-key FlagId (NOT
            // its wire UUID): the dependent flag's gate references prerequisite
            // flags by key (SDK-sync leaves the prereq `_id` empty), so its
            // `prerequisite_flag_id` is also deterministic-from-key. Both sides
            // must agree for the resolved-variant map lookup to hit.
            let closure_flag_id = resolve_flag_id("", key);
            // The fallback variant must be re-keyed onto this flag's own
            // variants (it was resolved by `build_prerequisite_gate` against the
            // proto's variant keys → deterministic ids); we recompute it here
            // for the fold using the proto's `fallback_variant_key`.
            let fallback_variant_id = if proto.fallback_variant_key.is_empty() {
                None
            } else {
                Some(deterministic_variant_id(key, &proto.fallback_variant_key))
            };
            closure_gates.insert(
                closure_flag_id,
                PrerequisiteGate {
                    prerequisites: prereqs.clone(),
                    fallback_variant_id,
                },
            );
            flag_entries.push((closure_flag_id, rules, prereqs));
        }

        if flag_entries.is_empty() {
            return HashMap::new();
        }

        // ── 3. Resolve segments for the closure ──────────────────────────
        let mut rule_based_segments: Vec<SegmentDefinition> = Vec::new();
        let mut list_segment_ids: Vec<String> = Vec::new();
        for seg_id in &all_seg_ids {
            let id_str = seg_id.as_uuid().to_string();
            if let Some(rule_seg) = snapshot.rule_segment(&id_str) {
                let rules: Vec<Rule> =
                    serde_json::from_slice(&rule_seg.rule_payload).unwrap_or_default();
                rule_based_segments.push(SegmentDefinition::RuleBased(RuleBasedSegment {
                    id: *seg_id,
                    rules,
                }));
            } else if snapshot.list_segment(&id_str).is_some() {
                list_segment_ids.push(id_str);
            }
        }
        let list_memberships = self
            .resolve_list_memberships(contexts, &list_segment_ids)
            .await;
        let resolved_segments =
            resolve_segments_for_bundle(contexts, &rule_based_segments, &list_memberships);

        // ── 4. Topo-sort + resolve in dependency order ───────────────────
        let _ = project_id; // env-only salt today; kept for signature parity
        let base_input = EvaluationInput {
            contexts,
            resolved_segments,
            evaluated_flags: HashMap::new(),
        };
        let _ = env_id;
        match evaluate_flags_with_prerequisites(&flag_entries, &base_input) {
            Ok(mut map) => {
                fold_prerequisite_fallbacks(&mut map, &flag_entries, &closure_gates);
                map
            }
            Err(e) => {
                warn!(
                    error = %e,
                    flag = %own_key,
                    "prerequisite resolution failed (cycle?); treating all prerequisites as unmet"
                );
                HashMap::new()
            }
        }
    }
}

// ============================================================================
// Helpers
// ============================================================================

/// Resolve the set of segments a single context bundle belongs to, merging
/// rule-based segment membership (evaluated via the core segment evaluator)
/// with the pre-resolved list-segment membership index.
///
/// Mirrors the flag-service preview's `resolve_segments_for_bundle`, but the
/// SDK's list memberships come from a [`ListMembershipIndex`] (keyed by
/// `(context_type, context_key)`) rather than a per-context `HashSet`.
fn resolve_segments_for_bundle(
    contexts: &[Context],
    rule_based_segments: &[SegmentDefinition],
    list_memberships: &ListMembershipIndex,
) -> HashSet<SegmentId> {
    let mut resolved: HashSet<SegmentId> = HashSet::new();
    // Rule-based segments: evaluate each against the FULL bundle (segment
    // rules may reference multiple context types) — matching preview semantics.
    for seg in rule_based_segments {
        if let SegmentDefinition::RuleBased(rb) = seg
            && rb.evaluate(contexts).map(|m| m.matched).unwrap_or(false)
        {
            resolved.insert(rb.id);
        }
    }
    // List segments: fold in any membership the index recorded for each
    // (context_type, context_key) in the bundle.
    for ctx in contexts {
        if let Some(set) = list_memberships.get(&ctx.context_type, &ctx.key) {
            resolved.extend(set.iter().copied());
        }
    }
    resolved
}

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

// ── Proto → core conversion (Phase 6) ────────────────────────────────────────
//
// The SDK's `DefinitionSnapshot` stores the proto `FeatureFlag` as-is — the
// unified evaluation path needs a `stitchd_core::flag::Flag` instead. The
// helpers below do that conversion lazily on the evaluation hot path.
//
// A future SDK refresh may pre-convert at snapshot-load time; until then the
// per-evaluation conversion is fine — `FeatureFlag` is small (variants +
// rules) and conversion is allocation-only (no parsing of large JSON
// payloads beyond the rule_payload bytes that the existing path already
// deserialises).

/// Convert a proto [`FeatureFlag`] to a core [`Flag`] suitable for
/// [`evaluate_flag`].
///
/// Returns `None` when conversion can't proceed (e.g. malformed rule
/// payload, unknown rule output). The caller falls back to returning the
/// flag's default variant for every context — consistent with the prior
/// behaviour of the legacy `evaluate_inner`.
///
/// Note: the proto `FeatureFlag` does NOT (yet) carry
/// `default_rule_distribution` — that field lives only on the admin RPCs
/// and is sealed for Phase 6. The conversion sets
/// `record.default_rule_distribution = None`; once the proto SDK service
/// ships the field (separate track), this helper will be updated to
/// populate it from the wire.
fn convert_proto_flag_to_core(proto: &FeatureFlag) -> Option<Flag> {
    // ── Variants ──────────────────────────────────────────────────────────
    //
    // VariantIds are derived deterministically from `(flag_key, variant_key)`
    // — NOT minted fresh — so the same variant resolves to the same
    // `VariantId` across separate conversions of *different* flags. This is
    // what makes the cross-flag prerequisite gate work in the SDK: a
    // dependent flag's `required_variant_id` (built from the prerequisite
    // flag's `(key, variant_key)`) must equal the `VariantId` that the
    // prerequisite flag's own conversion produces for that variant.
    let variants: Vec<CoreVariant> = proto
        .variants
        .iter()
        .filter_map(|v| {
            let value = proto_variant_value_to_core(v.value.as_ref()?)?;
            Some(CoreVariant {
                id: deterministic_variant_id(&proto.key, &v.key),
                key: v.key.clone(),
                value,
            })
        })
        .collect();

    // Map variant_key → VariantId so rule outputs (which are keyed by
    // string) can be re-bound to the core `VariantId` shape.
    let variant_id_by_key: HashMap<String, VariantId> =
        variants.iter().map(|v| (v.key.clone(), v.id)).collect();

    // ── Flag key + value_type ────────────────────────────────────────────
    let flag_key = FlagKey::new(proto.key.clone()).ok()?;
    let value_type = proto_value_type_to_core(proto.value_type);

    // ── default_variant_id ────────────────────────────────────────────────
    let default_variant_id = if proto.default_variant_key.is_empty() {
        None
    } else {
        variant_id_by_key.get(&proto.default_variant_key).copied()
    };

    // ── Flag id ──────────────────────────────────────────────────────────
    // Resolve the flag's `FlagId` consistently across conversions: prefer the
    // wire `flag_id` UUID (populated in SDK definition-sync), else derive
    // deterministically from the flag key. Determinism matters because a flag
    // referenced as a *prerequisite* (by key) must resolve to the SAME
    // `FlagId` whether it's converted as the eval target or as a closure
    // member during cross-flag prerequisite resolution.
    let flag_id = resolve_flag_id(&proto.flag_id, &proto.key);

    // ── Rules ────────────────────────────────────────────────────────────
    let mut rules: Vec<CoreFlagRule> = Vec::with_capacity(proto.rules.len());
    for (i, proto_rule) in proto.rules.iter().enumerate() {
        if let Some(core_rule) =
            convert_proto_flag_rule_to_core(flag_id, i as i32, proto_rule, &variant_id_by_key)
        {
            rules.push(core_rule);
        }
    }

    // ── Assemble FlagRecord ──────────────────────────────────────────────
    let record = FlagRecord {
        id: flag_id,
        project_id: ProjectId::new(),
        key: flag_key,
        name: proto.name.clone(),
        description: proto.description.clone(),
        value_type,
        enabled: proto.enabled,
        default_variant_id,
        // The SDK proto wire does NOT carry `default_rule_distribution`
        // yet (sealed Phase 3 scope). The SDK starts with `None`; the core
        // engine's default-rule-distribution path is exercised when the
        // caller constructs a `Flag` directly (e.g. in tests). Once the
        // SDK proto ships the field, this slot will populate from the
        // wire and the SDK gains the feature automatically per Task 8.
        default_rule_distribution: None,
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
        deleted_at: if proto.archived {
            Some(Utc::now())
        } else {
            None
        },
        version: proto.version as i64,
    };

    // ── Prerequisite gate (Phase 7, flag_lifecycle_20260604) ─────────────
    //
    // SDK definition-sync carries prerequisites by KEY (the `_id` fields are
    // empty on the wire), so we resolve them to the deterministic IDs above:
    //
    // - `prerequisite_flag_id`  ← `resolve_flag_id(key)` of the prereq flag.
    // - `required_variant_id`   ← `deterministic_variant_id(prereq_flag_key,
    //                              required_variant_key)` — keyed on the
    //                              prerequisite flag so it matches that flag's
    //                              own converted variant IDs.
    // - `fallback_variant_id`   ← THIS flag's own variant named by
    //                              `fallback_variant_key` (empty ⇒ None ⇒ the
    //                              engine uses the off/disabled default).
    let prerequisites = build_prerequisite_gate(proto, &variant_id_by_key);

    Some(Flag {
        record,
        hashing_config: vec![],
        rules,
        variants,
        prerequisites,
    })
}

/// Build the core [`PrerequisiteGate`] from a proto [`FeatureFlag`]'s
/// `prerequisites` + `fallback_variant_key`.
///
/// IDs are resolved deterministically from keys (see
/// [`convert_proto_flag_to_core`]); the prerequisite flag's `required_variant`
/// is keyed on the *prerequisite* flag (not this flag), and the fallback
/// variant resolves against THIS flag's own variant map.
fn build_prerequisite_gate(
    proto: &FeatureFlag,
    own_variant_id_by_key: &HashMap<String, VariantId>,
) -> PrerequisiteGate {
    let prerequisites: Vec<CorePrerequisite> = proto
        .prerequisites
        .iter()
        .map(|p| {
            // Prefer the wire `_id` fields when populated (admin/preview
            // snapshots), else resolve from keys (SDK definition-sync).
            let prerequisite_flag_id = if p.prerequisite_flag_id.is_empty() {
                resolve_flag_id("", &p.prerequisite_flag_key)
            } else {
                resolve_flag_id(&p.prerequisite_flag_id, &p.prerequisite_flag_key)
            };
            let required_variant_id = if p.required_variant_id.is_empty() {
                deterministic_variant_id(&p.prerequisite_flag_key, &p.required_variant_key)
            } else {
                Uuid::parse_str(&p.required_variant_id)
                    .map(VariantId::from_uuid)
                    .unwrap_or_else(|_| {
                        deterministic_variant_id(&p.prerequisite_flag_key, &p.required_variant_key)
                    })
            };
            CorePrerequisite {
                prerequisite_flag_id,
                required_variant_id,
            }
        })
        .collect();

    // Empty `fallback_variant_key` ⇒ None ⇒ the gate uses the flag's
    // off/disabled default variant (per `Flag::prerequisite_fallback_variant`).
    let fallback_variant_id = if proto.fallback_variant_key.is_empty() {
        None
    } else {
        own_variant_id_by_key
            .get(&proto.fallback_variant_key)
            .copied()
    };

    PrerequisiteGate {
        prerequisites,
        fallback_variant_id,
    }
}

/// Resolve a flag's `FlagId` from its wire UUID (preferred) or deterministically
/// from its key. Used for both the flag itself and prerequisite-flag references
/// so cross-flag IDs line up regardless of conversion order.
fn resolve_flag_id(wire_uuid: &str, flag_key: &str) -> FlagId {
    Uuid::parse_str(wire_uuid)
        .map(FlagId::from_uuid)
        .unwrap_or_else(|_| FlagId::from_uuid(deterministic_uuid("flag", flag_key, "")))
}

/// Deterministically derive a [`VariantId`] from `(flag_key, variant_key)`.
fn deterministic_variant_id(flag_key: &str, variant_key: &str) -> VariantId {
    VariantId::from_uuid(deterministic_uuid("variant", flag_key, variant_key))
}

/// Derive a stable v4-shaped [`Uuid`] from a domain tag + up to two key parts.
///
/// This is a deterministic, collision-resistant fold (FNV-1a over the tagged,
/// length-prefixed inputs into a 128-bit value) — NOT a cryptographic hash and
/// NOT a real UUIDv5 (the crate's `v5` feature isn't enabled), but it's stable
/// across processes and conversions, which is all the SDK's local prerequisite
/// resolution needs: the same key always maps to the same ID so a dependent
/// flag's `required_variant_id` matches the prerequisite flag's converted
/// variant. The version/variant nibbles are set so the value is a well-formed
/// RFC-4122 UUID.
fn deterministic_uuid(tag: &str, part_a: &str, part_b: &str) -> Uuid {
    // FNV-1a 64-bit, run twice with different seeds to fill 128 bits.
    fn fnv1a(seed: u64, bytes: &[u8]) -> u64 {
        let mut hash = seed;
        for &b in bytes {
            hash ^= u64::from(b);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
        hash
    }
    // Length-prefix each part so distinct splits can't collide (e.g.
    // ("ab","c") vs ("a","bc")).
    let mut buf: Vec<u8> = Vec::new();
    for part in [tag, part_a, part_b] {
        buf.extend_from_slice(&(part.len() as u64).to_le_bytes());
        buf.extend_from_slice(part.as_bytes());
    }
    let hi = fnv1a(0xcbf2_9ce4_8422_2325, &buf);
    let lo = fnv1a(0x84222325_cbf29ce4, &buf);
    let mut value = (u128::from(hi) << 64) | u128::from(lo);
    // Force RFC-4122 version 4 + variant bits so it's a valid UUID.
    value &= !(0xF000_u128 << 64);
    value |= 0x4000_u128 << 64;
    value &= !(0xC000_u128 << 48);
    value |= 0x8000_u128 << 48;
    Uuid::from_u128(value)
}

/// Convert a proto [`ProtoFlagRule`] to a core [`CoreFlagRule`].
///
/// Returns `None` when the rule's condition or output can't be parsed.
/// Variant-keyed outputs are bound to the variant's `VariantId` via the
/// supplied `variant_id_by_key` map.
fn convert_proto_flag_rule_to_core(
    flag_id: FlagId,
    rule_index: i32,
    proto: &ProtoFlagRule,
    variant_id_by_key: &HashMap<String, VariantId>,
) -> Option<CoreFlagRule> {
    let condition: ConditionExpr = serde_json::from_slice(&proto.rule_payload).ok()?;

    let output = match &proto.output {
        Some(ProtoOutput::VariantKey(key)) => {
            let vid = variant_id_by_key.get(key).copied()?;
            RuleOutput::Variant(vid)
        }
        Some(ProtoOutput::Allocation(alloc)) => proto_allocation_to_core(alloc, variant_id_by_key)?,
        None => return None,
    };

    let rule_id = Uuid::parse_str(&proto.rule_id)
        .map(RuleId::from_uuid)
        .unwrap_or_else(|_| RuleId::new());
    let name = if proto.name.is_empty() {
        None
    } else {
        Some(proto.name.clone())
    };

    Some(CoreFlagRule {
        flag_id,
        rule_index,
        rule: Rule {
            id: rule_id,
            name,
            condition,
            output,
        },
    })
}

/// Convert a proto [`PercentageAllocation`] to a core
/// [`RuleOutput::Percentage`].
///
/// Phase 3 of `flag_eval_unify_20260522` added `hash_inputs` (ordered selector
/// list) alongside the legacy `context_hash_specs` map; this helper prefers
/// the new field when present and falls back to the legacy map for
/// back-compat. The fallback uses canonical-sort semantics
/// (`context_type ASC, parameter ASC within type`) matching the PG
/// migration backfill so bucket assignments remain stable across the
/// dual-schema state.
fn proto_allocation_to_core(
    alloc: &PercentageAllocation,
    variant_id_by_key: &HashMap<String, VariantId>,
) -> Option<RuleOutput> {
    let spec = proto_to_core_hash_input_spec(alloc);
    // `RuleOutput::Percentage` keeps the legacy `Vec<PercentageTarget>`
    // shape — convert through that bridge until Phase 5/6 cuts over storage.
    let targets = hash_input_spec_to_targets(&spec);

    let weights = alloc
        .buckets
        .iter()
        .filter_map(|b| {
            let vid = variant_id_by_key.get(&b.variant_key).copied()?;
            Some((vid, b.weight_bp))
        })
        .collect();

    // Carry the exclusion gate proto→core so the SDK's in-memory snapshot
    // holds it; `evaluate_flag` reads it on the zero-DB-lookup eval path.
    // Proto bucket bounds are `u32`; the domain uses `u16` (buckets live in
    // `[0, 9999]`), so we truncate via `as u16`.
    let exclusion_gate = alloc.exclusion_gate.as_ref().map(|g| ExclusionGate {
        group_salt: g.group_salt.clone(),
        context_type: g.context_type.clone(),
        bucket_lo: g.bucket_lo as u16,
        bucket_hi: g.bucket_hi as u16,
    });

    Some(RuleOutput::Percentage {
        targets,
        weights,
        exclusion_gate,
    })
}

/// Read the canonical [`HashInputSpec`] out of a proto
/// [`PercentageAllocation`].
///
/// Prefer the new `hash_inputs` field; fall back to canonical-sort of the
/// legacy `context_hash_specs` map. Public to the SDK module so the
/// future test fixtures + integration tests can validate the proto wire
/// shape without going through `evaluate_flag`.
pub(crate) fn proto_to_core_hash_input_spec(alloc: &PercentageAllocation) -> HashInputSpec {
    if !alloc.hash_inputs.is_empty() {
        let selectors: Vec<HashSelector> = alloc
            .hash_inputs
            .iter()
            .filter_map(|sel| match &sel.selector {
                Some(ProtoHashSelectorOneof::ContextKey(ck)) => Some(HashSelector::ContextKey {
                    context_type: ck.context_type.clone(),
                }),
                Some(ProtoHashSelectorOneof::ContextParameter(cp)) => {
                    Some(HashSelector::ContextParameter {
                        context_type: cp.context_type.clone(),
                        parameter: cp.parameter.clone(),
                    })
                }
                None => None,
            })
            .collect();
        return HashInputSpec::new(selectors);
    }

    // Legacy-only path: canonical sort (context_type ASC, parameter ASC
    // within type). Mirrors `crates/stitchd-db/migrations/20260522000001_...`
    // backfill so legacy fixtures + dual-schema state stay byte-equivalent.
    let mut entries: Vec<(&String, &Vec<String>)> = alloc
        .context_hash_specs
        .iter()
        .map(|(k, v)| (k, &v.parameter_names))
        .collect();
    entries.sort_by(|a, b| a.0.cmp(b.0));

    let mut selectors: Vec<HashSelector> = Vec::new();
    for (ctx_type, params) in entries {
        if params.is_empty() {
            selectors.push(HashSelector::ContextKey {
                context_type: ctx_type.clone(),
            });
        } else {
            let mut sorted_params: Vec<&String> = params.iter().collect();
            sorted_params.sort();
            for param in sorted_params {
                selectors.push(HashSelector::ContextParameter {
                    context_type: ctx_type.clone(),
                    parameter: param.clone(),
                });
            }
        }
    }
    HashInputSpec::new(selectors)
}

/// Bridge: [`HashInputSpec`] → `Vec<PercentageTarget>` for the
/// `RuleOutput::Percentage` shape (which still carries the legacy target
/// type). Mirrors `engine::hash_input_spec_from_targets` but in the
/// opposite direction.
fn hash_input_spec_to_targets(spec: &HashInputSpec) -> Vec<PercentageTarget> {
    spec.selectors
        .iter()
        .map(|s| match s {
            HashSelector::ContextKey { context_type } => PercentageTarget {
                context_type: context_type.clone(),
                field: TargetField::Key,
            },
            HashSelector::ContextParameter {
                context_type,
                parameter,
            } => PercentageTarget {
                context_type: context_type.clone(),
                field: TargetField::Parameter(parameter.clone()),
            },
        })
        .collect()
}

/// Convert a proto `VariantValue` to a core [`CoreVariantValue`].
fn proto_variant_value_to_core(
    v: &stitchd_proto::flags::v1::VariantValue,
) -> Option<CoreVariantValue> {
    use stitchd_proto::flags::v1::variant_value::Value;
    match &v.value {
        Some(Value::BoolValue(b)) => Some(CoreVariantValue::BoolValue(*b)),
        Some(Value::IntValue(i)) => Some(CoreVariantValue::IntValue(*i)),
        Some(Value::DoubleValue(d)) => Some(CoreVariantValue::DoubleValue(*d)),
        Some(Value::StringValue(s)) => Some(CoreVariantValue::StrValue(s.clone())),
        Some(Value::JsonValue(s)) => serde_json::from_str(s)
            .ok()
            .map(CoreVariantValue::JsonValue),
        None => None,
    }
}

/// Convert a core [`CoreVariantValue`] to a `serde_json::Value` for the
/// SDK's public `EvalResult.variant_value` shape.
fn core_variant_value_to_json(v: &CoreVariantValue) -> serde_json::Value {
    match v {
        CoreVariantValue::BoolValue(b) => serde_json::Value::Bool(*b),
        CoreVariantValue::IntValue(i) => serde_json::json!(i),
        CoreVariantValue::DoubleValue(d) => serde_json::json!(d),
        CoreVariantValue::StrValue(s) => serde_json::Value::String(s.clone()),
        CoreVariantValue::JsonValue(j) => j.clone(),
    }
}

/// Convert a proto `FlagValueType` (int repr) to a core [`FlagValueType`].
fn proto_value_type_to_core(vt: i32) -> FlagValueType {
    use stitchd_proto::flags::v1::FlagValueType as Proto;
    match Proto::try_from(vt).unwrap_or(Proto::Bool) {
        Proto::Bool => FlagValueType::Bool,
        Proto::Int => FlagValueType::Int,
        Proto::Double => FlagValueType::Double,
        Proto::String => FlagValueType::Str,
        Proto::Json => FlagValueType::Json,
        Proto::Unspecified => FlagValueType::Bool,
    }
}

/// Parse the snapshot's environment-id string into a core [`EnvironmentId`].
/// Returns a freshly-minted ID when the snapshot has an empty / malformed
/// string (test fixtures + bare snapshots from `DefinitionSnapshot::default()`).
fn parse_env_id(env_id_str: &str) -> EnvironmentId {
    Uuid::parse_str(env_id_str)
        .map(EnvironmentId::from_uuid)
        .unwrap_or_else(|_| EnvironmentId::new())
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
            rule_id: String::new(),
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
            .evaluate(&[EvalRequest::single("no-such-flag", ctx)], TraceLevel::Off)
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
            event_definitions: vec![],
        });
        let client = sdk_client_with_snapshot(snap);
        let ctx = Context::new("user", "alice");
        let results = client
            .evaluate(&[EvalRequest::single("feature-x", ctx)], TraceLevel::Off)
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
            event_definitions: vec![],
        });
        let client = sdk_client_with_snapshot(snap);
        let ctx = Context::new("user", "alice");
        let results = client
            .evaluate(&[EvalRequest::single("show-banner", ctx)], TraceLevel::Off)
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
            event_definitions: vec![],
        });
        let client = sdk_client_with_snapshot(snap);

        // Context WITH plan=pro → should match rule
        let ctx_pro =
            Context::new("user", "alice").with_parameter("plan", CoreParam::Str("pro".into()));
        let res = client
            .evaluate(
                &[EvalRequest::single("checkout-flow", ctx_pro)],
                TraceLevel::Off,
            )
            .await;
        assert_eq!(res[0].outcome, EvalOutcome::Matched { rule_index: 0 });
        assert_eq!(res[0].variant_key, "new-checkout");

        // Context WITHOUT plan → no match, default
        let ctx_free = Context::new("user", "bob");
        let res2 = client
            .evaluate(
                &[EvalRequest::single("checkout-flow", ctx_free)],
                TraceLevel::Off,
            )
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
            event_definitions: vec![],
        });
        let client = sdk_client_with_snapshot(snap);

        // pro user → member of segment → treatment
        let ctx_pro =
            Context::new("user", "alice").with_parameter("plan", CoreParam::Str("pro".into()));
        let res = client
            .evaluate(
                &[EvalRequest::single("feature-flag", ctx_pro)],
                TraceLevel::Off,
            )
            .await;
        assert_eq!(res[0].outcome, EvalOutcome::Matched { rule_index: 0 });
        assert_eq!(res[0].variant_key, "treatment");

        // free user → not member → control
        let ctx_free = Context::new("user", "bob");
        let res2 = client
            .evaluate(
                &[EvalRequest::single("feature-flag", ctx_free)],
                TraceLevel::Off,
            )
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
            event_definitions: vec![],
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
            .evaluate(&[EvalRequest::single("new-ui", ctx)], TraceLevel::Off)
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
            event_definitions: vec![],
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
            .evaluate(&[EvalRequest::single("vip-feature", ctx)], TraceLevel::Off)
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
            event_definitions: vec![],
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
            .evaluate(
                &[EvalRequest::single("vip-feature", ctx())],
                TraceLevel::Off,
            )
            .await;
        assert_eq!(recording_fetcher.call_count(), 1);

        // Second call → LRU hit → no fetch
        client
            .evaluate(
                &[EvalRequest::single("vip-feature", ctx())],
                TraceLevel::Off,
            )
            .await;
        assert_eq!(
            recording_fetcher.call_count(),
            1,
            "second call must use LRU, not fetcher"
        );
    }

    // ── Phase 6: evaluate(TraceLevel::Full) carries EvaluationTrace ─────────

    #[tokio::test]
    async fn evaluate_full_trace_includes_evaluation_trace() {
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
            environment_id: "00000000-0000-0000-0000-000000000001".into(),
            event_definitions: vec![],
        });
        let client = sdk_client_with_snapshot(snap);
        let ctx =
            Context::new("user", "alice").with_parameter("plan", CoreParam::Str("pro".into()));

        let results = client
            .evaluate(&[EvalRequest::single("upgrade-cta", ctx)], TraceLevel::Full)
            .await;
        assert_eq!(results.len(), 1);
        let r = &results[0];
        assert_eq!(r.variant_key, "treatment");
        assert_eq!(r.outcome, EvalOutcome::Matched { rule_index: 0 });
        let trace = r.trace.as_ref().expect("Full trace level → Some(trace)");
        assert_eq!(trace.fired_rule_name.as_deref(), Some("pro-user-rule"));
        assert_eq!(trace.rule_traces.len(), 1);
    }

    #[tokio::test]
    async fn evaluate_full_trace_for_flag_not_found_has_no_trace() {
        // Phase 6: when the flag is missing, the SDK short-circuits before
        // calling `evaluate_flag`, so no rich trace is constructed. The
        // result's `trace` is `None` even at TraceLevel::Full — the caller
        // distinguishes the missing-flag path from the rule-fired path via
        // the `outcome` field.
        let snap = DefinitionSnapshot::default();
        let client = sdk_client_with_snapshot(snap);
        let ctx = Context::new("user", "alice");
        let results = client
            .evaluate(&[EvalRequest::single("nonexistent", ctx)], TraceLevel::Full)
            .await;
        assert_eq!(results[0].outcome, EvalOutcome::FlagNotFound);
        assert!(results[0].trace.is_none());
    }

    // ── Phase 6 Task 1: EvalRequest accepts a multi-context bundle ──────────
    //
    // Demonstrates the new public shape: `contexts: Vec<Context>` instead of
    // `context: Context`. The same call applies a percentage rule that mixes
    // selectors across the bundle (key + parameter on distinct context
    // types), but here we only check the shape — see
    // `evaluate_cross_context_hash_selectors_match_core` below for the
    // hashing-correctness test (Phase 6 Task 4).
    #[tokio::test]
    async fn eval_request_accepts_multi_context_bundle() {
        let flag = simple_bool_flag("multi-ctx-flag");
        let snap = DefinitionSnapshot::from_proto(SyncDefinitionsResponse {
            flags: vec![flag],
            rule_segments: vec![],
            list_segments: vec![],
            server_timestamp_ms: 0,
            environment_id: "00000000-0000-0000-0000-000000000001".into(),
            event_definitions: vec![],
        });
        let client = sdk_client_with_snapshot(snap);

        let user =
            Context::new("user", "alice").with_parameter("plan", CoreParam::Str("pro".into()));
        let device =
            Context::new("device", "iphone-12").with_parameter("os", CoreParam::Str("ios".into()));
        let app = Context::new("application", "v2");

        let results = client
            .evaluate(
                &[EvalRequest {
                    flag_key: "multi-ctx-flag".into(),
                    contexts: vec![user, device, app],
                }],
                TraceLevel::Off,
            )
            .await;

        // One result per context in the bundle (core's `evaluate_flag`
        // returns one result per subject context). All three are
        // independent invocations of the same flag's default rule.
        assert_eq!(results.len(), 3, "one result per context in the bundle");
        for (i, r) in results.iter().enumerate() {
            assert_eq!(r.flag_key, "multi-ctx-flag");
            assert_eq!(r.outcome, EvalOutcome::DefaultRule);
            assert_eq!(r.context_index, i);
            assert_eq!(r.variant_key, "true");
        }
    }

    // ── Phase 6 Task 4: SDK-side cross-context hashing test ─────────────────
    //
    // EvalRequest carries a user + device + application bundle. The
    // percentage rule mixes selectors across contexts (`user.key`,
    // `user.params.tier`, `device.params.os`). The SDK's `evaluate` path
    // must match the core engine's bucket assignment for the same bundle
    // when called via `evaluate_flag` directly.
    #[tokio::test]
    async fn evaluate_cross_context_hash_selectors_match_core() {
        use stitchd_proto::flags::v1::flag_rule::Output as POut;
        use stitchd_proto::flags::v1::{
            AllocationBucket, ContextHashSpec, ContextKeySelector as ProtoCtxKeySel,
            ContextParameterSelector as ProtoCtxParamSel, HashSelector as ProtoHashSelMsg,
            PercentageAllocation, hash_selector::Selector as ProtoSelOneof,
        };

        // Build a flag with a single percentage rule that hashes on
        // user.key + user.params.tier + device.params.os.
        let alloc = PercentageAllocation {
            context_hash_specs: HashMap::new(),
            buckets: vec![
                AllocationBucket {
                    variant_key: "control".into(),
                    weight_bp: 5000,
                },
                AllocationBucket {
                    variant_key: "treatment".into(),
                    weight_bp: 5000,
                },
            ],
            hash_inputs: vec![
                ProtoHashSelMsg {
                    selector: Some(ProtoSelOneof::ContextKey(ProtoCtxKeySel {
                        context_type: "user".into(),
                    })),
                },
                ProtoHashSelMsg {
                    selector: Some(ProtoSelOneof::ContextParameter(ProtoCtxParamSel {
                        context_type: "user".into(),
                        parameter: "tier".into(),
                    })),
                },
                ProtoHashSelMsg {
                    selector: Some(ProtoSelOneof::ContextParameter(ProtoCtxParamSel {
                        context_type: "device".into(),
                        parameter: "os".into(),
                    })),
                },
            ],
            exclusion_gate: None,
        };

        let proto_rule = ProtoFlagRule {
            rule_payload: serde_json::to_vec(&ConditionExpr::And(vec![])).unwrap(),
            output: Some(POut::Allocation(alloc)),
            name: "rollout".into(),
            rule_id: String::new(),
        };
        let _ = ContextHashSpec::default(); // silence the unused import on the new path

        let flag = FeatureFlag {
            key: "cross-ctx-flag".into(),
            enabled: true,
            variants: vec![
                string_variant("control", "off"),
                string_variant("treatment", "on"),
            ],
            default_variant_key: "control".into(),
            rules: vec![proto_rule],
            ..Default::default()
        };
        let env_uuid = "00000000-0000-0000-0000-000000000001";
        let snap = DefinitionSnapshot::from_proto(SyncDefinitionsResponse {
            flags: vec![flag.clone()],
            rule_segments: vec![],
            list_segments: vec![],
            server_timestamp_ms: 0,
            environment_id: env_uuid.into(),
            event_definitions: vec![],
        });
        let client = sdk_client_with_snapshot(snap);

        // Build a multi-context bundle.
        let user =
            Context::new("user", "alice").with_parameter("tier", CoreParam::Str("gold".into()));
        let device =
            Context::new("device", "iphone-12").with_parameter("os", CoreParam::Str("ios".into()));
        let app = Context::new("application", "v2");
        let bundle = vec![user, device, app];

        let sdk_results = client
            .evaluate(
                &[EvalRequest {
                    flag_key: "cross-ctx-flag".into(),
                    contexts: bundle.clone(),
                }],
                TraceLevel::Full,
            )
            .await;

        // The same bundle, fed directly into core's evaluate_flag, must
        // produce the same variant for each context.
        let core_flag = convert_proto_flag_to_core(&flag).expect("convert");
        let env_id = parse_env_id(env_uuid);
        let core_results = evaluate_flag(
            &core_flag,
            &bundle,
            &[],
            &ListMembershipIndex::new(),
            env_id,
            ProjectId::new(),
            TraceLevel::Full,
        );

        assert_eq!(sdk_results.len(), core_results.len());
        for (sdk_r, core_r) in sdk_results.iter().zip(core_results.iter()) {
            assert_eq!(
                sdk_r.variant_key, core_r.variant_key,
                "SDK + core variant must match for cross-context hash"
            );
            assert!(
                sdk_r.outcome == EvalOutcome::Matched { rule_index: 0 },
                "rule must fire (And([]) is vacuously true)"
            );
        }

        // The two variants the bucket may resolve to. The exact value is
        // deterministic but we don't pin it here — the contract is that
        // SDK + core agree.
        assert!(
            sdk_results[0].variant_key == "control" || sdk_results[0].variant_key == "treatment"
        );
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
        tokio::time::timeout(
            Duration::from_secs(5),
            client.shutdown(Duration::from_secs(1)),
        )
        .await
        .expect("shutdown must not hang")
        .expect("shutdown should succeed");
    }

    #[tokio::test]
    async fn shutdown_is_safe_when_already_stopped() {
        let snap = DefinitionSnapshot::default();
        let client = sdk_client_with_snapshot(snap);
        // Double-shutdown should not panic.
        let client2 = Arc::clone(&client);
        client
            .shutdown(Duration::from_millis(100))
            .await
            .expect("first shutdown ok");
        client2
            .shutdown(Duration::from_millis(100))
            .await
            .expect("second shutdown ok");
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
            event_definitions: vec![],
        });
        let client = sdk_client_with_snapshot(snap);
        let ctx = Context::new("user", "alice");

        assert_eq!(client.event_queue.len(), 0);
        client
            .evaluate(
                &[
                    EvalRequest::single("test-flag", ctx.clone()),
                    EvalRequest::single("test-flag", ctx),
                ],
                TraceLevel::Off,
            )
            .await;
        // Each single-context EvalRequest yields one EvalResult → one event.
        assert_eq!(
            client.event_queue.len(),
            2,
            "one event per (request, context) pair"
        );
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
            event_definitions: vec![],
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
    async fn test_client_track_with_polled_event_def_succeeds() {
        // End-to-end: simulate a `SyncDefinitions` poll response that carries
        // event_definitions, build a DefinitionSnapshot via `from_proto`
        // (NOT via `with_event_definitions`), and verify that
        // `Client::track()` enqueues without warn-skipping.
        use stitchd_proto::sdk::v1::EventDefinitionMeta;
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

        // Build a snapshot ONLY through the proto path — this is what the
        // polling layer does in production.
        let proto_resp = SyncDefinitionsResponse {
            flags: vec![],
            rule_segments: vec![],
            list_segments: vec![],
            server_timestamp_ms: 0,
            environment_id: "env-1".into(),
            event_definitions: vec![EventDefinitionMeta {
                event_key: "checkout_completed".into(),
                value_type: "bool".into(),
            }],
        };
        let snap = DefinitionSnapshot::from_proto(proto_resp);
        // Sanity: the polled snapshot should now have the registered event.
        assert!(snap.event_definition("checkout_completed").is_some());

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
            .expect("track must succeed for polled event definition");

        // Force flush so the wiremock expectation can be checked.
        let buffer = client.event_buffer.as_ref().unwrap();
        let report = buffer.flush().await.expect("flush must succeed");
        assert_eq!(report.accepted, 1);
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
                *self.slot.lock().unwrap() = Some(serde_json::from_slice(&req.body).unwrap());
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

    // ── Phase 5 Task 5.3: Client::flush() + Client::shutdown(timeout) ───────

    #[tokio::test]
    async fn test_client_flush_delegates_to_buffer() {
        // Client::flush() must wire through to EventBuffer::flush() and
        // surface the resulting FlushReport unchanged. We can confirm
        // by enqueueing via track() then asserting accepted=1.
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

        let snap = snapshot_with_event_defs(vec![("checkout", EventValueType::Bool)]);
        let client = sdk_client_with_track_buffer(snap, &server.uri());

        let ctx = Context::new("user", "alice");
        client
            .track("checkout", &ctx, Some(TypedValue::Bool(true)), None)
            .await
            .expect("track ok");

        let report = client.flush().await.expect("flush should succeed");
        assert_eq!(report.accepted, 1);
        assert_eq!(report.rejected, 0);
    }

    #[tokio::test]
    async fn test_client_flush_no_buffer_returns_empty_report() {
        // `sdk_client_with_snapshot` constructs a client with
        // event_buffer = None. flush() must short-circuit to an empty
        // FlushReport — never panic, never error.
        let snap = snapshot_with_event_defs(vec![]);
        let client = sdk_client_with_snapshot(snap);
        assert!(client.event_buffer.is_none());

        let report = client.flush().await.expect("flush ok on bufferless client");
        assert_eq!(report, FlushReport::default());
    }

    #[tokio::test]
    async fn test_client_shutdown_drains_pending() {
        // Enqueue several events through track() and confirm
        // `client.shutdown(timeout)` triggers one final flush that
        // empties the buffer.
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/events/track"))
            .respond_with(ResponseTemplate::new(202).set_body_json(serde_json::json!({
                "accepted_count": 5,
                "rejected": []
            })))
            .expect(1)
            .mount(&server)
            .await;

        let snap = snapshot_with_event_defs(vec![("e", EventValueType::Int)]);
        let client = sdk_client_with_track_buffer(snap, &server.uri());
        let ctx = Context::new("user", "u");
        for _ in 0..5 {
            client
                .track("e", &ctx, Some(TypedValue::Int(1)), None)
                .await
                .expect("track ok");
        }
        // Sanity: buffer non-empty before shutdown.
        assert!(client.event_buffer.is_some());

        let report = client
            .shutdown(Duration::from_secs(5))
            .await
            .expect("shutdown should succeed");
        assert_eq!(report.accepted, 5);
        assert_eq!(report.rejected, 0);
    }

    #[tokio::test]
    async fn test_client_shutdown_without_buffer_returns_ok() {
        // shutdown() on a bufferless client must still stop the
        // background tasks and return an empty report.
        let snap = snapshot_with_event_defs(vec![]);
        let client = sdk_client_with_snapshot(snap);
        let report = client
            .shutdown(Duration::from_millis(100))
            .await
            .expect("shutdown ok on bufferless client");
        assert_eq!(report, FlushReport::default());
    }

    // ── Prerequisite-gate helpers (Phase 7, flag_lifecycle) ──────────────────

    use stitchd_proto::flags::v1::FlagPrerequisite as ProtoPrereq;

    #[test]
    fn deterministic_uuid_is_stable_and_distinct() {
        // Same inputs → same UUID across calls.
        let a1 = deterministic_uuid("variant", "flagA", "on");
        let a2 = deterministic_uuid("variant", "flagA", "on");
        assert_eq!(a1, a2);
        // Different parts → different UUIDs.
        assert_ne!(a1, deterministic_uuid("variant", "flagA", "off"));
        assert_ne!(a1, deterministic_uuid("variant", "flagB", "on"));
        // Length-prefixing prevents the ("ab","c") vs ("a","bc") collision.
        assert_ne!(
            deterministic_uuid("variant", "ab", "c"),
            deterministic_uuid("variant", "a", "bc")
        );
        // Well-formed RFC-4122 v4 shape.
        assert_eq!(a1.get_version_num(), 4);
    }

    #[test]
    fn deterministic_variant_id_matches_across_conversions() {
        // The id a dependent flag derives for a prereq's required variant must
        // equal the id the prereq flag's own conversion mints for that variant.
        let from_gate = deterministic_variant_id("prereq", "on");
        let proto = FeatureFlag {
            key: "prereq".into(),
            enabled: true,
            variants: vec![string_variant("on", "ON")],
            ..Default::default()
        };
        let core = convert_proto_flag_to_core(&proto).unwrap();
        let from_flag = core.variants.iter().find(|v| v.key == "on").unwrap().id;
        assert_eq!(from_gate, from_flag);
    }

    #[test]
    fn resolve_flag_id_prefers_wire_uuid_else_key() {
        let uuid = "00000000-0000-0000-0000-0000000000ff";
        assert_eq!(
            resolve_flag_id(uuid, "ignored"),
            FlagId::from_uuid(Uuid::parse_str(uuid).unwrap())
        );
        // Empty / malformed wire id → deterministic-from-key (stable).
        assert_eq!(resolve_flag_id("", "k"), resolve_flag_id("not-a-uuid", "k"));
    }

    #[test]
    fn build_prerequisite_gate_resolves_keys_and_wire_ids() {
        let req_uuid = "00000000-0000-0000-0000-0000000000c9";
        let pre_uuid = "00000000-0000-0000-0000-0000000000ca";
        let proto = FeatureFlag {
            key: "dep".into(),
            enabled: true,
            variants: vec![string_variant("fb", "FB")],
            fallback_variant_key: "fb".into(),
            prerequisites: vec![
                // key-only (SDK-sync shape)
                ProtoPrereq {
                    prerequisite_flag_id: String::new(),
                    prerequisite_flag_key: "p1".into(),
                    required_variant_id: String::new(),
                    required_variant_key: "on".into(),
                },
                // wire-id-populated (admin/preview shape)
                ProtoPrereq {
                    prerequisite_flag_id: pre_uuid.into(),
                    prerequisite_flag_key: "p2".into(),
                    required_variant_id: req_uuid.into(),
                    required_variant_key: "on".into(),
                },
            ],
            ..Default::default()
        };
        let own_map: HashMap<String, VariantId> =
            [("fb".to_string(), deterministic_variant_id("dep", "fb"))]
                .into_iter()
                .collect();
        let gate = build_prerequisite_gate(&proto, &own_map);

        assert_eq!(gate.prerequisites.len(), 2);
        // key-only → deterministic-from-key resolution
        assert_eq!(
            gate.prerequisites[0].prerequisite_flag_id,
            resolve_flag_id("", "p1")
        );
        assert_eq!(
            gate.prerequisites[0].required_variant_id,
            deterministic_variant_id("p1", "on")
        );
        // wire-id populated → parsed UUIDs win
        assert_eq!(
            gate.prerequisites[1].prerequisite_flag_id,
            FlagId::from_uuid(Uuid::parse_str(pre_uuid).unwrap())
        );
        assert_eq!(
            gate.prerequisites[1].required_variant_id,
            VariantId::from_uuid(Uuid::parse_str(req_uuid).unwrap())
        );
        // fallback resolves against THIS flag's own variant map
        assert_eq!(
            gate.fallback_variant_id,
            Some(deterministic_variant_id("dep", "fb"))
        );
    }

    #[test]
    fn build_prerequisite_gate_empty_fallback_is_none() {
        let proto = FeatureFlag {
            key: "dep".into(),
            enabled: true,
            variants: vec![string_variant("fb", "FB")],
            fallback_variant_key: String::new(),
            ..Default::default()
        };
        let gate = build_prerequisite_gate(&proto, &HashMap::new());
        assert!(gate.fallback_variant_id.is_none());
        assert!(gate.prerequisites.is_empty());
    }

    #[test]
    fn fold_prerequisite_fallbacks_propagates_in_dependency_order() {
        // a -> b -> c. c unmet (resolves to a non-required variant) so b folds
        // to its fallback, which in turn makes a fold to its fallback.
        let a = resolve_flag_id("", "a");
        let b = resolve_flag_id("", "b");
        let c = resolve_flag_id("", "c");
        let b_on = deterministic_variant_id("b", "on");
        let c_on = deterministic_variant_id("c", "on");
        let b_fb = deterministic_variant_id("b", "fb");
        let a_fb = deterministic_variant_id("a", "fb");

        let entries: Vec<(FlagId, Vec<Rule>, Vec<CorePrerequisite>)> = vec![
            (
                a,
                vec![],
                vec![CorePrerequisite {
                    prerequisite_flag_id: b,
                    required_variant_id: b_on,
                }],
            ),
            (
                b,
                vec![],
                vec![CorePrerequisite {
                    prerequisite_flag_id: c,
                    required_variant_id: c_on,
                }],
            ),
            (c, vec![], vec![]),
        ];
        let mut gates: HashMap<FlagId, PrerequisiteGate> = HashMap::new();
        gates.insert(
            a,
            PrerequisiteGate {
                prerequisites: entries[0].2.clone(),
                fallback_variant_id: Some(a_fb),
            },
        );
        gates.insert(
            b,
            PrerequisiteGate {
                prerequisites: entries[1].2.clone(),
                fallback_variant_id: Some(b_fb),
            },
        );

        // Rules-only map: c resolved to its `off` variant (≠ c_on); b resolved
        // its rule output `on`; a resolved its rule output `on`.
        let mut map: HashMap<FlagId, Option<VariantId>> = HashMap::new();
        map.insert(c, Some(deterministic_variant_id("c", "off")));
        map.insert(b, Some(b_on));
        map.insert(a, Some(deterministic_variant_id("a", "on")));

        fold_prerequisite_fallbacks(&mut map, &entries, &gates);

        // b's gate (needs c=on) unmet → b folds to b_fb; a's gate (needs b=on)
        // now sees b_fb ≠ b_on → a folds to a_fb.
        assert_eq!(map.get(&b).copied().flatten(), Some(b_fb));
        assert_eq!(map.get(&a).copied().flatten(), Some(a_fb));
        assert_eq!(
            map.get(&c).copied().flatten(),
            Some(deterministic_variant_id("c", "off"))
        );
    }

    #[test]
    fn resolve_segments_for_bundle_matches_rule_and_list() {
        use stitchd_core::rule_engine::condition::Condition;

        let rb_id = SegmentId::new();
        let list_id = SegmentId::new();
        // A rule-based segment that always matches (empty AND).
        let rb = SegmentDefinition::RuleBased(RuleBasedSegment {
            id: rb_id,
            rules: vec![Rule {
                id: RuleId::new(),
                name: None,
                condition: ConditionExpr::And(vec![]),
                output: RuleOutput::Variant(VariantId::new()),
            }],
        });
        // A rule-based segment that never matches.
        let never = SegmentDefinition::RuleBased(RuleBasedSegment {
            id: SegmentId::new(),
            rules: vec![Rule {
                id: RuleId::new(),
                name: None,
                condition: ConditionExpr::Leaf(Condition::Eq {
                    context_type: "user".into(),
                    param: "x".into(),
                    value: CoreParam::Str("nope".into()),
                }),
                output: RuleOutput::Variant(VariantId::new()),
            }],
        });

        let ctx = Context::new("user", "u-1");
        let mut idx = ListMembershipIndex::new();
        idx.insert(
            "user".to_string(),
            "u-1".to_string(),
            [list_id].into_iter().collect(),
        );

        let resolved = resolve_segments_for_bundle(std::slice::from_ref(&ctx), &[rb, never], &idx);
        assert!(resolved.contains(&rb_id));
        assert!(resolved.contains(&list_id));
        assert_eq!(resolved.len(), 2);
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
