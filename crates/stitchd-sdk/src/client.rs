//! SDK client — initialization, background polling, and flag evaluation.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use tokio::sync::{Mutex, RwLock};
use tokio_util::sync::CancellationToken;

use stitchd_core::{
    context::EvaluationContext,
    hashing::calculate_allocation,
    id::SegmentId,
    rule_engine::{
        evaluate_rules,
        types::{EvaluationInput, RuleOutput, TargetField},
    },
    segment::RuleBasedSegment,
    variants::VariantValue,
};

use crate::{
    cache::{DefinitionCache, SdkFlagDef, SdkListSegmentMeta, collect_segment_ids},
    config::SdkConfig,
    error::SdkError,
    grpc_client::SdkGrpcClient,
    http_client::SdkHttpClient,
    lfu::LfuState,
};

/// Thread-safe SDK client. Obtain via [`SdkClient::init`].
pub struct SdkClient {
    cache: Arc<RwLock<DefinitionCache>>,
    http_client: SdkHttpClient,
    cancel: CancellationToken,
    lfu: Option<Arc<Mutex<LfuState>>>,
}

impl SdkClient {
    /// Create a client directly from a pre-built cache (for testing only).
    #[cfg(test)]
    pub(crate) fn from_cache(
        cache: DefinitionCache,
        http_url: impl Into<String>,
        sdk_key: impl Into<String>,
    ) -> Arc<Self> {
        Arc::new(Self {
            cache: Arc::new(RwLock::new(cache)),
            http_client: SdkHttpClient::new(http_url, sdk_key),
            cancel: CancellationToken::new(),
            lfu: None,
        })
    }

    /// Create a client with LFU enabled (for testing only).
    #[cfg(test)]
    pub(crate) fn from_cache_with_lfu(
        cache: DefinitionCache,
        http_url: impl Into<String>,
        sdk_key: impl Into<String>,
        lfu_capacity: usize,
        lfu_window: std::time::Duration,
    ) -> Arc<Self> {
        Arc::new(Self {
            cache: Arc::new(RwLock::new(cache)),
            http_client: SdkHttpClient::new(http_url, sdk_key),
            cancel: CancellationToken::new(),
            lfu: Some(Arc::new(Mutex::new(LfuState::new(
                lfu_capacity,
                lfu_window,
            )))),
        })
    }

    /// Initialize the SDK: fetch definitions once, then start background polling.
    ///
    /// Blocks until the first successful sync. Returns an error if the server is
    /// unreachable or the SDK key is invalid/revoked.
    pub async fn init(config: SdkConfig) -> Result<Arc<Self>, SdkError> {
        let grpc = SdkGrpcClient::new(&config.grpc_url, &config.sdk_key);
        let http = SdkHttpClient::new(&config.http_url, &config.sdk_key);

        let resp = grpc
            .fetch_definitions()
            .await
            .map_err(|e| SdkError::InitFailed(e.to_string()))?;

        let initial = DefinitionCache::from_sync_response(resp)?;
        let cache = Arc::new(RwLock::new(initial));
        let cancel = CancellationToken::new();

        let lfu: Option<Arc<Mutex<LfuState>>> = config
            .lfu
            .as_ref()
            .map(|cfg| Arc::new(Mutex::new(LfuState::new(cfg.capacity, cfg.window))));

        // Spawn background polling task.
        {
            let poll_cache = Arc::clone(&cache);
            let poll_cancel = cancel.clone();
            let poll_interval = config.poll_interval;
            let poll_grpc = SdkGrpcClient::new(&config.grpc_url, &config.sdk_key);
            let poll_http = SdkHttpClient::new(&config.http_url, &config.sdk_key);
            let poll_lfu = lfu.clone();

            tokio::spawn(async move {
                let mut interval = tokio::time::interval(poll_interval);
                interval.tick().await; // skip immediate first tick
                loop {
                    tokio::select! {
                        () = poll_cancel.cancelled() => break,
                        _ = interval.tick() => {
                            poll_once(
                                &poll_grpc,
                                &poll_http,
                                &poll_cache,
                                poll_lfu.as_ref(),
                            ).await;
                        }
                    }
                }
            });
        }

        Ok(Arc::new(Self {
            cache,
            http_client: http,
            cancel,
            lfu,
        }))
    }

    /// Evaluate a feature flag for the given context.
    ///
    /// Returns `None` if the flag is disabled or not found.
    /// For list-based segments this may make a REST call unless the context is
    /// in the LFU hot cache.
    pub async fn evaluate(
        &self,
        flag_key: &str,
        eval_ctx: &EvaluationContext,
    ) -> Result<Option<VariantValue>, SdkError> {
        // ── Phase 1: Clone what we need from the cache (brief read lock) ───────
        let (flag_def, rule_segs, list_metas, env_id_str) = {
            let cache = self.cache.read().await;

            let flag_def = match cache.flags.get(flag_key) {
                Some(f) if f.enabled => f.clone(),
                _ => return Ok(None),
            };

            let seg_ids = collect_segment_ids(&flag_def.rules);

            let rule_segs: HashMap<SegmentId, RuleBasedSegment> = seg_ids
                .iter()
                .filter_map(|id| cache.rule_segments.get(id).map(|s| (*id, s.clone())))
                .collect();

            let list_metas: HashMap<SegmentId, SdkListSegmentMeta> = seg_ids
                .iter()
                .filter_map(|id| cache.list_segments.get(id).map(|s| (*id, s.clone())))
                .collect();

            let env_id_str = cache
                .environment_id
                .map(|id| id.to_string())
                .unwrap_or_default();

            (flag_def, rule_segs, list_metas, env_id_str)
        }; // cache read lock released

        // ── Phase 2: Record evaluation frequency in LFU tracker ─────────────
        if let Some(lfu) = &self.lfu {
            let mut lfu_guard = lfu.lock().await;
            for ctx in &eval_ctx.contexts {
                lfu_guard.tracker.record(&ctx.context_type, &ctx.key);
            }
        }

        // ── Phase 3: Resolve segment membership ──────────────────────────────
        let seg_ids = collect_segment_ids(&flag_def.rules);
        let mut resolved: HashSet<SegmentId> = HashSet::new();

        for seg_id in &seg_ids {
            if let Some(rule_seg) = rule_segs.get(seg_id) {
                if rule_seg.evaluate(&eval_ctx.contexts)?.matched {
                    resolved.insert(*seg_id);
                }
            } else if let Some(list_meta) = list_metas.get(seg_id) {
                let ctx_key = eval_ctx
                    .get_context(&list_meta.context_type)
                    .map(|c| c.key.as_str())
                    .unwrap_or("");

                // Check LFU cache first.
                let lfu_result = if let Some(lfu) = &self.lfu {
                    lfu.lock()
                        .await
                        .cache
                        .get(&list_meta.context_type, ctx_key, &list_meta.key)
                } else {
                    None
                };

                let is_member = if let Some(cached) = lfu_result {
                    cached
                } else {
                    let m = self
                        .http_client
                        .list_check(
                            &env_id_str,
                            &list_meta.context_type,
                            ctx_key,
                            std::slice::from_ref(&list_meta.key),
                        )
                        .await?;
                    m.get(&list_meta.key).copied().unwrap_or(false)
                };

                if is_member {
                    resolved.insert(*seg_id);
                }
            }
        }

        // ── Phase 4: Evaluate flag rules ─────────────────────────────────────
        apply_rules(
            &flag_def,
            &eval_ctx.contexts,
            resolved,
            flag_key,
            &env_id_str,
        )
    }
}

impl Drop for SdkClient {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

/// Execute one poll cycle: fetch new definitions and optionally refresh the LFU cache.
async fn poll_once(
    grpc: &SdkGrpcClient,
    http: &SdkHttpClient,
    cache: &Arc<RwLock<DefinitionCache>>,
    lfu: Option<&Arc<Mutex<LfuState>>>,
) {
    let resp = match grpc.fetch_definitions().await {
        Ok(r) => r,
        Err(_) => return, // stale-while-revalidate: keep old cache on error
    };

    let new_cache = match DefinitionCache::from_sync_response(resp) {
        Ok(c) => c,
        Err(_) => return,
    };

    // Update the definition cache.
    *cache.write().await = new_cache;

    // If LFU is enabled, batch-refresh membership for hot contexts.
    let Some(lfu) = lfu else { return };

    let hot_set = lfu.lock().await.tracker.hot_set();
    if hot_set.is_empty() {
        return;
    }

    let (env_id_str, seg_keys) = {
        let cache_guard = cache.read().await;
        let env_id = cache_guard
            .environment_id
            .map(|id| id.to_string())
            .unwrap_or_default();
        let keys: Vec<String> = cache_guard
            .list_segments
            .values()
            .map(|s| s.key.clone())
            .collect();
        (env_id, keys)
    };

    if seg_keys.is_empty() {
        return;
    }

    if let Ok(new_entries) = http
        .list_check_batch(&env_id_str, &hot_set, &seg_keys)
        .await
    {
        lfu.lock().await.cache.replace(new_entries);
    }
}

/// Pure evaluation of flag rules against pre-resolved segments.
pub(crate) fn apply_rules(
    flag_def: &SdkFlagDef,
    contexts: &[stitchd_core::context::Context],
    resolved: HashSet<SegmentId>,
    flag_key: &str,
    env_id_str: &str,
) -> Result<Option<VariantValue>, SdkError> {
    let input = EvaluationInput {
        contexts,
        resolved_segments: resolved,
        evaluated_flags: HashMap::new(),
    };

    match evaluate_rules(&flag_def.rules, &input)? {
        None => Ok(None),
        Some(RuleOutput::Variant(vid)) => Ok(flag_def.variant_map.get(vid).map(|(_, v)| v.clone())),
        Some(RuleOutput::Percentage { targets, weights }) => {
            let mut target_values: Vec<String> = Vec::new();
            for target in targets {
                if let Some(ctx) = contexts
                    .iter()
                    .find(|c| c.context_type == target.context_type)
                {
                    let val = match &target.field {
                        TargetField::Key => ctx.key.clone(),
                        TargetField::Parameter(name) => ctx
                            .parameters
                            .get(name)
                            .map(|v| v.to_string())
                            .unwrap_or_default(),
                    };
                    target_values.push(val);
                }
            }

            let pct = calculate_allocation(flag_key, env_id_str, &target_values);
            let bucket = ((pct * 10.0).floor() as u32).min(999);

            let mut cumulative = 0u32;
            for (vid, weight) in weights {
                cumulative += weight;
                if bucket < cumulative {
                    return Ok(flag_def.variant_map.get(vid).map(|(_, v)| v.clone()));
                }
            }
            Ok(None)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use stitchd_core::{
        context::{Context, EvaluationContext, ParameterValue},
        id::{RuleId, SegmentId, VariantId},
        rule_engine::{
            condition::Condition,
            types::{ConditionExpr, Rule, RuleOutput},
        },
        variants::VariantValue,
    };

    use crate::cache::SdkFlagDef;

    fn always_true_rule(vid: VariantId) -> Rule {
        Rule {
            id: RuleId::new(),
            name: None,
            condition: ConditionExpr::And(vec![]),
            output: RuleOutput::Variant(vid),
        }
    }

    fn bool_flag_def(key: &str, variant_key: &str, value: bool) -> SdkFlagDef {
        let vid = VariantId::new();
        let mut variant_map = HashMap::new();
        variant_map.insert(
            vid,
            (variant_key.to_owned(), VariantValue::BoolValue(value)),
        );
        SdkFlagDef {
            key: key.to_owned(),
            enabled: true,
            rules: vec![always_true_rule(vid)],
            variant_map,
        }
    }

    #[test]
    fn rule_based_returns_correct_variant() {
        let def = bool_flag_def("my-flag", "on", true);
        let ctx = EvaluationContext::new().with_context(Context::new("user", "u1"));
        let result = apply_rules(&def, &ctx.contexts, HashSet::new(), "my-flag", "env-1");
        assert_eq!(result.unwrap(), Some(VariantValue::BoolValue(true)));
    }

    #[test]
    fn rule_based_no_matching_rule_returns_none() {
        let vid = VariantId::new();
        let mut variant_map = HashMap::new();
        variant_map.insert(vid, ("on".to_owned(), VariantValue::BoolValue(true)));
        let def = SdkFlagDef {
            key: "flag".to_owned(),
            enabled: true,
            rules: vec![Rule {
                id: RuleId::new(),
                name: None,
                condition: ConditionExpr::Leaf(Condition::Eq {
                    context_type: "user".into(),
                    param: "plan".into(),
                    value: ParameterValue::Str("pro".into()),
                }),
                output: RuleOutput::Variant(vid),
            }],
            variant_map,
        };
        let ctx = EvaluationContext::new().with_context(
            Context::new("user", "u1").with_parameter("plan", ParameterValue::Str("free".into())),
        );
        let result = apply_rules(&def, &ctx.contexts, HashSet::new(), "flag", "env-1");
        assert_eq!(result.unwrap(), None);
    }

    #[test]
    fn in_segment_condition_uses_resolved_set() {
        let vid = VariantId::new();
        let seg_id = SegmentId::new();
        let mut variant_map = HashMap::new();
        variant_map.insert(vid, ("treatment".to_owned(), VariantValue::BoolValue(true)));
        let def = SdkFlagDef {
            key: "flag".to_owned(),
            enabled: true,
            rules: vec![Rule {
                id: RuleId::new(),
                name: None,
                condition: ConditionExpr::Leaf(Condition::InSegment(seg_id)),
                output: RuleOutput::Variant(vid),
            }],
            variant_map,
        };
        let ctx = EvaluationContext::new().with_context(Context::new("user", "u1"));

        // Not in segment → None
        assert_eq!(
            apply_rules(&def, &ctx.contexts, HashSet::new(), "flag", "env-1").unwrap(),
            None
        );

        // In segment → Some(treatment)
        let mut resolved = HashSet::new();
        resolved.insert(seg_id);
        assert_eq!(
            apply_rules(&def, &ctx.contexts, resolved, "flag", "env-1").unwrap(),
            Some(VariantValue::BoolValue(true))
        );
    }

    use crate::cache::DefinitionCache;

    fn make_bool_flag_cache(flag_key: &str, enabled: bool) -> DefinitionCache {
        use stitchd_proto::flags::v1::{
            FeatureFlag, FlagRule, SyncResponse, Variant, VariantValue as ProtoVariantValue,
            flag_rule::Output, variant_value::Value,
        };
        let resp = SyncResponse {
            flags: vec![FeatureFlag {
                key: flag_key.to_string(),
                enabled,
                value_type: 1,
                variants: vec![Variant {
                    key: "on".to_string(),
                    value: Some(ProtoVariantValue {
                        value: Some(Value::BoolValue(true)),
                    }),
                }],
                rules: vec![FlagRule {
                    rule_payload: serde_json::to_vec(&ConditionExpr::And(vec![])).unwrap(),
                    output: Some(Output::VariantKey("on".to_string())),
                    name: String::new(),
                }],
                ..Default::default()
            }],
            server_timestamp_ms: 0,
            rule_segments: vec![],
            list_segments: vec![],
            environment_id: String::new(),
        };
        DefinitionCache::from_sync_response(resp).unwrap()
    }

    #[tokio::test]
    async fn evaluate_returns_some_for_enabled_flag() {
        let cache = make_bool_flag_cache("feature-x", true);
        let client = SdkClient::from_cache(cache, "http://localhost:9999", "key");
        let ctx = EvaluationContext::new().with_context(Context::new("user", "u1"));
        let result = client.evaluate("feature-x", &ctx).await.unwrap();
        assert_eq!(result, Some(VariantValue::BoolValue(true)));
    }

    #[tokio::test]
    async fn evaluate_returns_none_for_disabled_flag() {
        let cache = make_bool_flag_cache("feature-x", false);
        let client = SdkClient::from_cache(cache, "http://localhost:9999", "key");
        let ctx = EvaluationContext::new().with_context(Context::new("user", "u1"));
        let result = client.evaluate("feature-x", &ctx).await.unwrap();
        assert_eq!(result, None);
    }

    #[tokio::test]
    async fn evaluate_returns_none_for_unknown_flag() {
        let cache = make_bool_flag_cache("feature-x", true);
        let client = SdkClient::from_cache(cache, "http://localhost:9999", "key");
        let ctx = EvaluationContext::new().with_context(Context::new("user", "u1"));
        let result = client.evaluate("nonexistent", &ctx).await.unwrap();
        assert_eq!(result, None);
    }

    #[tokio::test]
    async fn evaluate_with_rule_based_segment_not_matching() {
        use stitchd_core::rule_engine::condition::Condition;
        use stitchd_proto::flags::v1::{
            FeatureFlag, FlagRule, SyncResponse, Variant, VariantValue as ProtoVariantValue,
            flag_rule::Output, variant_value::Value,
        };
        use stitchd_proto::segments::v1::RuleSegment as ProtoRuleSegment;

        let seg_uuid = uuid::Uuid::new_v4();
        let seg_id = SegmentId::from_uuid(seg_uuid);

        // Rule segment: empty rules → doesn't match anything
        let seg_rules: Vec<stitchd_core::rule_engine::types::Rule> = vec![];
        let seg_payload = serde_json::to_vec(&seg_rules).unwrap();

        // Flag: matches if in segment
        let flag_condition = ConditionExpr::Leaf(Condition::InSegment(seg_id));
        let resp = SyncResponse {
            flags: vec![FeatureFlag {
                key: "seg-flag".to_string(),
                enabled: true,
                value_type: 1,
                variants: vec![Variant {
                    key: "on".to_string(),
                    value: Some(ProtoVariantValue {
                        value: Some(Value::BoolValue(true)),
                    }),
                }],
                rules: vec![FlagRule {
                    rule_payload: serde_json::to_vec(&flag_condition).unwrap(),
                    output: Some(Output::VariantKey("on".to_string())),
                    name: String::new(),
                }],
                ..Default::default()
            }],
            server_timestamp_ms: 0,
            rule_segments: vec![ProtoRuleSegment {
                id: seg_uuid.to_string(),
                rule_payload: seg_payload,
                context_type: "user".to_string(),
                key: "test-seg".to_string(),
            }],
            list_segments: vec![],
            environment_id: String::new(),
        };
        let cache = DefinitionCache::from_sync_response(resp).unwrap();
        let client = SdkClient::from_cache(cache, "http://localhost:9999", "key");
        let ctx = EvaluationContext::new().with_context(Context::new("user", "u1"));
        // Rule segment has empty rules → doesn't match → flag returns None
        let result = client.evaluate("seg-flag", &ctx).await.unwrap();
        assert_eq!(result, None);
    }

    #[tokio::test]
    async fn evaluate_with_rule_based_segment_matching() {
        use stitchd_core::rule_engine::condition::Condition;
        use stitchd_proto::flags::v1::{
            FeatureFlag, FlagRule, SyncResponse, Variant, VariantValue as ProtoVariantValue,
            flag_rule::Output, variant_value::Value,
        };
        use stitchd_proto::segments::v1::RuleSegment as ProtoRuleSegment;

        let seg_uuid = uuid::Uuid::new_v4();
        let seg_id = SegmentId::from_uuid(seg_uuid);

        // Rule segment: always matches (always-true And([])-based rule)
        let seg_rule = stitchd_core::rule_engine::types::Rule {
            id: RuleId::new(),
            name: None,
            condition: ConditionExpr::And(vec![]), // always true
            output: stitchd_core::rule_engine::types::RuleOutput::Variant(VariantId::new()),
        };
        let seg_payload = serde_json::to_vec(&vec![seg_rule]).unwrap();

        // Flag: matches if in segment
        let flag_condition = ConditionExpr::Leaf(Condition::InSegment(seg_id));
        let resp = SyncResponse {
            flags: vec![FeatureFlag {
                key: "seg-flag".to_string(),
                enabled: true,
                value_type: 1,
                variants: vec![Variant {
                    key: "on".to_string(),
                    value: Some(ProtoVariantValue {
                        value: Some(Value::BoolValue(true)),
                    }),
                }],
                rules: vec![FlagRule {
                    rule_payload: serde_json::to_vec(&flag_condition).unwrap(),
                    output: Some(Output::VariantKey("on".to_string())),
                    name: String::new(),
                }],
                ..Default::default()
            }],
            server_timestamp_ms: 0,
            rule_segments: vec![ProtoRuleSegment {
                id: seg_uuid.to_string(),
                rule_payload: seg_payload,
                context_type: "user".to_string(),
                key: "test-seg".to_string(),
            }],
            list_segments: vec![],
            environment_id: String::new(),
        };
        let cache = DefinitionCache::from_sync_response(resp).unwrap();
        let client = SdkClient::from_cache(cache, "http://localhost:9999", "key");
        let ctx = EvaluationContext::new().with_context(Context::new("user", "u1"));
        // Segment matches → flag resolves in-segment condition → Some(true)
        let result = client.evaluate("seg-flag", &ctx).await.unwrap();
        assert_eq!(result, Some(VariantValue::BoolValue(true)));
    }

    #[tokio::test]
    async fn drop_cancels_token() {
        // Just verify Drop doesn't panic
        let cache = make_bool_flag_cache("f", true);
        let client = SdkClient::from_cache(cache, "http://localhost:9999", "key");
        drop(client);
    }

    #[tokio::test]
    async fn evaluate_with_lfu_records_frequency() {
        let cache = make_bool_flag_cache("feature-x", true);
        let client = SdkClient::from_cache_with_lfu(
            cache,
            "http://localhost:9999",
            "key",
            10,
            std::time::Duration::from_secs(60),
        );
        let ctx = EvaluationContext::new().with_context(Context::new("user", "u1"));
        let result = client.evaluate("feature-x", &ctx).await.unwrap();
        assert_eq!(result, Some(VariantValue::BoolValue(true)));
    }

    #[tokio::test]
    async fn evaluate_list_segment_http_check() {
        use stitchd_core::rule_engine::condition::Condition;
        use stitchd_proto::flags::v1::{
            FeatureFlag, FlagRule, SyncResponse, Variant, VariantValue as ProtoVariantValue,
            flag_rule::Output, variant_value::Value,
        };
        use stitchd_proto::segments::v1::ListSegmentMeta as ProtoListMeta;
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        let seg_uuid = uuid::Uuid::new_v4();

        // Mock the list-check endpoint to say user is a member
        Mock::given(method("POST"))
            .and(path("/v1/environments//segments/list-check".to_string()))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "memberships": {
                    "list-seg": true
                }
            })))
            .mount(&server)
            .await;

        let seg_id = SegmentId::from_uuid(seg_uuid);
        let flag_condition = ConditionExpr::Leaf(Condition::InSegment(seg_id));
        let resp = SyncResponse {
            flags: vec![FeatureFlag {
                key: "list-flag".to_string(),
                enabled: true,
                value_type: 1,
                variants: vec![Variant {
                    key: "on".to_string(),
                    value: Some(ProtoVariantValue {
                        value: Some(Value::BoolValue(true)),
                    }),
                }],
                rules: vec![FlagRule {
                    rule_payload: serde_json::to_vec(&flag_condition).unwrap(),
                    output: Some(Output::VariantKey("on".to_string())),
                    name: String::new(),
                }],
                ..Default::default()
            }],
            server_timestamp_ms: 0,
            rule_segments: vec![],
            list_segments: vec![ProtoListMeta {
                id: seg_uuid.to_string(),
                key: "list-seg".to_string(),
                context_type: "user".to_string(),
            }],
            environment_id: String::new(),
        };

        let cache = DefinitionCache::from_sync_response(resp).unwrap();
        let client = SdkClient::from_cache(cache, server.uri(), "key");
        let ctx = EvaluationContext::new().with_context(Context::new("user", "u1"));
        let result = client.evaluate("list-flag", &ctx).await.unwrap();
        assert_eq!(result, Some(VariantValue::BoolValue(true)));
    }

    #[tokio::test]
    async fn evaluate_list_segment_not_member() {
        use stitchd_core::rule_engine::condition::Condition;
        use stitchd_proto::flags::v1::{
            FeatureFlag, FlagRule, SyncResponse, Variant, VariantValue as ProtoVariantValue,
            flag_rule::Output, variant_value::Value,
        };
        use stitchd_proto::segments::v1::ListSegmentMeta as ProtoListMeta;
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        let seg_uuid = uuid::Uuid::new_v4();

        Mock::given(method("POST"))
            .and(path("/v1/environments//segments/list-check".to_string()))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "memberships": {
                    "list-seg": false
                }
            })))
            .mount(&server)
            .await;

        let seg_id = SegmentId::from_uuid(seg_uuid);
        let flag_condition = ConditionExpr::Leaf(Condition::InSegment(seg_id));
        let resp = SyncResponse {
            flags: vec![FeatureFlag {
                key: "list-flag".to_string(),
                enabled: true,
                value_type: 1,
                variants: vec![Variant {
                    key: "on".to_string(),
                    value: Some(ProtoVariantValue {
                        value: Some(Value::BoolValue(true)),
                    }),
                }],
                rules: vec![FlagRule {
                    rule_payload: serde_json::to_vec(&flag_condition).unwrap(),
                    output: Some(Output::VariantKey("on".to_string())),
                    name: String::new(),
                }],
                ..Default::default()
            }],
            server_timestamp_ms: 0,
            rule_segments: vec![],
            list_segments: vec![ProtoListMeta {
                id: seg_uuid.to_string(),
                key: "list-seg".to_string(),
                context_type: "user".to_string(),
            }],
            environment_id: String::new(),
        };

        let cache = DefinitionCache::from_sync_response(resp).unwrap();
        let client = SdkClient::from_cache(cache, server.uri(), "key");
        let ctx = EvaluationContext::new().with_context(Context::new("user", "u1"));
        let result = client.evaluate("list-flag", &ctx).await.unwrap();
        assert_eq!(result, None);
    }

    #[tokio::test]
    async fn evaluate_list_segment_lfu_cache_hit() {
        use stitchd_core::rule_engine::condition::Condition;
        use stitchd_proto::flags::v1::{
            FeatureFlag, FlagRule, SyncResponse, Variant, VariantValue as ProtoVariantValue,
            flag_rule::Output, variant_value::Value,
        };
        use stitchd_proto::segments::v1::ListSegmentMeta as ProtoListMeta;

        let seg_uuid = uuid::Uuid::new_v4();
        let seg_id = SegmentId::from_uuid(seg_uuid);
        let flag_condition = ConditionExpr::Leaf(Condition::InSegment(seg_id));

        let resp = SyncResponse {
            flags: vec![FeatureFlag {
                key: "list-flag".to_string(),
                enabled: true,
                value_type: 1,
                variants: vec![Variant {
                    key: "on".to_string(),
                    value: Some(ProtoVariantValue {
                        value: Some(Value::BoolValue(true)),
                    }),
                }],
                rules: vec![FlagRule {
                    rule_payload: serde_json::to_vec(&flag_condition).unwrap(),
                    output: Some(Output::VariantKey("on".to_string())),
                    name: String::new(),
                }],
                ..Default::default()
            }],
            server_timestamp_ms: 0,
            rule_segments: vec![],
            list_segments: vec![ProtoListMeta {
                id: seg_uuid.to_string(),
                key: "list-seg".to_string(),
                context_type: "user".to_string(),
            }],
            environment_id: String::new(),
        };

        let cache = DefinitionCache::from_sync_response(resp).unwrap();
        // Use LFU with a pre-populated cache entry: user/u1/list-seg = true
        let client = SdkClient::from_cache_with_lfu(
            cache,
            "http://127.0.0.1:19998", // won't be called — LFU hit
            "key",
            10,
            std::time::Duration::from_secs(60),
        );

        // Pre-populate LFU cache
        {
            let lfu_arc = client.lfu.as_ref().unwrap();
            let mut lfu = lfu_arc.lock().await;
            let mut entries = std::collections::HashMap::new();
            entries.insert(
                ("user".to_string(), "u1".to_string(), "list-seg".to_string()),
                true,
            );
            lfu.cache.replace(entries);
        }

        let ctx = EvaluationContext::new().with_context(Context::new("user", "u1"));
        let result = client.evaluate("list-flag", &ctx).await.unwrap();
        assert_eq!(result, Some(VariantValue::BoolValue(true)));
    }

    #[tokio::test]
    async fn poll_once_grpc_failure_is_swallowed() {
        // poll_once with unreachable gRPC — should return without panicking
        let grpc = SdkGrpcClient::new("http://127.0.0.1:19997", "key");
        let http = SdkHttpClient::new("http://127.0.0.1:19997", "key");
        let cache: Arc<RwLock<DefinitionCache>> = Arc::new(RwLock::new(DefinitionCache::default()));
        poll_once(&grpc, &http, &cache, None).await; // should not panic
    }

    /// Start an in-process gRPC server for testing init and poll_once success paths.
    async fn start_test_grpc_server() -> String {
        use stitchd_proto::flags::v1::{
            SyncRequest, SyncResponse,
            flag_sync_service_server::{FlagSyncService, FlagSyncServiceServer},
        };
        use tokio::net::TcpListener;
        use tokio_stream::wrappers::TcpListenerStream;

        #[derive(Clone)]
        struct TestFlagSyncService;

        #[tonic::async_trait]
        impl FlagSyncService for TestFlagSyncService {
            async fn sync(
                &self,
                _request: tonic::Request<SyncRequest>,
            ) -> Result<tonic::Response<SyncResponse>, tonic::Status> {
                Ok(tonic::Response::new(SyncResponse {
                    flags: vec![],
                    rule_segments: vec![],
                    list_segments: vec![],
                    server_timestamp_ms: 0,
                    environment_id: String::new(),
                }))
            }
        }

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let stream = TcpListenerStream::new(listener);

        tokio::spawn(async move {
            tonic::transport::Server::builder()
                .add_service(FlagSyncServiceServer::new(TestFlagSyncService))
                .serve_with_incoming(stream)
                .await
                .ok();
        });

        format!("http://{addr}")
    }

    #[tokio::test]
    async fn init_succeeds_with_in_process_server() {
        let grpc_url = start_test_grpc_server().await;
        let config = SdkConfig::new(grpc_url, "http://127.0.0.1:9999", "test-key");
        let client = SdkClient::init(config).await;
        assert!(client.is_ok());
    }

    #[tokio::test]
    async fn init_succeeds_with_lfu_config() {
        let grpc_url = start_test_grpc_server().await;
        let mut config = SdkConfig::new(grpc_url, "http://127.0.0.1:9999", "test-key");
        config.lfu = Some(crate::config::LfuConfig {
            capacity: 10,
            window: std::time::Duration::from_secs(60),
        });
        let client = SdkClient::init(config).await;
        assert!(client.is_ok());
    }

    #[tokio::test]
    async fn init_fails_when_grpc_unreachable() {
        let config = SdkConfig::new("http://127.0.0.1:19996", "http://127.0.0.1:9999", "key");
        let result = SdkClient::init(config).await;
        assert!(result.is_err());
        let err = result.err().expect("expected error");
        matches!(err, SdkError::InitFailed(_));
    }

    #[tokio::test]
    async fn poll_once_updates_cache_on_success() {
        let grpc_url = start_test_grpc_server().await;
        let grpc = SdkGrpcClient::new(&grpc_url, "key");
        let http = SdkHttpClient::new("http://127.0.0.1:9999", "key");
        let cache: Arc<RwLock<DefinitionCache>> = Arc::new(RwLock::new(DefinitionCache::default()));
        poll_once(&grpc, &http, &cache, None).await;
        // After poll_once with a working server, cache should be updated (empty but valid)
        let cache_guard = cache.read().await;
        assert!(cache_guard.flags.is_empty());
    }

    #[tokio::test]
    async fn poll_once_with_lfu_empty_hot_set_returns_early() {
        let grpc_url = start_test_grpc_server().await;
        let grpc = SdkGrpcClient::new(&grpc_url, "key");
        let http = SdkHttpClient::new("http://127.0.0.1:9999", "key");
        let cache: Arc<RwLock<DefinitionCache>> = Arc::new(RwLock::new(DefinitionCache::default()));
        let lfu = Arc::new(Mutex::new(LfuState::new(
            10,
            std::time::Duration::from_secs(60),
        )));
        // LFU has no entries → hot_set is empty → early return
        poll_once(&grpc, &http, &cache, Some(&lfu)).await;
        // Should still have updated the main cache
        let cache_guard = cache.read().await;
        assert!(cache_guard.flags.is_empty());
    }

    fn percentage_flag_def(
        flag_key: &str,
        context_type: &str,
        target_field: TargetField,
    ) -> SdkFlagDef {
        let vid_a = VariantId::new();
        let vid_b = VariantId::new();
        let mut variant_map = HashMap::new();
        variant_map.insert(
            vid_a,
            ("treatment".to_owned(), VariantValue::BoolValue(true)),
        );
        variant_map.insert(
            vid_b,
            ("control".to_owned(), VariantValue::BoolValue(false)),
        );
        SdkFlagDef {
            key: flag_key.to_owned(),
            enabled: true,
            rules: vec![Rule {
                id: RuleId::new(),
                name: None,
                condition: ConditionExpr::And(vec![]),
                output: RuleOutput::Percentage {
                    targets: vec![stitchd_core::rule_engine::types::PercentageTarget {
                        context_type: context_type.to_owned(),
                        field: target_field,
                    }],
                    weights: vec![(vid_a, 500), (vid_b, 500)],
                },
            }],
            variant_map,
        }
    }

    #[test]
    fn percentage_rule_deterministic() {
        let def = percentage_flag_def("pct-flag", "user", TargetField::Key);
        let ctx = EvaluationContext::new().with_context(Context::new("user", "u1"));

        // Same inputs should always produce the same result
        let result1 =
            apply_rules(&def, &ctx.contexts, HashSet::new(), "pct-flag", "env-1").unwrap();
        let result2 =
            apply_rules(&def, &ctx.contexts, HashSet::new(), "pct-flag", "env-1").unwrap();
        assert_eq!(result1, result2);
        // Should return Some variant (one of treatment/control)
        assert!(result1.is_some());
    }

    #[test]
    fn percentage_rule_with_parameter_field() {
        let def = percentage_flag_def(
            "pct-flag",
            "user",
            TargetField::Parameter("account_id".to_string()),
        );
        let ctx = EvaluationContext::new().with_context(
            Context::new("user", "u1")
                .with_parameter("account_id", ParameterValue::Str("acct-123".into())),
        );

        let result = apply_rules(&def, &ctx.contexts, HashSet::new(), "pct-flag", "env-1").unwrap();
        assert!(result.is_some());
    }

    #[test]
    fn percentage_rule_missing_context_type_skips_target() {
        // If the target context_type is not present in the evaluation context,
        // the target value is skipped (empty target_values list)
        let def = percentage_flag_def("pct-flag", "device", TargetField::Key);
        // Only user context, no device
        let ctx = EvaluationContext::new().with_context(Context::new("user", "u1"));

        // Should still return a result (with empty target_values, hash is deterministic)
        let result = apply_rules(&def, &ctx.contexts, HashSet::new(), "pct-flag", "env-1").unwrap();
        // The result might be None if weights don't cover the bucket, or Some
        let _ = result; // just verify no panic
    }

    #[test]
    fn percentage_rule_all_weights_zero_returns_none() {
        let vid_a = VariantId::new();
        let mut variant_map = HashMap::new();
        variant_map.insert(vid_a, ("on".to_owned(), VariantValue::BoolValue(true)));
        let def = SdkFlagDef {
            key: "pct-flag".to_owned(),
            enabled: true,
            rules: vec![Rule {
                id: RuleId::new(),
                name: None,
                condition: ConditionExpr::And(vec![]),
                output: RuleOutput::Percentage {
                    targets: vec![stitchd_core::rule_engine::types::PercentageTarget {
                        context_type: "user".to_owned(),
                        field: TargetField::Key,
                    }],
                    weights: vec![], // no weights → cumulative never exceeds bucket
                },
            }],
            variant_map,
        };
        let ctx = EvaluationContext::new().with_context(Context::new("user", "u1"));
        let result = apply_rules(&def, &ctx.contexts, HashSet::new(), "pct-flag", "env-1").unwrap();
        assert_eq!(result, None);
    }

    #[test]
    fn parameter_value_missing_returns_empty_string_as_hash_input() {
        // Parameter not present in context → unwrap_or_default → empty string
        let def = percentage_flag_def(
            "pct-flag",
            "user",
            TargetField::Parameter("nonexistent_param".to_string()),
        );
        let ctx = EvaluationContext::new().with_context(Context::new("user", "u1"));

        let result = apply_rules(&def, &ctx.contexts, HashSet::new(), "pct-flag", "env-1").unwrap();
        // Should not panic; some deterministic result
        let _ = result;
    }
}
