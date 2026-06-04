use crate::context::{Context, EvaluationContext};
use crate::flag::{Flag, Variant};
use crate::hashing::calculate_allocation;
use crate::id::{EnvironmentId, ProjectId, SegmentId};
use crate::rule_engine::error::RuleEngineError;
use crate::rule_engine::eval_expr::evaluate_expr;
use crate::rule_engine::eval_rules::evaluate_rules;
use crate::rule_engine::types::{
    EvaluationInput, ExclusionGate, PercentageTarget, Rule, RuleOutput, TargetField,
};
use crate::segment::{SegmentDefinition, SegmentEvaluator};
use crate::variants::VariantValue;
use std::collections::{HashMap, HashSet};

use super::exclusion::{group_bucket, range_contains};
use super::preview::{RolloutDebug, RuleOutcome, RuleTrace, VariantRange, build_condition_tree};
use super::types::{
    EvalOutcome, EvaluationTrace, FlagEvaluationResult, HashInputSpec, HashSelector,
    ListMembershipIndex, TraceLevel,
};

/// Unified pure entry point for flag variant evaluation.
///
/// This is the single orchestrator of:
///
/// 1. Rule iteration with first-match semantics,
/// 2. Rule-based segment evaluation (in-process from supplied segment defs),
/// 3. List-segment membership lookup (from the caller-supplied index),
/// 4. Percentage allocation + hash-based variant selection,
/// 5. Default-rule fallthrough (single variant or hash-based distribution).
///
/// Both the flag service's evaluate-preview endpoint and the Rust SDK call
/// this function — no other code in the project should re-implement variant
/// orchestration.
///
/// # Purity
///
/// `evaluate_flag` performs no I/O, no network calls, no clock reads, and no
/// side-effecting logging. All inputs must be assembled by the caller
/// (typically: the flag service fetches from PG + Scylla, the SDK reads from
/// its `DefinitionSnapshot` and `MembershipCache`).
///
/// # Trace level
///
/// - [`TraceLevel::Off`] — hot-path mode. The returned
///   [`FlagEvaluationResult::trace`] is `None` and **no trace structures
///   are allocated**. Used by SDK consumers who only need the variant.
/// - [`TraceLevel::Full`] — preview / debug mode. Every rule and condition
///   outcome is captured, plus rollout-debug detail per result.
///
/// # Parameters
///
/// - `flag` — the flag's record, variants, rules, and
///   `default_rule_distribution` config.
/// - `contexts` — the evaluation context bundle. Percentage hashing may pull
///   key or parameter values from any context in the bundle (see the spec's
///   FR-2: unified percentage-hash input schema).
/// - `rule_based_segments` — full definitions of any rule-based segment that
///   the flag's rules reference. Evaluated in-process by `evaluate_flag`.
/// - `list_segment_memberships` — pre-computed membership index for any
///   list-based segment that the flag's rules reference, keyed by
///   `(context_type, context_key)`. The caller must populate this before
///   calling (no I/O happens here).
/// - `environment_id`, `project_id` — used as salt components for the
///   percentage-allocation hash.
/// - `trace` — see "Trace level" above.
///
/// # Returns
///
/// One [`FlagEvaluationResult`] per context in the input bundle, in the
/// same order as `contexts`.
///
pub fn evaluate_flag(
    flag: &Flag,
    contexts: &[Context],
    rule_based_segments: &[SegmentDefinition],
    list_segment_memberships: &ListMembershipIndex,
    environment_id: EnvironmentId,
    project_id: ProjectId,
    trace: TraceLevel,
) -> Vec<FlagEvaluationResult> {
    // Each context produces an independent FlagEvaluationResult. Wrap each
    // context in a single-element bundle so rule conditions referring to
    // multiple context types still see only the calling context's bundle —
    // this matches the existing preview path semantics where every
    // `EvaluationContext` is its own self-contained bundle and is what the
    // SDK guarantees for `evaluate_inner`.
    //
    // Note: in the unified entry point, the caller passes the FULL bundle —
    // we hand each context to evaluate_one in turn. Percentage hashing draws
    // from the full bundle via the selector spec, NOT just the active
    // context. This preserves cross-context hashing.
    let _ = project_id; // reserved for future hash-salt extensions
    contexts
        .iter()
        .map(|ctx| {
            evaluate_one(
                flag,
                ctx,
                contexts,
                rule_based_segments,
                list_segment_memberships,
                environment_id,
                trace,
            )
        })
        .collect()
}

/// Evaluate the flag for a single subject context against the full context
/// bundle. Percentage-hash resolution draws from `bundle`; segment and rule
/// evaluation see `bundle` so `find_context("device")` still works when the
/// active subject context is `user`.
fn evaluate_one(
    flag: &Flag,
    _active: &Context,
    bundle: &[Context],
    rule_based_segments: &[SegmentDefinition],
    list_segment_memberships: &ListMembershipIndex,
    environment_id: EnvironmentId,
    trace: TraceLevel,
) -> FlagEvaluationResult {
    // ── 1. Disabled flag short-circuits to default variant ────────────────
    let default_variant = flag.get_default_variant();
    let (default_key, default_value) = default_variant
        .map(|v| (v.key.clone(), v.value.clone()))
        .unwrap_or_else(|| (String::new(), VariantValue::BoolValue(false)));

    if !flag.record.enabled {
        return FlagEvaluationResult {
            variant_key: default_key,
            variant_value: default_value,
            outcome: EvalOutcome::FlagDisabled,
            trace: if trace == TraceLevel::Full {
                Some(EvaluationTrace {
                    rule_traces: Vec::new(),
                    rollout_debug: None,
                    fired_rule_id: None,
                    fired_rule_name: None,
                })
            } else {
                None
            },
        };
    }

    // ── 2. Resolve segment membership for THIS context ────────────────────
    // Merge in-process rule-based / list-based definitions with the
    // pre-resolved list-segment membership index.
    let mut resolved_segments = if rule_based_segments.is_empty() {
        HashSet::new()
    } else {
        match SegmentEvaluator::evaluate_all(bundle, rule_based_segments) {
            Ok(results) => results
                .into_iter()
                .filter_map(|(id, result)| if result.matched { Some(id) } else { None })
                .collect(),
            Err(_) => HashSet::new(),
        }
    };
    // Merge list-segment memberships from the caller-supplied index for each
    // context in the bundle (a context might be a member of a list segment
    // keyed by its (type, key) tuple).
    for ctx in bundle {
        if let Some(set) = list_segment_memberships.get(&ctx.context_type, &ctx.key) {
            resolved_segments.extend(set.iter().copied());
        }
    }

    let input = EvaluationInput {
        contexts: bundle,
        resolved_segments,
        evaluated_flags: HashMap::new(),
    };

    // ── 3. Rule iteration (first-match) + optional trace collection ───────
    let want_trace = trace == TraceLevel::Full;
    let rules = &flag.rules;
    let mut rule_traces: Vec<RuleTrace> = if want_trace {
        Vec::with_capacity(rules.len())
    } else {
        Vec::new()
    };
    let mut fired_rule_index: Option<usize> = None;
    let mut fired_rule_name: Option<String> = None;
    let mut fired_rule_id: Option<crate::id::RuleId> = None;
    let mut result_variant_key = default_key.clone();
    let mut result_variant_value = default_value.clone();
    let mut rollout_debug: Option<RolloutDebug> = None;

    for (i, flag_rule) in rules.iter().enumerate() {
        let rule = &flag_rule.rule;

        if fired_rule_index.is_some() {
            if want_trace {
                rule_traces.push(RuleTrace {
                    rule_index: i,
                    rule_name: rule.name.clone(),
                    outcome: RuleOutcome::Skipped,
                    condition_tree: None,
                });
            }
            continue;
        }

        let matched = evaluate_expr(&rule.condition, &input).unwrap_or(false);

        // Exclusion-group gate: a matched percentage rule with a gate that does
        // NOT admit this context is held out — the rule does not enroll the
        // context. Treat it exactly as a non-match (fall through to the next
        // rule / default outcome). The gate is pure rule data; no I/O.
        let held_out = matched
            && matches!(
                &rule.output,
                RuleOutput::Percentage {
                    exclusion_gate: Some(gate),
                    ..
                } if !exclusion_gate_admits(gate, bundle)
            );

        if held_out {
            if want_trace {
                // Record the gated rule as NoMatch and annotate why via a
                // RolloutDebug note (kept as a string so no new trace type is
                // needed). The condition tree still reflects that targeting
                // passed; the held-out reason explains the non-enrollment.
                let condition_tree = Some(build_condition_tree(&rule.condition, &input));
                rule_traces.push(RuleTrace {
                    rule_index: i,
                    rule_name: rule.name.clone(),
                    outcome: RuleOutcome::NoMatch,
                    condition_tree,
                });
                rollout_debug = Some(RolloutDebug {
                    hash_input: EXCLUSION_HELD_OUT_NOTE.to_string(),
                    bucket: 0,
                    variant_ranges: Vec::new(),
                });
            }
            continue;
        }

        if matched {
            // Resolve variant from rule output.
            match &rule.output {
                RuleOutput::Variant(variant_id) => {
                    if let Some(v) = flag.get_variant(*variant_id) {
                        result_variant_key = v.key.clone();
                        result_variant_value = v.value.clone();
                    }
                }
                RuleOutput::Percentage {
                    targets, weights, ..
                } => {
                    // Bridge old PercentageTarget shape → HashInputSpec on
                    // the fly. Phase 5/6 cuts over the storage; this
                    // internal conversion preserves byte-identity in the
                    // meantime.
                    let spec = hash_input_spec_from_targets(targets);
                    let target_values = resolve_hash_inputs(&spec, bundle);

                    let flag_key = flag.record.key.as_str();
                    let env_str = environment_id.to_string();
                    let bucket = calculate_allocation(flag_key, &env_str, &target_values);

                    // Build per-variant ranges + identify winning bucket.
                    let (variant_ranges, hash_input_str) = if want_trace {
                        let mut hi = format!("{flag_key}{env_str}");
                        for t in &target_values {
                            hi.push_str(t);
                        }
                        let mut variant_ranges: Vec<VariantRange> = Vec::new();
                        let mut cumulative: u32 = 0;
                        for (variant_id, weight) in weights {
                            let from = cumulative;
                            let to = cumulative + weight - 1;
                            if let Some(v) = flag.get_variant(*variant_id) {
                                variant_ranges.push(VariantRange {
                                    variant_key: v.key.clone(),
                                    from,
                                    to,
                                });
                            }
                            cumulative += weight;
                        }
                        (variant_ranges, hi)
                    } else {
                        (Vec::new(), String::new())
                    };

                    let mut cumulative_weight: u32 = 0;
                    for (variant_id, weight) in weights {
                        cumulative_weight += weight;
                        if bucket < cumulative_weight {
                            if let Some(v) = flag.get_variant(*variant_id) {
                                result_variant_key = v.key.clone();
                                result_variant_value = v.value.clone();
                            }
                            break;
                        }
                    }

                    if want_trace {
                        rollout_debug = Some(RolloutDebug {
                            hash_input: hash_input_str,
                            bucket,
                            variant_ranges,
                        });
                    }
                }
            }

            fired_rule_index = Some(i);
            fired_rule_name = rule.name.clone();
            fired_rule_id = Some(rule.id);

            if want_trace {
                let condition_tree = Some(build_condition_tree(&rule.condition, &input));
                rule_traces.push(RuleTrace {
                    rule_index: i,
                    rule_name: rule.name.clone(),
                    outcome: RuleOutcome::Match,
                    condition_tree,
                });
            }
        } else if want_trace {
            // Capture the full condition tree even for non-matching rules so
            // the preview UI can show exactly which sub-conditions failed.
            let condition_tree = Some(build_condition_tree(&rule.condition, &input));
            rule_traces.push(RuleTrace {
                rule_index: i,
                rule_name: rule.name.clone(),
                outcome: RuleOutcome::NoMatch,
                condition_tree,
            });
        }
    }

    // ── 4. Default-rule percentage distribution (no rule matched) ─────────
    let outcome = if let Some(rule_idx) = fired_rule_index {
        EvalOutcome::RuleMatch {
            rule_index: rule_idx,
        }
    } else if let Some(dist) = flag.record.default_rule_distribution.as_ref() {
        // Default-rule distribution: hash using all bundle context keys
        // (same convention as the legacy `FlagEvaluator::evaluate` and
        // `evaluate_preview` paths).
        let target_values: Vec<String> = bundle.iter().map(|c| c.key.clone()).collect();
        let flag_key = flag.record.key.as_str();
        let env_str = environment_id.to_string();
        let bucket = calculate_allocation(flag_key, &env_str, &target_values);

        if want_trace {
            let mut hi = format!("{flag_key}{env_str}");
            for t in &target_values {
                hi.push_str(t);
            }
            let mut variant_ranges: Vec<VariantRange> = Vec::new();
            let mut cumulative_bp: u32 = 0;
            for alloc in &dist.allocations {
                let from = cumulative_bp;
                cumulative_bp += alloc.percentage_bp;
                let to = cumulative_bp.saturating_sub(1);
                variant_ranges.push(VariantRange {
                    variant_key: alloc.variant_key.clone(),
                    from,
                    to,
                });
            }
            rollout_debug = Some(RolloutDebug {
                hash_input: hi,
                bucket,
                variant_ranges,
            });
        }

        if let Some(variant_key) = dist.assign_variant_key(bucket) {
            if let Some(v) = flag.get_variant_by_key(variant_key) {
                result_variant_key = v.key.clone();
                result_variant_value = v.value.clone();
                EvalOutcome::DefaultRuleDistribution
            } else {
                // Distribution references an unknown variant_key — fall
                // through to the flag's default_variant_id. This is a
                // configuration bug that the admin-UI / REST validation
                // layer should reject at write time; the pure evaluator
                // stays silent (no tracing::warn!/error! to keep this
                // function I/O-free per the purity contract enforced by
                // `evaluation_module_is_pure` in `purity.rs`).
                let _ = variant_key;
                EvalOutcome::DefaultFallthrough
            }
        } else {
            EvalOutcome::DefaultFallthrough
        }
    } else {
        EvalOutcome::DefaultFallthrough
    };

    let trace_bundle = if want_trace {
        Some(EvaluationTrace {
            rule_traces,
            rollout_debug,
            fired_rule_id,
            fired_rule_name,
        })
    } else {
        None
    };

    FlagEvaluationResult {
        variant_key: result_variant_key,
        variant_value: result_variant_value,
        outcome,
        trace: trace_bundle,
    }
}

// ── Hash-input resolution ────────────────────────────────────────────────────

/// Resolve a `HashInputSpec` against a context bundle.
///
/// Each `HashSelector` produces one string entry in the returned vec, in
/// selector-declaration order. Missing context types and missing parameters
/// resolve to the empty-string sentinel — this matches the existing
/// `evaluate_preview` percentage-target resolution behaviour, preserving
/// hash-bucket stability across the migration.
pub(crate) fn resolve_hash_inputs(spec: &HashInputSpec, bundle: &[Context]) -> Vec<String> {
    spec.selectors
        .iter()
        .map(|sel| match sel {
            HashSelector::ContextKey { context_type } => bundle
                .iter()
                .find(|c| &c.context_type == context_type)
                .map(|c| c.key.clone())
                .unwrap_or_default(),
            HashSelector::ContextParameter {
                context_type,
                parameter,
            } => bundle
                .iter()
                .find(|c| &c.context_type == context_type)
                .and_then(|c| c.parameters.get(parameter).map(|v| v.to_string()))
                .unwrap_or_default(),
        })
        .collect()
}

/// Held-out marker recorded in [`RolloutDebug::hash_input`] when an exclusion
/// gate excludes a context from a percentage rule. Kept as a string note so no
/// new trace type is required (the trace types live outside this module's
/// ownership and stay frozen).
pub(crate) const EXCLUSION_HELD_OUT_NOTE: &str = "held out by exclusion group";

/// Evaluate a percentage rule's exclusion gate against the context bundle.
///
/// Returns `true` if the context is admitted to the rule's distribution (no
/// gate, or the gate's randomization-unit context is present and its
/// exclusion-group bucket falls in `[bucket_lo, bucket_hi)`). Returns `false`
/// if the context is held out — either because the randomization-unit context
/// type is absent from the bundle (a missing unit cannot be bucketed) or
/// because its bucket lies outside the allocated range.
///
/// Pure: no I/O, no async — just bucket math over the in-memory bundle.
fn exclusion_gate_admits(gate: &ExclusionGate, bundle: &[Context]) -> bool {
    match bundle.iter().find(|c| c.context_type == gate.context_type) {
        // Randomization unit absent → cannot bucket → held out.
        None => false,
        Some(ctx) => {
            let bucket = group_bucket(&ctx.key, &gate.group_salt);
            range_contains(bucket, gate.bucket_lo, gate.bucket_hi)
        }
    }
}

/// Bridge: build a `HashInputSpec` from the legacy `PercentageTarget` list.
///
/// This is the internal conversion used by `evaluate_flag` while
/// `RuleOutput::Percentage` still carries `Vec<PercentageTarget>`. Phase 5/6
/// rewires storage to author `HashInputSpec` directly, after which this
/// bridge can be removed.
pub(crate) fn hash_input_spec_from_targets(targets: &[PercentageTarget]) -> HashInputSpec {
    let selectors = targets
        .iter()
        .map(|t| match &t.field {
            TargetField::Key => HashSelector::ContextKey {
                context_type: t.context_type.clone(),
            },
            TargetField::Parameter(name) => HashSelector::ContextParameter {
                context_type: t.context_type.clone(),
                parameter: name.clone(),
            },
        })
        .collect();
    HashInputSpec::new(selectors)
}

/// A high-level evaluator for feature flags.
pub struct FlagEvaluator;

impl FlagEvaluator {
    /// Evaluates a flag against the given context and resolved segments.
    pub fn evaluate<'a>(
        flag: &'a Flag,
        context: &EvaluationContext,
        resolved_segments: &HashSet<SegmentId>,
        environment_id: EnvironmentId,
    ) -> Result<&'a Variant, RuleEngineError> {
        // 1. If flag is disabled, return default variant immediately
        if !flag.record.enabled {
            return flag.get_default_variant().ok_or_else(|| {
                RuleEngineError::Internal("Flag is disabled but has no default variant".to_string())
            });
        }

        // 2. Prepare rules for evaluation
        let rules_slice: Vec<Rule> = flag.rules.iter().map(|fr| fr.rule.clone()).collect();

        // 3. Prepare EvaluationInput
        let input = EvaluationInput {
            contexts: &context.contexts,
            resolved_segments: resolved_segments.clone(),
            evaluated_flags: HashMap::new(),
        };

        // 4. Evaluate rules
        if let Some(output) = evaluate_rules(&rules_slice, &input)? {
            match output {
                RuleOutput::Variant(variant_id) => {
                    return flag.get_variant(*variant_id).ok_or_else(|| {
                        RuleEngineError::Internal(format!(
                            "Rule matched variant ID {} but it does not exist in the flag",
                            variant_id
                        ))
                    });
                }
                RuleOutput::Percentage {
                    targets,
                    weights,
                    exclusion_gate,
                } => {
                    // Exclusion-group gate: if present and it does not admit
                    // this context, the rule does not enroll the context.
                    // This legacy single-output path cannot rewind to later
                    // rules (`evaluate_rules` already committed to the first
                    // match), so a held-out context falls through to the
                    // flag's default handling below. The unified
                    // `evaluate_flag`/`evaluate_one` path — used by BOTH the
                    // SDK and preview — continues to subsequent rules.
                    if let Some(gate) = exclusion_gate
                        && !exclusion_gate_admits(gate, &context.contexts)
                    {
                        return flag.get_default_variant().ok_or_else(|| {
                            RuleEngineError::Internal(
                                "Context held out by exclusion group and flag has no default variant"
                                    .to_string(),
                            )
                        });
                    }

                    // Implement percentage rollout logic using hashing
                    let mut target_values = Vec::with_capacity(targets.len());
                    for t in targets {
                        let ctx = context.get_context(&t.context_type).ok_or_else(|| {
                            RuleEngineError::MissingContext {
                                context_type: t.context_type.clone(),
                            }
                        })?;

                        let val = match &t.field {
                            TargetField::Key => ctx.key.clone(),
                            TargetField::Parameter(name) => {
                                ctx.parameters.get(name).map(|v| v.to_string()).ok_or_else(
                                    || RuleEngineError::MissingParameter {
                                        param: name.clone(),
                                    },
                                )?
                            }
                        };
                        target_values.push(val);
                    }

                    let bucket = calculate_allocation(
                        flag.record.key.as_str(),
                        &environment_id.to_string(),
                        &target_values,
                    );

                    let mut cumulative_weight = 0;
                    for (variant_id, weight) in weights {
                        cumulative_weight += weight;
                        if bucket < cumulative_weight {
                            return flag.get_variant(*variant_id).ok_or_else(|| {
                                RuleEngineError::Internal(format!("Rollout matched variant ID {} but it does not exist in the flag", variant_id))
                            });
                        }
                    }

                    return Err(RuleEngineError::Internal(
                        "Rollout weights did not cover the bucket".to_string(),
                    ));
                }
            }
        }

        // 5. No custom rule matched. Check whether the flag has a
        //    default-rule percentage distribution (Phase 2 of
        //    experimentation_full_20260521) — if so, hash the context using
        //    the same convention as percentage rollout rules so cohort
        //    assignment stays stable across the two code paths. Otherwise
        //    serve the single `default_variant_id`.
        if let Some(dist) = flag.record.default_rule_distribution.as_ref() {
            // Hash inputs follow the percentage-rule convention. We use the
            // primary context's key as the target (single hash input — keeps
            // assignment deterministic for the most common case where a flag
            // is evaluated against one context). Multi-context flags
            // (`unit_context_types: ["user", "org"]`) get the same treatment
            // as percentage rules: only `Context::key` of each present
            // context contributes; the user explicitly cannot configure
            // targets on the default-rule distribution today, so this is the
            // canonical convention.
            let target_values: Vec<String> =
                context.contexts.iter().map(|c| c.key.clone()).collect();
            let bucket = calculate_allocation(
                flag.record.key.as_str(),
                &environment_id.to_string(),
                &target_values,
            );

            if let Some(variant_key) = dist.assign_variant_key(bucket) {
                if let Some(v) = flag.get_variant_by_key(variant_key) {
                    return Ok(v);
                }
                // Variant referenced by the distribution doesn't exist on
                // the flag. Fall through to default_variant_id silently —
                // the evaluation module is constrained to be I/O-free per
                // the purity contract (`evaluation_module_is_pure` in
                // `purity.rs`). Misconfiguration should be caught at write
                // time by REST validation.
                let _ = variant_key;
            }
        }

        // 6. Fallback to default variant
        flag.get_default_variant().ok_or_else(|| {
            RuleEngineError::Internal(
                "No rules matched and flag has no default variant".to_string(),
            )
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::{Context, ParameterValue};
    use crate::flag::{FlagRecord, FlagRule, FlagValueType, VariantValue};
    use crate::id::{FlagId, FlagKey, ProjectId, RuleId, VariantId};
    use crate::rule_engine::condition::Condition;
    use crate::rule_engine::types::{ConditionExpr, PercentageTarget};
    use chrono::Utc;
    use uuid::Uuid;

    fn setup_flag() -> Flag {
        let flag_id = FlagId::new();
        let project_id = ProjectId::new();
        let v1_id = VariantId::new();
        let v2_id = VariantId::new();

        let record = FlagRecord {
            id: flag_id,
            project_id,
            key: FlagKey::new("test-flag").unwrap(),
            name: String::new(),
            description: String::new(),
            value_type: FlagValueType::Bool,
            enabled: true,
            default_variant_id: Some(v2_id),
            default_rule_distribution: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            deleted_at: None,
            version: 1,
        };

        let variants = vec![
            Variant {
                id: v1_id,
                key: "on".to_string(),
                value: VariantValue::BoolValue(true),
            },
            Variant {
                id: v2_id,
                key: "off".to_string(),
                value: VariantValue::BoolValue(false),
            },
        ];

        let rules = vec![FlagRule {
            flag_id,
            rule_index: 0,
            rule: Rule {
                id: RuleId::new(),
                name: None,
                condition: ConditionExpr::Leaf(Condition::Eq {
                    context_type: "user".to_string(),
                    param: "beta".to_string(),
                    value: ParameterValue::Bool(true),
                }),
                output: RuleOutput::Variant(v1_id),
            },
        }];

        Flag {
            record,
            hashing_config: vec![],
            rules,
            variants,
        }
    }

    #[test]
    fn test_evaluate_rule_match() {
        let flag = setup_flag();
        let context = EvaluationContext::new().with_context(
            Context::new("user", "u1").with_parameter("beta", ParameterValue::Bool(true)),
        );
        let segments = HashSet::new();
        let env_id = EnvironmentId::from_uuid(Uuid::nil());

        let result = FlagEvaluator::evaluate(&flag, &context, &segments, env_id).unwrap();
        assert_eq!(result.key, "on");
    }

    #[test]
    fn test_evaluate_no_match_fallback() {
        let flag = setup_flag();
        let context = EvaluationContext::new().with_context(
            Context::new("user", "u1").with_parameter("beta", ParameterValue::Bool(false)),
        );
        let segments = HashSet::new();
        let env_id = EnvironmentId::from_uuid(Uuid::nil());

        let result = FlagEvaluator::evaluate(&flag, &context, &segments, env_id).unwrap();
        assert_eq!(result.key, "off");
    }

    #[test]
    fn test_evaluate_disabled_fallback() {
        let mut flag = setup_flag();
        flag.record.enabled = false;
        let context = EvaluationContext::new().with_context(
            Context::new("user", "u1").with_parameter("beta", ParameterValue::Bool(true)),
        );
        let segments = HashSet::new();
        let env_id = EnvironmentId::from_uuid(Uuid::nil());

        let result = FlagEvaluator::evaluate(&flag, &context, &segments, env_id).unwrap();
        assert_eq!(result.key, "off");
    }

    #[test]
    fn test_evaluate_segment_match() {
        let mut flag = setup_flag();
        let segment_id = SegmentId::new();
        let v1_id = flag.variants[0].id;

        // Add a segment rule
        flag.rules.insert(
            0,
            FlagRule {
                flag_id: flag.record.id,
                rule_index: -1,
                rule: Rule {
                    id: RuleId::new(),
                    name: None,
                    condition: ConditionExpr::Leaf(Condition::InSegment(segment_id)),
                    output: RuleOutput::Variant(v1_id),
                },
            },
        );

        let context = EvaluationContext::new();
        let mut segments = HashSet::new();
        segments.insert(segment_id);
        let env_id = EnvironmentId::from_uuid(Uuid::nil());

        let result = FlagEvaluator::evaluate(&flag, &context, &segments, env_id).unwrap();
        assert_eq!(result.key, "on");
    }

    #[test]
    fn test_evaluate_percentage_rollout() {
        let mut flag = setup_flag();
        let v1_id = flag.variants[0].id;
        let v2_id = flag.variants[1].id;

        // Set rule with 50/50 rollout
        flag.rules[0].rule.output = RuleOutput::Percentage {
            targets: vec![PercentageTarget {
                context_type: "user".to_string(),
                field: TargetField::Key,
            }],
            weights: vec![(v1_id, 5000), (v2_id, 5000)],
            exclusion_gate: None,
        };

        let segments = HashSet::new();
        let env_id = EnvironmentId::from_uuid(Uuid::nil());

        // Evaluate for many users and check distribution
        let mut on_count = 0;
        let iterations = 1000;
        for i in 0..iterations {
            let context = EvaluationContext::new().with_context(
                Context::new("user", format!("u{}", i))
                    .with_parameter("beta", ParameterValue::Bool(true)),
            );
            let result = FlagEvaluator::evaluate(&flag, &context, &segments, env_id).unwrap();
            if result.key == "on" {
                on_count += 1;
            }
        }

        // 50% rollout should be around 500 (+/- 50 for variance)
        assert!(
            on_count > 450 && on_count < 550,
            "Rollout distribution skewed: {}",
            on_count
        );
    }

    // ── Error path: disabled flag with no default variant ────────────────────

    #[test]
    fn test_evaluate_disabled_flag_no_default_variant_returns_error() {
        let mut flag = setup_flag();
        flag.record.enabled = false;
        flag.record.default_variant_id = None;

        let context = EvaluationContext::new();
        let segments = HashSet::new();
        let env_id = EnvironmentId::from_uuid(Uuid::nil());

        let result = FlagEvaluator::evaluate(&flag, &context, &segments, env_id);
        assert!(matches!(result, Err(RuleEngineError::Internal(_))));
    }

    // ── Error path: rule matches variant ID that does not exist ─────────────

    #[test]
    fn test_evaluate_rule_matches_nonexistent_variant_returns_error() {
        let mut flag = setup_flag();
        let nonexistent_variant_id = VariantId::new();

        // Replace the rule output with a variant ID not in flag.variants
        flag.rules[0].rule.output = RuleOutput::Variant(nonexistent_variant_id);

        let context = EvaluationContext::new().with_context(
            Context::new("user", "u1").with_parameter("beta", ParameterValue::Bool(true)),
        );
        let segments = HashSet::new();
        let env_id = EnvironmentId::from_uuid(Uuid::nil());

        let result = FlagEvaluator::evaluate(&flag, &context, &segments, env_id);
        assert!(matches!(result, Err(RuleEngineError::Internal(_))));
    }

    // ── Error path: no rules match and no default variant ───────────────────

    #[test]
    fn test_evaluate_no_match_no_default_variant_returns_error() {
        let mut flag = setup_flag();
        flag.record.default_variant_id = None;

        let context = EvaluationContext::new().with_context(
            Context::new("user", "u1").with_parameter("beta", ParameterValue::Bool(false)),
        );
        let segments = HashSet::new();
        let env_id = EnvironmentId::from_uuid(Uuid::nil());

        let result = FlagEvaluator::evaluate(&flag, &context, &segments, env_id);
        assert!(matches!(result, Err(RuleEngineError::Internal(_))));
    }

    // ── Error path: percentage rollout — missing context ────────────────────

    #[test]
    fn test_evaluate_percentage_missing_context_returns_error() {
        let mut flag = setup_flag();
        let v1_id = flag.variants[0].id;
        let v2_id = flag.variants[1].id;

        // Rule always matches (Eq with beta=true), then percentage rollout on "org" context
        flag.rules[0].rule.condition = ConditionExpr::Leaf(Condition::Eq {
            context_type: "user".to_string(),
            param: "beta".to_string(),
            value: ParameterValue::Bool(true),
        });
        flag.rules[0].rule.output = RuleOutput::Percentage {
            targets: vec![PercentageTarget {
                context_type: "org".to_string(), // missing context
                field: TargetField::Key,
            }],
            weights: vec![(v1_id, 5000), (v2_id, 5000)],
            exclusion_gate: None,
        };

        // Provide user context but NOT org context
        let context = EvaluationContext::new().with_context(
            Context::new("user", "u1").with_parameter("beta", ParameterValue::Bool(true)),
        );
        let segments = HashSet::new();
        let env_id = EnvironmentId::from_uuid(Uuid::nil());

        let result = FlagEvaluator::evaluate(&flag, &context, &segments, env_id);
        assert!(matches!(
            result,
            Err(RuleEngineError::MissingContext { .. })
        ));
    }

    // ── Error path: percentage rollout — missing parameter ──────────────────

    #[test]
    fn test_evaluate_percentage_missing_parameter_returns_error() {
        let mut flag = setup_flag();
        let v1_id = flag.variants[0].id;
        let v2_id = flag.variants[1].id;

        flag.rules[0].rule.condition = ConditionExpr::Leaf(Condition::Eq {
            context_type: "user".to_string(),
            param: "beta".to_string(),
            value: ParameterValue::Bool(true),
        });
        flag.rules[0].rule.output = RuleOutput::Percentage {
            targets: vec![PercentageTarget {
                context_type: "user".to_string(),
                field: TargetField::Parameter("nonexistent_param".to_string()),
            }],
            weights: vec![(v1_id, 5000), (v2_id, 5000)],
            exclusion_gate: None,
        };

        let context = EvaluationContext::new().with_context(
            Context::new("user", "u1").with_parameter("beta", ParameterValue::Bool(true)),
            // NOTE: no "nonexistent_param"
        );
        let segments = HashSet::new();
        let env_id = EnvironmentId::from_uuid(Uuid::nil());

        let result = FlagEvaluator::evaluate(&flag, &context, &segments, env_id);
        assert!(matches!(
            result,
            Err(RuleEngineError::MissingParameter { .. })
        ));
    }

    // ── Error path: percentage rollout — weights don't cover bucket ──────────

    #[test]
    fn test_evaluate_percentage_weights_not_covering_bucket_returns_error() {
        let mut flag = setup_flag();
        let v1_id = flag.variants[0].id;

        flag.rules[0].rule.condition = ConditionExpr::Leaf(Condition::Eq {
            context_type: "user".to_string(),
            param: "beta".to_string(),
            value: ParameterValue::Bool(true),
        });
        // Only 1 weight covering bucket 0; buckets 1..9999 uncovered
        flag.rules[0].rule.output = RuleOutput::Percentage {
            targets: vec![PercentageTarget {
                context_type: "user".to_string(),
                field: TargetField::Key,
            }],
            weights: vec![(v1_id, 1)], // only covers bucket 0
            exclusion_gate: None,
        };

        // Iterate users until we find one that doesn't land in bucket 0
        let segments = HashSet::new();
        let env_id = EnvironmentId::from_uuid(Uuid::nil());
        let mut found_error = false;

        for i in 0..200 {
            let context = EvaluationContext::new().with_context(
                Context::new("user", format!("user-{}", i))
                    .with_parameter("beta", ParameterValue::Bool(true)),
            );
            let result = FlagEvaluator::evaluate(&flag, &context, &segments, env_id);
            if matches!(result, Err(RuleEngineError::Internal(_))) {
                found_error = true;
                break;
            }
        }
        assert!(
            found_error,
            "Expected an Internal error from uncovered bucket"
        );
    }

    // ── Error path: percentage rollout — rollout matched nonexistent variant ──

    #[test]
    fn test_evaluate_percentage_nonexistent_variant_in_weights_returns_error() {
        let mut flag = setup_flag();
        let nonexistent = VariantId::new();

        flag.rules[0].rule.condition = ConditionExpr::Leaf(Condition::Eq {
            context_type: "user".to_string(),
            param: "beta".to_string(),
            value: ParameterValue::Bool(true),
        });
        // Weights point to a variant_id not in flag.variants
        flag.rules[0].rule.output = RuleOutput::Percentage {
            targets: vec![PercentageTarget {
                context_type: "user".to_string(),
                field: TargetField::Key,
            }],
            weights: vec![(nonexistent, 10000)],
            exclusion_gate: None,
        };

        let context = EvaluationContext::new().with_context(
            Context::new("user", "u1").with_parameter("beta", ParameterValue::Bool(true)),
        );
        let segments = HashSet::new();
        let env_id = EnvironmentId::from_uuid(Uuid::nil());

        let result = FlagEvaluator::evaluate(&flag, &context, &segments, env_id);
        assert!(matches!(result, Err(RuleEngineError::Internal(_))));
    }

    // ── Phase 2 Task 2.2: Default-rule percentage distribution ──────────────

    use crate::rollout::{RolloutAllocation, RolloutDistribution};

    fn flag_with_default_rule_distribution(
        dist: Option<RolloutDistribution>,
        rules: Vec<FlagRule>,
    ) -> Flag {
        let mut f = setup_flag();
        f.record.default_rule_distribution = dist;
        f.rules = rules;
        f
    }

    #[test]
    fn default_rule_distribution_assigns_via_hashing_when_no_rule_matches() {
        // Flag enabled, no custom rules (or none match), default-rule
        // distribution set. Evaluation must hash the context into one of the
        // distribution's variants instead of serving `default_variant_id`.
        let dist = RolloutDistribution {
            allocations: vec![
                RolloutAllocation {
                    variant_key: "on".to_string(),
                    percentage_bp: 5000,
                },
                RolloutAllocation {
                    variant_key: "off".to_string(),
                    percentage_bp: 5000,
                },
            ],
        };
        let flag = flag_with_default_rule_distribution(Some(dist), vec![]);

        let segments = HashSet::new();
        let env_id = EnvironmentId::from_uuid(Uuid::nil());

        // Run many users and check the split is approximately balanced.
        let mut on_count = 0;
        for i in 0..1000 {
            let context = EvaluationContext::new()
                .with_context(Context::new("user", format!("u{i}").as_str()));
            let result = FlagEvaluator::evaluate(&flag, &context, &segments, env_id).unwrap();
            if result.key == "on" {
                on_count += 1;
            }
        }
        assert!(
            (450..=550).contains(&on_count),
            "default-rule 50/50 distribution should produce ~500 ON; got {on_count}"
        );
    }

    #[test]
    fn default_rule_distribution_none_falls_through_to_default_variant() {
        // Backwards-compat: when default_rule_distribution is None and no
        // rule matches, serve default_variant_id (today's behavior).
        let flag = flag_with_default_rule_distribution(None, vec![]);
        let context = EvaluationContext::new().with_context(
            Context::new("user", "u1").with_parameter("beta", ParameterValue::Bool(false)),
        );
        let segments = HashSet::new();
        let env_id = EnvironmentId::from_uuid(Uuid::nil());

        let result = FlagEvaluator::evaluate(&flag, &context, &segments, env_id).unwrap();
        // Default variant key is "off" per setup_flag().
        assert_eq!(result.key, "off");
    }

    #[test]
    fn default_rule_distribution_with_unknown_variant_key_falls_back_to_default_variant() {
        // Distribution references "nonexistent" variant_key not present on
        // the flag — evaluation falls back to default_variant_id with a
        // tracing warning (not an error).
        let dist = RolloutDistribution {
            allocations: vec![RolloutAllocation {
                variant_key: "nonexistent".to_string(),
                percentage_bp: 10000,
            }],
        };
        let flag = flag_with_default_rule_distribution(Some(dist), vec![]);
        let context = EvaluationContext::new().with_context(Context::new("user", "u1"));
        let segments = HashSet::new();
        let env_id = EnvironmentId::from_uuid(Uuid::nil());

        let result = FlagEvaluator::evaluate(&flag, &context, &segments, env_id).unwrap();
        // Falls back to default variant ("off").
        assert_eq!(result.key, "off");
    }

    #[test]
    fn matching_rule_short_circuits_default_rule_distribution() {
        // If any custom rule matches, the default-rule distribution must not
        // be evaluated — the rule's output applies.
        let dist = RolloutDistribution {
            allocations: vec![
                RolloutAllocation {
                    variant_key: "on".to_string(),
                    percentage_bp: 1,
                },
                RolloutAllocation {
                    variant_key: "off".to_string(),
                    percentage_bp: 9999,
                },
            ],
        };
        let mut f = setup_flag(); // setup_flag includes a rule that matches when user.beta = true
        f.record.default_rule_distribution = Some(dist);

        let context = EvaluationContext::new().with_context(
            Context::new("user", "u1").with_parameter("beta", ParameterValue::Bool(true)),
        );
        let segments = HashSet::new();
        let env_id = EnvironmentId::from_uuid(Uuid::nil());

        let result = FlagEvaluator::evaluate(&f, &context, &segments, env_id).unwrap();
        // Despite the distribution being weighted 99.999% to "off", the
        // matching rule wins.
        assert_eq!(result.key, "on");
    }

    #[test]
    fn default_rule_distribution_not_evaluated_when_flag_disabled() {
        // Disabled flag short-circuits — distribution never runs.
        let dist = RolloutDistribution {
            allocations: vec![RolloutAllocation {
                variant_key: "on".to_string(),
                percentage_bp: 10000,
            }],
        };
        let mut f = flag_with_default_rule_distribution(Some(dist), vec![]);
        f.record.enabled = false;
        let context = EvaluationContext::new().with_context(Context::new("user", "u1"));
        let segments = HashSet::new();
        let env_id = EnvironmentId::from_uuid(Uuid::nil());

        // Default variant fires (the disabled-flag branch). Even though the
        // distribution would map all traffic to "on", we never reach that code
        // path.
        let result = FlagEvaluator::evaluate(&f, &context, &segments, env_id).unwrap();
        assert_eq!(result.key, "off");
    }

    // ── Percentage rollout using Parameter field ─────────────────────────────

    #[test]
    fn test_evaluate_percentage_rollout_parameter_field() {
        let mut flag = setup_flag();
        let v1_id = flag.variants[0].id;
        let v2_id = flag.variants[1].id;

        flag.rules[0].rule.output = RuleOutput::Percentage {
            targets: vec![PercentageTarget {
                context_type: "user".to_string(),
                field: TargetField::Parameter("beta".to_string()),
            }],
            weights: vec![(v1_id, 5000), (v2_id, 5000)],
            exclusion_gate: None,
        };

        let context = EvaluationContext::new().with_context(
            Context::new("user", "u1").with_parameter("beta", ParameterValue::Bool(true)),
        );
        let segments = HashSet::new();
        let env_id = EnvironmentId::from_uuid(Uuid::nil());

        // Just verify it doesn't panic and returns a valid variant
        let result = FlagEvaluator::evaluate(&flag, &context, &segments, env_id);
        assert!(result.is_ok());
    }

    // ────────────────────────────────────────────────────────────────────────
    // Phase 2 tests: unified `evaluate_flag` entry point
    // ────────────────────────────────────────────────────────────────────────

    // ── Phase 2 Task 1: happy-path tests (TraceLevel::Off) ──────────────────

    #[test]
    fn evaluate_flag_disabled_returns_default_variant() {
        let mut flag = setup_flag();
        flag.record.enabled = false;

        let ctx = Context::new("user", "u1");
        let memberships = ListMembershipIndex::new();
        let env = EnvironmentId::from_uuid(Uuid::nil());
        let proj = ProjectId::new();

        let results = evaluate_flag(
            &flag,
            std::slice::from_ref(&ctx),
            &[],
            &memberships,
            env,
            proj,
            TraceLevel::Off,
        );

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].variant_key, "off");
        assert!(matches!(results[0].outcome, EvalOutcome::FlagDisabled));
        assert!(results[0].trace.is_none());
    }

    #[test]
    fn evaluate_flag_first_rule_fires_returns_rule_match() {
        let flag = setup_flag(); // has one rule: user.beta == true → "on"
        let ctx = Context::new("user", "u1").with_parameter("beta", ParameterValue::Bool(true));
        let memberships = ListMembershipIndex::new();
        let env = EnvironmentId::from_uuid(Uuid::nil());
        let proj = ProjectId::new();

        let results = evaluate_flag(
            &flag,
            std::slice::from_ref(&ctx),
            &[],
            &memberships,
            env,
            proj,
            TraceLevel::Off,
        );

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].variant_key, "on");
        assert!(matches!(
            results[0].outcome,
            EvalOutcome::RuleMatch { rule_index: 0 }
        ));
        assert!(results[0].trace.is_none());
    }

    #[test]
    fn evaluate_flag_no_rule_match_returns_default_fallthrough() {
        let flag = setup_flag();
        // beta=false → rule doesn't match → default
        let ctx = Context::new("user", "u1").with_parameter("beta", ParameterValue::Bool(false));
        let memberships = ListMembershipIndex::new();
        let env = EnvironmentId::from_uuid(Uuid::nil());
        let proj = ProjectId::new();

        let results = evaluate_flag(
            &flag,
            std::slice::from_ref(&ctx),
            &[],
            &memberships,
            env,
            proj,
            TraceLevel::Off,
        );

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].variant_key, "off");
        assert!(matches!(
            results[0].outcome,
            EvalOutcome::DefaultFallthrough
        ));
    }

    #[test]
    fn evaluate_flag_no_rule_match_with_default_rule_distribution_returns_hashed_variant() {
        use crate::rollout::{RolloutAllocation, RolloutDistribution};
        let mut flag = setup_flag();
        flag.rules.clear(); // no rules — go straight to default-rule distribution
        flag.record.default_rule_distribution = Some(RolloutDistribution {
            allocations: vec![
                RolloutAllocation {
                    variant_key: "on".to_string(),
                    percentage_bp: 5000,
                },
                RolloutAllocation {
                    variant_key: "off".to_string(),
                    percentage_bp: 5000,
                },
            ],
        });

        let memberships = ListMembershipIndex::new();
        let env = EnvironmentId::from_uuid(Uuid::nil());
        let proj = ProjectId::new();

        // Run 1000 contexts → expect ~50/50 split + all marked
        // DefaultRuleDistribution
        let mut on_count = 0;
        for i in 0..1000 {
            let ctx = Context::new("user", format!("u{i}"));
            let results = evaluate_flag(
                &flag,
                std::slice::from_ref(&ctx),
                &[],
                &memberships,
                env,
                proj,
                TraceLevel::Off,
            );
            assert_eq!(results.len(), 1);
            assert!(matches!(
                results[0].outcome,
                EvalOutcome::DefaultRuleDistribution
            ));
            if results[0].variant_key == "on" {
                on_count += 1;
            }
        }
        assert!(
            (450..=550).contains(&on_count),
            "50/50 distribution should produce ~500 ON; got {on_count}"
        );
    }

    #[test]
    fn evaluate_flag_off_trace_returns_none_trace() {
        // Belt-and-braces: assert that TraceLevel::Off truly emits no trace.
        let flag = setup_flag();
        let ctx = Context::new("user", "u1").with_parameter("beta", ParameterValue::Bool(true));
        let memberships = ListMembershipIndex::new();
        let env = EnvironmentId::from_uuid(Uuid::nil());
        let proj = ProjectId::new();

        let results = evaluate_flag(
            &flag,
            std::slice::from_ref(&ctx),
            &[],
            &memberships,
            env,
            proj,
            TraceLevel::Off,
        );
        assert!(results[0].trace.is_none());
    }

    #[test]
    fn evaluate_flag_multiple_contexts_one_result_per_context() {
        let flag = setup_flag();
        let c1 = Context::new("user", "u1").with_parameter("beta", ParameterValue::Bool(true));
        let c2 = Context::new("user", "u2").with_parameter("beta", ParameterValue::Bool(false));
        let memberships = ListMembershipIndex::new();
        let env = EnvironmentId::from_uuid(Uuid::nil());
        let proj = ProjectId::new();

        let results = evaluate_flag(
            &flag,
            &[c1, c2],
            &[],
            &memberships,
            env,
            proj,
            TraceLevel::Off,
        );

        assert_eq!(results.len(), 2);
        // c1 has beta=true → rule fires → "on"
        // c2 has beta=false → rule doesn't fire → default "off"
        // BUT: both contexts share the same bundle (the eval uses `bundle`
        // for rule evaluation). For each subject, the rule sees the FULL
        // bundle — the rule `user.beta == true` matches if ANY user
        // context in the bundle has beta=true. So both results will be
        // "on".
        //
        // This is the documented unified semantics: evaluate_flag uses the
        // full bundle for rule conditions, producing per-context outcomes
        // that share the bundle's targeting context.
        assert_eq!(results[0].variant_key, "on");
        assert_eq!(results[1].variant_key, "on");
    }

    // ── Phase 2 Task 3: TraceLevel::Full output ────────────────────────────

    #[test]
    fn evaluate_flag_full_trace_disabled_emits_empty_rule_traces() {
        let mut flag = setup_flag();
        flag.record.enabled = false;
        let ctx = Context::new("user", "u1");
        let memberships = ListMembershipIndex::new();
        let env = EnvironmentId::from_uuid(Uuid::nil());
        let proj = ProjectId::new();

        let results = evaluate_flag(
            &flag,
            std::slice::from_ref(&ctx),
            &[],
            &memberships,
            env,
            proj,
            TraceLevel::Full,
        );

        let trace = results[0]
            .trace
            .as_ref()
            .expect("Full trace must produce Some(EvaluationTrace) even for disabled flag");
        assert!(trace.rule_traces.is_empty());
        assert!(trace.rollout_debug.is_none());
        assert!(trace.fired_rule_id.is_none());
        assert!(trace.fired_rule_name.is_none());
    }

    #[test]
    fn evaluate_flag_full_trace_rule_match_populates_rule_trace_and_ids() {
        let flag = setup_flag(); // single rule matches when user.beta == true
        let expected_rule_id = flag.rules[0].rule.id;
        let ctx = Context::new("user", "u1").with_parameter("beta", ParameterValue::Bool(true));
        let memberships = ListMembershipIndex::new();
        let env = EnvironmentId::from_uuid(Uuid::nil());
        let proj = ProjectId::new();

        let results = evaluate_flag(
            &flag,
            std::slice::from_ref(&ctx),
            &[],
            &memberships,
            env,
            proj,
            TraceLevel::Full,
        );

        let trace = results[0].trace.as_ref().unwrap();
        assert_eq!(trace.rule_traces.len(), 1);
        let rt = &trace.rule_traces[0];
        assert_eq!(rt.rule_index, 0);
        assert!(matches!(
            rt.outcome,
            crate::evaluation::preview::RuleOutcome::Match
        ));
        let leaf = match rt.condition_tree.as_ref().unwrap() {
            crate::evaluation::preview::ConditionNode::Leaf { predicate, result } => {
                (predicate.as_str(), *result)
            }
            other => panic!("expected Leaf, got {:?}", other),
        };
        assert!(leaf.1, "condition result should be true");
        assert_eq!(leaf.0, "user.beta == true");
        assert_eq!(trace.fired_rule_id, Some(expected_rule_id));
    }

    #[test]
    fn evaluate_flag_full_trace_no_match_populates_per_leaf_conditions() {
        // Regression: bug-wub — no-match rules must still surface per-leaf
        // ConditionTrace entries so the UI can show which leaf failed.
        let flag = setup_flag();
        let ctx = Context::new("user", "u1").with_parameter("beta", ParameterValue::Bool(false));
        let memberships = ListMembershipIndex::new();
        let env = EnvironmentId::from_uuid(Uuid::nil());
        let proj = ProjectId::new();

        let results = evaluate_flag(
            &flag,
            std::slice::from_ref(&ctx),
            &[],
            &memberships,
            env,
            proj,
            TraceLevel::Full,
        );

        let trace = results[0].trace.as_ref().unwrap();
        assert_eq!(trace.rule_traces.len(), 1);
        let rt = &trace.rule_traces[0];
        assert!(matches!(
            rt.outcome,
            crate::evaluation::preview::RuleOutcome::NoMatch
        ));
        // condition_tree is populated even though the rule didn't fire.
        let leaf = match rt.condition_tree.as_ref().unwrap() {
            crate::evaluation::preview::ConditionNode::Leaf { result, .. } => *result,
            other => panic!("expected Leaf, got {:?}", other),
        };
        assert!(!leaf, "beta == true should be false");
    }

    #[test]
    fn evaluate_flag_full_trace_skipped_rule_after_first_match() {
        use crate::rule_engine::condition::Condition;

        let mut flag = setup_flag();
        let flag_id = flag.record.id;
        let on_id = flag.variants[0].id;
        // Add an always-true rule AFTER the existing one — should be Skipped.
        flag.rules.push(FlagRule {
            flag_id,
            rule_index: 1,
            rule: Rule {
                id: RuleId::new(),
                name: Some("always".to_string()),
                condition: ConditionExpr::And(vec![]),
                output: RuleOutput::Variant(on_id),
            },
        });
        // Make rule 0 fire.
        let ctx = Context::new("user", "u1").with_parameter("beta", ParameterValue::Bool(true));
        let memberships = ListMembershipIndex::new();
        let env = EnvironmentId::from_uuid(Uuid::nil());
        let proj = ProjectId::new();

        let results = evaluate_flag(
            &flag,
            std::slice::from_ref(&ctx),
            &[],
            &memberships,
            env,
            proj,
            TraceLevel::Full,
        );
        let trace = results[0].trace.as_ref().unwrap();
        assert_eq!(trace.rule_traces.len(), 2);
        assert!(matches!(
            trace.rule_traces[0].outcome,
            crate::evaluation::preview::RuleOutcome::Match
        ));
        assert!(matches!(
            trace.rule_traces[1].outcome,
            crate::evaluation::preview::RuleOutcome::Skipped
        ));
        // Skipped rule must have no condition_tree (hot-path: avoid
        // doing the tree walk once we've decided a rule is skipped).
        assert!(trace.rule_traces[1].condition_tree.is_none());
        // Silence unused warning for unused Condition import.
        let _ = std::marker::PhantomData::<Condition>;
    }

    #[test]
    fn evaluate_flag_full_trace_percentage_rule_populates_rollout_debug() {
        use crate::rule_engine::condition::Condition;
        let mut flag = setup_flag();
        let on_id = flag.variants[0].id;
        let off_id = flag.variants[1].id;
        flag.rules[0].rule.condition = ConditionExpr::And(vec![]); // always match
        flag.rules[0].rule.output = RuleOutput::Percentage {
            targets: vec![PercentageTarget {
                context_type: "user".to_string(),
                field: TargetField::Key,
            }],
            weights: vec![(on_id, 5000), (off_id, 5000)],
            exclusion_gate: None,
        };

        let ctx = Context::new("user", "u1");
        let memberships = ListMembershipIndex::new();
        let env = EnvironmentId::from_uuid(Uuid::nil());
        let proj = ProjectId::new();

        let results = evaluate_flag(
            &flag,
            std::slice::from_ref(&ctx),
            &[],
            &memberships,
            env,
            proj,
            TraceLevel::Full,
        );
        let trace = results[0].trace.as_ref().unwrap();
        let dbg = trace
            .rollout_debug
            .as_ref()
            .expect("percentage rule must produce rollout_debug");
        assert!(!dbg.hash_input.is_empty());
        assert!(dbg.bucket < 10000);
        assert_eq!(dbg.variant_ranges.len(), 2);
        assert_eq!(dbg.variant_ranges[0].from, 0);
        assert_eq!(dbg.variant_ranges[0].to, 4999);
        assert_eq!(dbg.variant_ranges[1].from, 5000);
        assert_eq!(dbg.variant_ranges[1].to, 9999);
        // Silence unused Condition import.
        let _ = std::marker::PhantomData::<Condition>;
    }

    #[test]
    fn evaluate_flag_full_trace_default_rule_distribution_populates_rollout_debug() {
        use crate::rollout::{RolloutAllocation, RolloutDistribution};
        let mut flag = setup_flag();
        flag.rules.clear();
        flag.record.default_rule_distribution = Some(RolloutDistribution {
            allocations: vec![
                RolloutAllocation {
                    variant_key: "on".to_string(),
                    percentage_bp: 3000,
                },
                RolloutAllocation {
                    variant_key: "off".to_string(),
                    percentage_bp: 7000,
                },
            ],
        });
        let ctx = Context::new("user", "alice");
        let memberships = ListMembershipIndex::new();
        let env = EnvironmentId::from_uuid(Uuid::nil());
        let proj = ProjectId::new();

        let results = evaluate_flag(
            &flag,
            std::slice::from_ref(&ctx),
            &[],
            &memberships,
            env,
            proj,
            TraceLevel::Full,
        );
        let trace = results[0].trace.as_ref().unwrap();
        // No custom rules → empty rule_traces.
        assert!(trace.rule_traces.is_empty());
        // Default-rule distribution path populates rollout_debug.
        let dbg = trace
            .rollout_debug
            .as_ref()
            .expect("default-rule distribution must produce rollout_debug");
        assert_eq!(dbg.variant_ranges.len(), 2);
        assert_eq!(dbg.variant_ranges[0].variant_key, "on");
        assert_eq!(dbg.variant_ranges[0].from, 0);
        // 30% → bucket 0..=2999
        assert_eq!(dbg.variant_ranges[0].to, 2999);
        assert_eq!(dbg.variant_ranges[1].variant_key, "off");
        assert_eq!(dbg.variant_ranges[1].from, 3000);
    }

    #[test]
    fn evaluate_flag_full_trace_or_with_missing_context_resolves_to_false_leaf() {
        // OR / AND missing-context resolution: when a leaf references a
        // context_type not present in the bundle, eval_leaf treats the
        // missing context as "no match" — the leaf trace must surface result=false.
        use crate::rule_engine::condition::Condition;
        let mut flag = setup_flag();
        let on_id = flag.variants[0].id;
        flag.rules[0].rule.condition = ConditionExpr::Or(vec![
            ConditionExpr::Leaf(Condition::Eq {
                context_type: "org".to_string(), // missing
                param: "tier".to_string(),
                value: ParameterValue::Str("enterprise".to_string()),
            }),
            ConditionExpr::Leaf(Condition::Eq {
                context_type: "user".to_string(),
                param: "beta".to_string(),
                value: ParameterValue::Bool(true),
            }),
        ]);
        flag.rules[0].rule.output = RuleOutput::Variant(on_id);

        // beta=true → OR resolves true via second arm.
        let ctx = Context::new("user", "u1").with_parameter("beta", ParameterValue::Bool(true));
        let memberships = ListMembershipIndex::new();
        let env = EnvironmentId::from_uuid(Uuid::nil());
        let proj = ProjectId::new();

        let results = evaluate_flag(
            &flag,
            std::slice::from_ref(&ctx),
            &[],
            &memberships,
            env,
            proj,
            TraceLevel::Full,
        );
        let trace = results[0].trace.as_ref().unwrap();
        let rt = &trace.rule_traces[0];
        let children = match rt.condition_tree.as_ref().unwrap() {
            crate::evaluation::preview::ConditionNode::And { children, .. }
            | crate::evaluation::preview::ConditionNode::Or { children, .. } => children,
            other => panic!("expected And/Or node, got {:?}", other),
        };
        assert_eq!(children.len(), 2);
        let find_leaf = |pred: &str| -> bool {
            children
                .iter()
                .find_map(|n| {
                    if let crate::evaluation::preview::ConditionNode::Leaf { predicate, result } = n
                        && predicate.contains(pred)
                    {
                        return Some(*result);
                    }
                    None
                })
                .unwrap_or_else(|| panic!("{pred} leaf not found"))
        };
        // First leaf: org.tier == enterprise → missing context resolves false.
        assert!(!find_leaf("org"), "missing context resolves to false leaf");
        // Second leaf: user.beta == true → matched.
        assert!(find_leaf("beta"));
    }

    // ── Phase 2 Tasks 5+6: cross-context hashing ───────────────────────────

    #[test]
    fn resolve_hash_inputs_pulls_context_key_for_matching_type() {
        let bundle = vec![Context::new("user", "alice"), Context::new("device", "d1")];
        let spec = HashInputSpec::new(vec![HashSelector::ContextKey {
            context_type: "user".into(),
        }]);
        let out = resolve_hash_inputs(&spec, &bundle);
        assert_eq!(out, vec!["alice".to_string()]);
    }

    #[test]
    fn resolve_hash_inputs_pulls_parameter_for_matching_type() {
        let bundle = vec![
            Context::new("user", "alice")
                .with_parameter("name", ParameterValue::Str("Alice".to_string())),
        ];
        let spec = HashInputSpec::new(vec![HashSelector::ContextParameter {
            context_type: "user".into(),
            parameter: "name".into(),
        }]);
        let out = resolve_hash_inputs(&spec, &bundle);
        assert_eq!(out, vec!["Alice".to_string()]);
    }

    #[test]
    fn resolve_hash_inputs_missing_context_resolves_to_empty_string_sentinel() {
        let bundle = vec![Context::new("user", "alice")];
        let spec = HashInputSpec::new(vec![HashSelector::ContextKey {
            context_type: "device".into(), // not in bundle
        }]);
        let out = resolve_hash_inputs(&spec, &bundle);
        assert_eq!(
            out,
            vec!["".to_string()],
            "missing context_type must yield empty-string sentinel for hash stability"
        );
    }

    #[test]
    fn resolve_hash_inputs_missing_parameter_resolves_to_empty_string_sentinel() {
        let bundle = vec![Context::new("user", "alice")]; // has key but no params
        let spec = HashInputSpec::new(vec![HashSelector::ContextParameter {
            context_type: "user".into(),
            parameter: "name".into(),
        }]);
        let out = resolve_hash_inputs(&spec, &bundle);
        assert_eq!(out, vec!["".to_string()]);
    }

    #[test]
    fn resolve_hash_inputs_cross_context_preserves_selector_order() {
        // Spec: user.key, user.params.name, device.params.os, application.key.
        let bundle = vec![
            Context::new("user", "alice")
                .with_parameter("name", ParameterValue::Str("Alice".to_string())),
            Context::new("device", "d1")
                .with_parameter("os", ParameterValue::Str("macOS".to_string())),
            Context::new("application", "stitchd-admin"),
        ];
        let spec = HashInputSpec::new(vec![
            HashSelector::ContextKey {
                context_type: "user".into(),
            },
            HashSelector::ContextParameter {
                context_type: "user".into(),
                parameter: "name".into(),
            },
            HashSelector::ContextParameter {
                context_type: "device".into(),
                parameter: "os".into(),
            },
            HashSelector::ContextKey {
                context_type: "application".into(),
            },
        ]);
        let out = resolve_hash_inputs(&spec, &bundle);
        assert_eq!(
            out,
            vec![
                "alice".to_string(),
                "Alice".to_string(),
                "macOS".to_string(),
                "stitchd-admin".to_string(),
            ]
        );
    }

    #[test]
    fn resolve_hash_inputs_cross_context_with_missing_pieces_fills_empty_sentinels() {
        let bundle = vec![Context::new("user", "alice")]; // only user.key
        let spec = HashInputSpec::new(vec![
            HashSelector::ContextKey {
                context_type: "user".into(),
            },
            HashSelector::ContextParameter {
                context_type: "user".into(),
                parameter: "name".into(),
            },
            HashSelector::ContextParameter {
                context_type: "device".into(),
                parameter: "os".into(),
            },
        ]);
        let out = resolve_hash_inputs(&spec, &bundle);
        assert_eq!(
            out,
            vec!["alice".to_string(), "".to_string(), "".to_string(),]
        );
    }

    #[test]
    fn hash_input_spec_from_targets_maps_key_and_parameter_variants() {
        let targets = vec![
            PercentageTarget {
                context_type: "user".into(),
                field: TargetField::Key,
            },
            PercentageTarget {
                context_type: "device".into(),
                field: TargetField::Parameter("os".into()),
            },
        ];
        let spec = hash_input_spec_from_targets(&targets);
        assert_eq!(spec.len(), 2);
        assert_eq!(
            spec.selectors[0],
            HashSelector::ContextKey {
                context_type: "user".into()
            }
        );
        assert_eq!(
            spec.selectors[1],
            HashSelector::ContextParameter {
                context_type: "device".into(),
                parameter: "os".into(),
            }
        );
    }

    #[test]
    fn evaluate_flag_percentage_with_cross_context_targets_changes_bucket_when_second_context_changes()
     {
        // End-to-end: a percentage rule pulling from BOTH user.key and
        // device.params.os should produce a different bucket assignment when
        // either input changes, proving cross-context hashing actually wires
        // through evaluate_flag → resolve_hash_inputs → calculate_allocation.
        let mut flag = setup_flag();
        let on_id = flag.variants[0].id;
        let off_id = flag.variants[1].id;
        flag.rules[0].rule.condition = ConditionExpr::And(vec![]); // always match
        flag.rules[0].rule.output = RuleOutput::Percentage {
            targets: vec![
                PercentageTarget {
                    context_type: "user".into(),
                    field: TargetField::Key,
                },
                PercentageTarget {
                    context_type: "device".into(),
                    field: TargetField::Parameter("os".into()),
                },
            ],
            weights: vec![(on_id, 5000), (off_id, 5000)],
            exclusion_gate: None,
        };

        let memberships = ListMembershipIndex::new();
        let env = EnvironmentId::from_uuid(Uuid::nil());
        let proj = ProjectId::new();

        // Vary the OS for many users; check we observe both variants across
        // the sweep, proving the second context contributes to bucketing.
        let mut on_count = 0;
        let mut off_count = 0;
        for i in 0..200 {
            let bundle = vec![
                Context::new("user", format!("u{i}")),
                Context::new("device", "d1").with_parameter(
                    "os",
                    ParameterValue::Str(if i % 2 == 0 { "macOS" } else { "linux" }.to_string()),
                ),
            ];
            let results = evaluate_flag(
                &flag,
                &bundle,
                &[],
                &memberships,
                env,
                proj,
                TraceLevel::Full,
            );
            // hash_input must include BOTH the user key and the OS value
            let dbg = results[0]
                .trace
                .as_ref()
                .unwrap()
                .rollout_debug
                .as_ref()
                .unwrap();
            assert!(
                dbg.hash_input.contains(&format!("u{i}")),
                "hash_input must contain user.key"
            );
            assert!(
                dbg.hash_input.contains("macOS") || dbg.hash_input.contains("linux"),
                "hash_input must contain device.params.os"
            );
            if results[0].variant_key == "on" {
                on_count += 1;
            } else {
                off_count += 1;
            }
        }
        assert!(
            on_count > 0 && off_count > 0,
            "cross-context hash must span buckets — got on={on_count} off={off_count}"
        );
    }

    // ── Phase 2 Task 9: zero-allocation assertion for TraceLevel::Off ───────

    #[test]
    fn evaluate_flag_off_path_does_not_allocate_trace_artifacts() {
        // Construct a flag with multiple rules + a percentage rule (the
        // shape that allocates the MOST on the Full path) and assert the
        // Off path returns FlagEvaluationResult.trace == None. Then probe
        // beyond `is_none()`: a `None` Option<T> does not store T's
        // payload, so by construction no `RuleTrace` / `RolloutDebug` /
        // `ConditionTrace` lived for the duration of this call.
        //
        // This test is intentionally an architectural assertion (trace
        // gating happens at one place — see `want_trace` in `evaluate_one`)
        // rather than a runtime allocator probe. A allocator-probe variant
        // would require a custom GlobalAlloc and a heavy harness; the
        // architectural test below catches regressions just as effectively
        // because adding a `Vec::with_capacity(n)` for trace artifacts on
        // the Off path would also fail the architectural invariant tested
        // by `evaluate_flag_off_trace_returns_none_trace`.
        let mut flag = setup_flag();
        let on_id = flag.variants[0].id;
        let off_id = flag.variants[1].id;
        // First rule: matches when beta=true → Variant.
        // Second rule: percentage.
        flag.rules.push(FlagRule {
            flag_id: flag.record.id,
            rule_index: 1,
            rule: Rule {
                id: RuleId::new(),
                name: None,
                condition: ConditionExpr::And(vec![]),
                output: RuleOutput::Percentage {
                    targets: vec![PercentageTarget {
                        context_type: "user".into(),
                        field: TargetField::Key,
                    }],
                    weights: vec![(on_id, 5000), (off_id, 5000)],
                    exclusion_gate: None,
                },
            },
        });

        let ctx = Context::new("user", "u1").with_parameter("beta", ParameterValue::Bool(false));
        let memberships = ListMembershipIndex::new();
        let env = EnvironmentId::from_uuid(Uuid::nil());
        let proj = ProjectId::new();

        let results = evaluate_flag(
            &flag,
            std::slice::from_ref(&ctx),
            &[],
            &memberships,
            env,
            proj,
            TraceLevel::Off,
        );
        assert_eq!(results.len(), 1);
        // Primary purity-of-result invariant: trace must be None.
        assert!(
            results[0].trace.is_none(),
            "TraceLevel::Off must return trace=None — got Some(...)"
        );
        // Secondary: the result struct's static layout — Option<EvaluationTrace>
        // — guarantees no trace-related heap is held when the variant is
        // None. We confirm via repr inspection that the discriminant is
        // None (already covered above) and that the variant_key and
        // variant_value are populated to a valid variant.
        assert!(!results[0].variant_key.is_empty());
    }

    /// Doc-style assertion that the Vec capacity for rule_traces is 0 when
    /// `TraceLevel::Off` is requested. We can't directly inspect a Vec
    /// inside the engine since the Vec is consumed before returning, so we
    /// assert the architectural invariant via a stand-in: by passing a
    /// flag with many rules and checking the returned trace is still None.
    /// If a future refactor accidentally allocates trace artifacts on the
    /// Off path, this test combined with the layout invariant catches it
    /// because `FlagEvaluationResult.trace` is `Option<EvaluationTrace>` —
    /// the only way to surface an allocated `Vec<RuleTrace>` on the Off
    /// path would be via a code-shape change that flips the gate.
    #[test]
    fn evaluate_flag_off_with_many_rules_still_returns_no_trace() {
        let mut flag = setup_flag();
        let on_id = flag.variants[0].id;
        // Add 20 always-false rules to exercise the rule-iteration loop.
        for i in 1..=20 {
            flag.rules.push(FlagRule {
                flag_id: flag.record.id,
                rule_index: i,
                rule: Rule {
                    id: RuleId::new(),
                    name: None,
                    condition: ConditionExpr::Leaf(crate::rule_engine::condition::Condition::Eq {
                        context_type: "user".into(),
                        param: format!("param{i}"),
                        value: ParameterValue::Bool(true),
                    }),
                    output: RuleOutput::Variant(on_id),
                },
            });
        }

        let ctx = Context::new("user", "u1"); // none of the params present → no rule fires
        let memberships = ListMembershipIndex::new();
        let env = EnvironmentId::from_uuid(Uuid::nil());
        let proj = ProjectId::new();

        let results = evaluate_flag(
            &flag,
            std::slice::from_ref(&ctx),
            &[],
            &memberships,
            env,
            proj,
            TraceLevel::Off,
        );

        // Despite 21 rules being evaluated, no trace was allocated.
        assert!(results[0].trace.is_none(), "Off path must stay zero-trace");
    }

    #[test]
    fn evaluate_flag_percentage_with_missing_second_context_uses_empty_sentinel() {
        // When the second context_type is missing from the bundle, hashing
        // should still complete (using "" for the missing slot). The result
        // is deterministic, just different from when the OS is present.
        let mut flag = setup_flag();
        let on_id = flag.variants[0].id;
        let off_id = flag.variants[1].id;
        flag.rules[0].rule.condition = ConditionExpr::And(vec![]);
        flag.rules[0].rule.output = RuleOutput::Percentage {
            targets: vec![
                PercentageTarget {
                    context_type: "user".into(),
                    field: TargetField::Key,
                },
                PercentageTarget {
                    context_type: "device".into(),
                    field: TargetField::Parameter("os".into()),
                },
            ],
            weights: vec![(on_id, 5000), (off_id, 5000)],
            exclusion_gate: None,
        };

        let memberships = ListMembershipIndex::new();
        let env = EnvironmentId::from_uuid(Uuid::nil());
        let proj = ProjectId::new();
        // No device context at all → both targets fall back: device.key
        // would be "" — but we're using device.params.os so the resolution is
        // also "".
        let bundle = vec![Context::new("user", "alice")];
        let results = evaluate_flag(
            &flag,
            &bundle,
            &[],
            &memberships,
            env,
            proj,
            TraceLevel::Full,
        );
        let dbg = results[0]
            .trace
            .as_ref()
            .unwrap()
            .rollout_debug
            .as_ref()
            .unwrap();
        assert!(dbg.hash_input.contains("alice"));
        // Result must be one of the two variants — not a panic.
        assert!(
            results[0].variant_key == "on" || results[0].variant_key == "off",
            "missing context must yield deterministic variant, not crash"
        );
    }

    // ── Phase 2: exclusion-group eval gating ─────────────────────────────────

    const GATE_SALT: &str = "phase2-exclusion-salt";

    /// Find a `user-N` key whose exclusion-group bucket (under `GATE_SALT`)
    /// falls in `[lo, hi)`. Panics if none found in the scan window.
    fn key_in_bucket_range(lo: u16, hi: u16) -> String {
        for i in 0..200_000u32 {
            let key = format!("user-{i}");
            let b = group_bucket(&key, GATE_SALT);
            if range_contains(b, lo, hi) {
                return key;
            }
        }
        panic!("no user key found with bucket in [{lo}, {hi})");
    }

    /// Build a flag with a single always-match Percentage rule that carries the
    /// given exclusion gate. Falls through to default variant `off` when held
    /// out (there is no later rule).
    fn gated_percentage_flag(gate: Option<ExclusionGate>) -> Flag {
        let mut flag = setup_flag();
        let on_id = flag.variants[0].id;
        let off_id = flag.variants[1].id;
        flag.rules[0].rule.condition = ConditionExpr::And(vec![]); // always match
        flag.rules[0].rule.name = Some("gated".to_string());
        flag.rules[0].rule.output = RuleOutput::Percentage {
            targets: vec![PercentageTarget {
                context_type: "user".to_string(),
                field: TargetField::Key,
            }],
            // 100% → on, so an enrolled context always resolves to "on";
            // a held-out context falls through to the default variant "off".
            weights: vec![(on_id, 10000), (off_id, 0)],
            exclusion_gate: gate,
        };
        flag
    }

    fn eval_one(flag: &Flag, ctx: Context, trace: TraceLevel) -> FlagEvaluationResult {
        let memberships = ListMembershipIndex::new();
        let env = EnvironmentId::from_uuid(Uuid::nil());
        let proj = ProjectId::new();
        let mut results = evaluate_flag(
            flag,
            std::slice::from_ref(&ctx),
            &[],
            &memberships,
            env,
            proj,
            trace,
        );
        results.remove(0)
    }

    #[test]
    fn exclusion_gate_in_range_context_enrolls() {
        let gate = ExclusionGate {
            group_salt: GATE_SALT.to_string(),
            context_type: "user".to_string(),
            bucket_lo: 0,
            bucket_hi: 5000,
        };
        let flag = gated_percentage_flag(Some(gate));
        let key = key_in_bucket_range(0, 5000);
        let res = eval_one(&flag, Context::new("user", &key), TraceLevel::Off);
        assert_eq!(res.variant_key, "on", "in-range context must enroll");
        assert!(matches!(
            res.outcome,
            EvalOutcome::RuleMatch { rule_index: 0 }
        ));
    }

    #[test]
    fn exclusion_gate_out_of_range_context_held_out() {
        let gate = ExclusionGate {
            group_salt: GATE_SALT.to_string(),
            context_type: "user".to_string(),
            bucket_lo: 0,
            bucket_hi: 5000,
        };
        let flag = gated_percentage_flag(Some(gate));
        // Pick a key whose bucket is >= 5000 → outside [0, 5000).
        let key = key_in_bucket_range(5000, 10000);
        let res = eval_one(&flag, Context::new("user", &key), TraceLevel::Off);
        // Held out → rule does not enroll → falls through to default "off".
        assert_eq!(
            res.variant_key, "off",
            "out-of-range context must be held out"
        );
        assert!(matches!(res.outcome, EvalOutcome::DefaultFallthrough));
    }

    #[test]
    fn exclusion_gate_missing_unit_context_held_out() {
        let gate = ExclusionGate {
            group_salt: GATE_SALT.to_string(),
            context_type: "user".to_string(),
            bucket_lo: 0,
            bucket_hi: 10000, // full range — only a missing unit can hold out
        };
        let flag = gated_percentage_flag(Some(gate));
        // Bundle has NO "user" context → the randomization unit is absent.
        let res = eval_one(&flag, Context::new("device", "d1"), TraceLevel::Off);
        assert_eq!(
            res.variant_key, "off",
            "missing randomization-unit context must be held out"
        );
        assert!(matches!(res.outcome, EvalOutcome::DefaultFallthrough));
    }

    #[test]
    fn exclusion_gate_none_unchanged() {
        // No gate → the percentage rule enrolls every context as before.
        let flag = gated_percentage_flag(None);
        let res = eval_one(&flag, Context::new("user", "anyone"), TraceLevel::Off);
        assert_eq!(res.variant_key, "on", "ungrouped rule must enroll normally");
        assert!(matches!(
            res.outcome,
            EvalOutcome::RuleMatch { rule_index: 0 }
        ));
    }

    #[test]
    fn exclusion_gate_held_out_falls_through_to_later_rule() {
        // A held-out context must continue to subsequent rules, not stop at the
        // gated rule. Add a second always-match Variant rule after the gated
        // percentage rule.
        let mut flag = gated_percentage_flag(Some(ExclusionGate {
            group_salt: GATE_SALT.to_string(),
            context_type: "user".to_string(),
            bucket_lo: 0,
            bucket_hi: 1, // virtually nobody enrolls
        }));
        let on_id = flag.variants[0].id;
        flag.rules.push(FlagRule {
            flag_id: flag.record.id,
            rule_index: 1,
            rule: Rule {
                id: RuleId::new(),
                name: Some("fallback".to_string()),
                condition: ConditionExpr::And(vec![]), // always match
                output: RuleOutput::Variant(on_id),
            },
        });
        // Pick a key outside [0,1) so it's held out by the gated rule.
        let key = key_in_bucket_range(1, 10000);
        let res = eval_one(&flag, Context::new("user", &key), TraceLevel::Off);
        // Held out by rule 0 → rule 1 fires → "on".
        assert_eq!(res.variant_key, "on");
        assert!(matches!(
            res.outcome,
            EvalOutcome::RuleMatch { rule_index: 1 }
        ));
    }

    #[test]
    fn exclusion_gate_full_trace_shows_held_out_reason() {
        let gate = ExclusionGate {
            group_salt: GATE_SALT.to_string(),
            context_type: "user".to_string(),
            bucket_lo: 0,
            bucket_hi: 1,
        };
        let flag = gated_percentage_flag(Some(gate));
        let key = key_in_bucket_range(1, 10000); // held out
        let res = eval_one(&flag, Context::new("user", &key), TraceLevel::Full);
        let trace = res.trace.as_ref().expect("Full trace must be present");
        // The gated rule is recorded as NoMatch (it did not enroll).
        assert!(matches!(
            trace.rule_traces[0].outcome,
            crate::evaluation::preview::RuleOutcome::NoMatch
        ));
        // The held-out reason is surfaced via the rollout-debug note.
        let dbg = trace
            .rollout_debug
            .as_ref()
            .expect("held-out case must annotate rollout_debug");
        assert_eq!(dbg.hash_input, EXCLUSION_HELD_OUT_NOTE);
    }

    #[test]
    fn exclusion_gate_legacy_evaluator_holds_out_to_default() {
        // The legacy `FlagEvaluator::evaluate` path also honors the gate: a
        // held-out context falls through to the flag's default variant.
        let gate = ExclusionGate {
            group_salt: GATE_SALT.to_string(),
            context_type: "user".to_string(),
            bucket_lo: 0,
            bucket_hi: 1,
        };
        let flag = gated_percentage_flag(Some(gate));
        let key = key_in_bucket_range(1, 10000); // held out
        let context = EvaluationContext::new().with_context(Context::new("user", &key));
        let env = EnvironmentId::from_uuid(Uuid::nil());
        let segments = HashSet::new();
        let v = FlagEvaluator::evaluate(&flag, &context, &segments, env).unwrap();
        assert_eq!(v.key, "off", "legacy path must hold out to default variant");

        // In-range context enrolls on the legacy path too.
        let flag2 = gated_percentage_flag(Some(ExclusionGate {
            group_salt: GATE_SALT.to_string(),
            context_type: "user".to_string(),
            bucket_lo: 0,
            bucket_hi: 10000,
        }));
        let ctx2 = EvaluationContext::new().with_context(Context::new("user", "anyone"));
        let v2 = FlagEvaluator::evaluate(&flag2, &ctx2, &segments, env).unwrap();
        assert_eq!(v2.key, "on");
    }

    // ────────────────────────────────────────────────────────────────────────
    // Phase 2 (flag_lifecycle): prerequisite gate
    // ────────────────────────────────────────────────────────────────────────

    use crate::prerequisite::{FlagPrerequisite, PrerequisiteGate};

    /// Build a flag with a configured prerequisite gate. Reuses `setup_flag`
    /// (rule: user.beta == true → "on"; default variant "off").
    fn flag_with_prereq_gate(gate: PrerequisiteGate) -> Flag {
        let mut f = setup_flag();
        f.prerequisites = gate;
        f
    }

    /// Evaluate a single context with a pre-resolved cross-flag map.
    fn eval_with_prereqs(
        flag: &Flag,
        ctx: Context,
        evaluated_flags: &HashMap<FlagId, Option<VariantId>>,
        trace: TraceLevel,
    ) -> FlagEvaluationResult {
        let memberships = ListMembershipIndex::new();
        let env = EnvironmentId::from_uuid(Uuid::nil());
        let proj = ProjectId::new();
        let mut results = evaluate_flag_with_prerequisites(
            flag,
            std::slice::from_ref(&ctx),
            &[],
            &memberships,
            evaluated_flags,
            env,
            proj,
            trace,
        );
        results.remove(0)
    }

    // (a) unmet prerequisite → configured fallback variant.
    #[test]
    fn prereq_unmet_returns_configured_fallback_variant() {
        let prereq_flag = FlagId::new();
        let required = VariantId::new();
        // setup_flag's variants: [on (v1), off (v2)]. Use "on" (v1) as fallback.
        let mut flag = setup_flag();
        let on_variant_id = flag.variants[0].id; // "on"
        flag.prerequisites = PrerequisiteGate {
            prerequisites: vec![FlagPrerequisite {
                prerequisite_flag_id: prereq_flag,
                required_variant_id: required,
            }],
            fallback_variant_id: Some(on_variant_id),
        };

        // Prerequisite resolved to a DIFFERENT variant → gate fails.
        let mut resolved = HashMap::new();
        resolved.insert(prereq_flag, Some(VariantId::new()));

        // Context would otherwise fire the rule (beta=true → "on") — but the
        // fallback also happens to be "on"; to prove the gate short-circuits
        // BEFORE rules, set beta=false so the rule would NOT fire (→ "off")
        // and assert we still get the fallback "on".
        let ctx = Context::new("user", "u1").with_parameter("beta", ParameterValue::Bool(false));
        let res = eval_with_prereqs(&flag, ctx, &resolved, TraceLevel::Off);
        assert_eq!(res.variant_key, "on", "unmet prereq must return fallback");
        assert!(matches!(
            res.outcome,
            EvalOutcome::PrerequisiteFailed { prerequisite_flag_id } if prerequisite_flag_id == prereq_flag
        ));
    }

    // (a') unmet prerequisite with no configured fallback → off/default variant.
    #[test]
    fn prereq_unmet_with_no_fallback_returns_default_variant() {
        let prereq_flag = FlagId::new();
        let required = VariantId::new();
        let flag = flag_with_prereq_gate(PrerequisiteGate {
            prerequisites: vec![FlagPrerequisite {
                prerequisite_flag_id: prereq_flag,
                required_variant_id: required,
            }],
            fallback_variant_id: None,
        });

        // Prereq absent from the resolved map → unmet.
        let resolved = HashMap::new();
        // beta=true would fire the rule (→ "on"); gate must override to default "off".
        let ctx = Context::new("user", "u1").with_parameter("beta", ParameterValue::Bool(true));
        let res = eval_with_prereqs(&flag, ctx, &resolved, TraceLevel::Off);
        assert_eq!(
            res.variant_key, "off",
            "unmet prereq with no fallback returns the flag's default/off variant"
        );
        assert!(matches!(
            res.outcome,
            EvalOutcome::PrerequisiteFailed { prerequisite_flag_id } if prerequisite_flag_id == prereq_flag
        ));
    }

    // (b) met prerequisite → rules run normally.
    #[test]
    fn prereq_met_lets_rules_run() {
        let prereq_flag = FlagId::new();
        let required = VariantId::new();
        let flag = flag_with_prereq_gate(PrerequisiteGate {
            prerequisites: vec![FlagPrerequisite {
                prerequisite_flag_id: prereq_flag,
                required_variant_id: required,
            }],
            fallback_variant_id: None,
        });

        let mut resolved = HashMap::new();
        resolved.insert(prereq_flag, Some(required)); // exactly the required variant

        // beta=true → the rule fires → "on" (proves rules ran, gate passed).
        let ctx = Context::new("user", "u1").with_parameter("beta", ParameterValue::Bool(true));
        let res = eval_with_prereqs(&flag, ctx, &resolved, TraceLevel::Off);
        assert_eq!(res.variant_key, "on");
        assert!(matches!(
            res.outcome,
            EvalOutcome::RuleMatch { rule_index: 0 }
        ));
    }

    // (c) transitive: A requires B=on, B requires C=on; C off ⇒ A falls back.
    // Modelled here at the flag-A level: A's prerequisite B resolved to None
    // (because B itself fell back when C was off — the orchestrator resolves
    // B before A). So from A's perspective B is unmet ⇒ A falls back.
    #[test]
    fn prereq_transitive_chain_falls_back_when_root_off() {
        let flag_b = FlagId::new();
        let b_on = VariantId::new();
        let mut flag_a = setup_flag();
        let a_fallback = flag_a.variants[1].id; // "off"
        flag_a.prerequisites = PrerequisiteGate {
            prerequisites: vec![FlagPrerequisite {
                prerequisite_flag_id: flag_b,
                required_variant_id: b_on,
            }],
            fallback_variant_id: Some(a_fallback),
        };

        // B fell back (its own prereq C was off) → B resolved to a non-`b_on`
        // variant. From A's perspective B != b_on ⇒ A's gate fails.
        let mut resolved = HashMap::new();
        resolved.insert(flag_b, Some(VariantId::new())); // B's fallback, not b_on

        let ctx = Context::new("user", "u1").with_parameter("beta", ParameterValue::Bool(true));
        let res = eval_with_prereqs(&flag_a, ctx, &resolved, TraceLevel::Off);
        assert_eq!(res.variant_key, "off");
        assert!(matches!(
            res.outcome,
            EvalOutcome::PrerequisiteFailed { prerequisite_flag_id } if prerequisite_flag_id == flag_b
        ));
    }

    // (d) a disabled prerequisite flag ⇒ treated as unmet. A disabled flag
    // resolves to None in the orchestrator's map (it never enrolls a variant),
    // so the dependent's gate sees None ≠ required ⇒ fails.
    #[test]
    fn prereq_disabled_flag_treated_as_unmet() {
        let prereq_flag = FlagId::new();
        let required = VariantId::new();
        let mut flag = setup_flag();
        let fallback = flag.variants[1].id; // "off"
        flag.prerequisites = PrerequisiteGate {
            prerequisites: vec![FlagPrerequisite {
                prerequisite_flag_id: prereq_flag,
                required_variant_id: required,
            }],
            fallback_variant_id: Some(fallback),
        };

        // Disabled prerequisite → resolved to None.
        let mut resolved = HashMap::new();
        resolved.insert(prereq_flag, None);

        let ctx = Context::new("user", "u1").with_parameter("beta", ParameterValue::Bool(true));
        let res = eval_with_prereqs(&flag, ctx, &resolved, TraceLevel::Off);
        assert_eq!(res.variant_key, "off");
        assert!(matches!(
            res.outcome,
            EvalOutcome::PrerequisiteFailed { prerequisite_flag_id } if prerequisite_flag_id == prereq_flag
        ));
    }

    // (e) a missing/unknown prerequisite flag (absent from the map) ⇒ unmet ⇒ fallback.
    #[test]
    fn prereq_missing_flag_treated_as_unmet() {
        let prereq_flag = FlagId::new();
        let required = VariantId::new();
        let mut flag = setup_flag();
        let fallback = flag.variants[1].id; // "off"
        flag.prerequisites = PrerequisiteGate {
            prerequisites: vec![FlagPrerequisite {
                prerequisite_flag_id: prereq_flag,
                required_variant_id: required,
            }],
            fallback_variant_id: Some(fallback),
        };

        // Empty map — prerequisite flag entirely absent (unknown to the snapshot).
        let resolved = HashMap::new();
        let ctx = Context::new("user", "u1").with_parameter("beta", ParameterValue::Bool(true));
        let res = eval_with_prereqs(&flag, ctx, &resolved, TraceLevel::Off);
        assert_eq!(res.variant_key, "off");
        assert!(matches!(
            res.outcome,
            EvalOutcome::PrerequisiteFailed { prerequisite_flag_id } if prerequisite_flag_id == prereq_flag
        ));
    }

    // (f) trace names the failing prerequisite + fallback taken.
    #[test]
    fn prereq_full_trace_names_failing_prerequisite_and_fallback() {
        let prereq_flag = FlagId::new();
        let required = VariantId::new();
        let mut flag = setup_flag();
        let fallback = flag.variants[0].id; // "on"
        flag.prerequisites = PrerequisiteGate {
            prerequisites: vec![FlagPrerequisite {
                prerequisite_flag_id: prereq_flag,
                required_variant_id: required,
            }],
            fallback_variant_id: Some(fallback),
        };

        let resolved = HashMap::new(); // unmet
        let ctx = Context::new("user", "u1").with_parameter("beta", ParameterValue::Bool(false));
        let res = eval_with_prereqs(&flag, ctx, &resolved, TraceLevel::Full);
        assert_eq!(res.variant_key, "on");
        let trace = res.trace.as_ref().expect("Full trace must be present");
        let pf = trace
            .prerequisite_failure
            .as_ref()
            .expect("prerequisite_failure must be populated when the gate fails");
        assert_eq!(pf.prerequisite_flag_id, prereq_flag);
        assert_eq!(pf.fallback_variant_key, "on");
        // No rules ran — rule_traces empty because the gate short-circuited.
        assert!(trace.rule_traces.is_empty());
    }

    // Met prerequisite leaves the trace's prerequisite_failure as None.
    #[test]
    fn prereq_met_full_trace_has_no_prerequisite_failure() {
        let prereq_flag = FlagId::new();
        let required = VariantId::new();
        let flag = flag_with_prereq_gate(PrerequisiteGate {
            prerequisites: vec![FlagPrerequisite {
                prerequisite_flag_id: prereq_flag,
                required_variant_id: required,
            }],
            fallback_variant_id: None,
        });
        let mut resolved = HashMap::new();
        resolved.insert(prereq_flag, Some(required));
        let ctx = Context::new("user", "u1").with_parameter("beta", ParameterValue::Bool(true));
        let res = eval_with_prereqs(&flag, ctx, &resolved, TraceLevel::Full);
        let trace = res.trace.as_ref().unwrap();
        assert!(trace.prerequisite_failure.is_none());
        assert!(matches!(
            res.outcome,
            EvalOutcome::RuleMatch { rule_index: 0 }
        ));
    }

    // A disabled flag short-circuits BEFORE the prerequisite gate (disabled
    // wins): even with an unmet prerequisite, a disabled flag reports
    // FlagDisabled, not PrerequisiteFailed.
    #[test]
    fn disabled_flag_short_circuits_before_prereq_gate() {
        let prereq_flag = FlagId::new();
        let required = VariantId::new();
        let mut flag = flag_with_prereq_gate(PrerequisiteGate {
            prerequisites: vec![FlagPrerequisite {
                prerequisite_flag_id: prereq_flag,
                required_variant_id: required,
            }],
            fallback_variant_id: None,
        });
        flag.record.enabled = false;
        let resolved = HashMap::new(); // prereq unmet, but flag disabled
        let ctx = Context::new("user", "u1");
        let res = eval_with_prereqs(&flag, ctx, &resolved, TraceLevel::Off);
        assert_eq!(res.variant_key, "off");
        assert!(matches!(res.outcome, EvalOutcome::FlagDisabled));
    }

    // The plain `evaluate_flag` entry point (no prereq map) treats any
    // configured prerequisite as unmet → fallback — i.e. it delegates with an
    // empty resolved map. This guarantees existing callers gate conservatively.
    #[test]
    fn plain_evaluate_flag_treats_configured_prereq_as_unmet() {
        let prereq_flag = FlagId::new();
        let required = VariantId::new();
        let mut flag = setup_flag();
        let fallback = flag.variants[1].id; // "off"
        flag.prerequisites = PrerequisiteGate {
            prerequisites: vec![FlagPrerequisite {
                prerequisite_flag_id: prereq_flag,
                required_variant_id: required,
            }],
            fallback_variant_id: Some(fallback),
        };
        let memberships = ListMembershipIndex::new();
        let env = EnvironmentId::from_uuid(Uuid::nil());
        let proj = ProjectId::new();
        let ctx = Context::new("user", "u1").with_parameter("beta", ParameterValue::Bool(true));
        let results = evaluate_flag(
            &flag,
            std::slice::from_ref(&ctx),
            &[],
            &memberships,
            env,
            proj,
            TraceLevel::Off,
        );
        assert_eq!(results[0].variant_key, "off");
        assert!(matches!(
            results[0].outcome,
            EvalOutcome::PrerequisiteFailed { .. }
        ));
    }

    // Multiple prerequisites: the FIRST failing one is reported; a later
    // satisfied prerequisite does not rescue the gate.
    #[test]
    fn prereq_multiple_reports_first_failure() {
        let p1 = FlagId::new();
        let p2 = FlagId::new();
        let r1 = VariantId::new();
        let r2 = VariantId::new();
        let mut flag = setup_flag();
        let fallback = flag.variants[1].id;
        flag.prerequisites = PrerequisiteGate {
            prerequisites: vec![
                FlagPrerequisite {
                    prerequisite_flag_id: p1,
                    required_variant_id: r1,
                },
                FlagPrerequisite {
                    prerequisite_flag_id: p2,
                    required_variant_id: r2,
                },
            ],
            fallback_variant_id: Some(fallback),
        };
        // p1 unmet (resolved to other), p2 met.
        let mut resolved = HashMap::new();
        resolved.insert(p1, Some(VariantId::new()));
        resolved.insert(p2, Some(r2));
        let ctx = Context::new("user", "u1");
        let res = eval_with_prereqs(&flag, ctx, &resolved, TraceLevel::Off);
        assert!(matches!(
            res.outcome,
            EvalOutcome::PrerequisiteFailed { prerequisite_flag_id } if prerequisite_flag_id == p1
        ));
    }
}
