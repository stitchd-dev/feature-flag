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
}
