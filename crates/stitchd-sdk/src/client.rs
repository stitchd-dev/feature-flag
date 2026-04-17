//! SDK client — initialization, background polling, and flag evaluation.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;

use stitchd_core::{
    context::EvaluationContext,
    hashing::calculate_allocation,
    id::SegmentId,
    rule_engine::{
        evaluate_rules,
        types::{EvaluationInput, RuleOutput, TargetField},
    },
    variants::VariantValue,
};

use crate::{
    cache::{DefinitionCache, collect_segment_ids},
    config::SdkConfig,
    error::SdkError,
    grpc_client::SdkGrpcClient,
    http_client::SdkHttpClient,
};

/// Thread-safe SDK client. Obtain via [`SdkClient::init`].
pub struct SdkClient {
    cache: Arc<RwLock<DefinitionCache>>,
    http_client: SdkHttpClient,
    cancel: CancellationToken,
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

        let poll_cache = Arc::clone(&cache);
        let poll_cancel = cancel.clone();
        let poll_interval = config.poll_interval;
        let poll_grpc = SdkGrpcClient::new(&config.grpc_url, &config.sdk_key);

        tokio::spawn(async move {
            let mut interval = tokio::time::interval(poll_interval);
            interval.tick().await; // skip the immediate first tick
            loop {
                tokio::select! {
                    () = poll_cancel.cancelled() => break,
                    _ = interval.tick() => {
                        if let Ok(resp) = poll_grpc.fetch_definitions().await {
                            if let Ok(new_cache) = DefinitionCache::from_sync_response(resp) {
                                *poll_cache.write().await = new_cache;
                            }
                        }
                    }
                }
            }
        });

        Ok(Arc::new(Self { cache, http_client: http, cancel }))
    }

    /// Evaluate a feature flag for the given context.
    ///
    /// Returns `None` if the flag is disabled or not found.
    /// For list-based segments this makes one REST call per context type referenced.
    pub async fn evaluate(
        &self,
        flag_key: &str,
        eval_ctx: &EvaluationContext,
    ) -> Result<Option<VariantValue>, SdkError> {
        let cache = self.cache.read().await;

        let flag_def = match cache.flags.get(flag_key) {
            Some(f) => f,
            None => return Ok(None),
        };

        if !flag_def.enabled {
            return Ok(None);
        }

        let env_id_str = cache
            .environment_id
            .map(|id| id.to_string())
            .unwrap_or_default();

        // Resolve segment membership.
        let segment_ids = collect_segment_ids(&flag_def.rules);
        let mut resolved: HashSet<SegmentId> = HashSet::new();

        for seg_id in &segment_ids {
            if let Some(rule_seg) = cache.rule_segments.get(seg_id) {
                if rule_seg.evaluate(&eval_ctx.contexts)?.matched {
                    resolved.insert(*seg_id);
                }
            } else if let Some(list_meta) = cache.list_segments.get(seg_id) {
                let ctx_key = eval_ctx
                    .get_context(&list_meta.context_type)
                    .map(|c| c.key.as_str())
                    .unwrap_or("");

                let memberships = self
                    .http_client
                    .list_check(
                        &env_id_str,
                        &list_meta.context_type,
                        ctx_key,
                        &[list_meta.key.clone()],
                    )
                    .await?;

                if memberships.get(&list_meta.key).copied().unwrap_or(false) {
                    resolved.insert(*seg_id);
                }
            }
        }

        let input = EvaluationInput {
            contexts: &eval_ctx.contexts,
            resolved_segments: resolved,
            evaluated_flags: HashMap::new(),
        };

        let rules = &flag_def.rules;
        let variant_map = &flag_def.variant_map;

        match evaluate_rules(rules, &input)? {
            None => Ok(None),
            Some(RuleOutput::Variant(vid)) => {
                Ok(variant_map.get(vid).map(|(_, v)| v.clone()))
            }
            Some(RuleOutput::Percentage { targets, weights }) => {
                let mut target_values: Vec<String> = Vec::new();
                for target in targets {
                    if let Some(ctx) = eval_ctx.get_context(&target.context_type) {
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

                let pct = calculate_allocation(flag_key, &env_id_str, &target_values);
                let bucket = ((pct * 10.0).floor() as u32).min(999);

                let mut cumulative = 0u32;
                for (vid, weight) in weights {
                    cumulative += weight;
                    if bucket < cumulative {
                        return Ok(variant_map.get(vid).map(|(_, v)| v.clone()));
                    }
                }
                Ok(None)
            }
        }
    }
}

impl Drop for SdkClient {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}
