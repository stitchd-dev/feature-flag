//! Flag evaluation preview — runs evaluation with full rule-trace output.
//!
//! This module is used by the admin UI's "Preview" feature to let operators
//! test flag targeting rules against sample contexts without recording events.
//!
//! As of Phase 2 (flag_eval_unify_20260522), `evaluate_preview` is a thin
//! wrapper that delegates orchestration to
//! [`crate::evaluation::engine::evaluate_flag`] with [`TraceLevel::Full`].
//! All rule-iteration / percentage / default-rule-distribution logic lives
//! in `engine.rs`; this module only wires the per-`EvaluationContext` shape
//! that the flag-service preview RPC expects.

use serde::{Deserialize, Serialize};
use std::collections::HashSet;

use crate::context::EvaluationContext;
use crate::flag::Flag;
use crate::id::{EnvironmentId, RuleId, SegmentId};
use crate::rule_engine::condition::Condition;
use crate::rule_engine::eval_leaf::evaluate_leaf;
use crate::rule_engine::types::{ConditionExpr, EvaluationInput};
use crate::segment::SegmentDefinition;
use crate::variants::VariantValue;

use super::engine::evaluate_flag;
use super::types::{EvalOutcome, ListMembershipIndex, TraceLevel};

// ── Output types ──────────────────────────────────────────────────────────────

/// A node in the condition evaluation tree, mirroring the `ConditionExpr`
/// shape so the admin UI can render AND / OR / NOT groups exactly as they
/// appear in the rule builder.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ConditionNode {
    /// Terminal condition (a single predicate).
    Leaf { predicate: String, result: bool },
    /// All children must be true (short-circuits on first false).
    And {
        result: bool,
        children: Vec<ConditionNode>,
    },
    /// At least one child must be true (short-circuits on first true).
    Or {
        result: bool,
        children: Vec<ConditionNode>,
    },
    /// Negation of the single child.
    Not {
        result: bool,
        child: Box<ConditionNode>,
    },
}

impl ConditionNode {
    pub fn result(&self) -> bool {
        match self {
            Self::Leaf { result, .. }
            | Self::And { result, .. }
            | Self::Or { result, .. }
            | Self::Not { result, .. } => *result,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuleOutcome {
    Match,
    NoMatch,
    Skipped,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuleTrace {
    pub rule_index: usize,
    pub rule_name: Option<String>,
    pub outcome: RuleOutcome,
    /// Full condition tree — `None` for the catch-all (no explicit conditions).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub condition_tree: Option<ConditionNode>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VariantRange {
    pub variant_key: String,
    pub from: u32,
    pub to: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RolloutDebug {
    pub hash_input: String,
    pub bucket: u32,
    pub variant_ranges: Vec<VariantRange>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextPreviewResult {
    pub context_index: usize,
    pub variant_key: String,
    pub variant_value: serde_json::Value,
    /// `None` means the default rule fired (no targeting rule matched).
    pub fired_rule_index: Option<usize>,
    pub fired_rule_name: Option<String>,
    /// UUID of the rule that matched, or `None` for default-rule fall-through
    /// / disabled flag. Distinct from `fired_rule_index` because the index is
    /// positional within the flag's rule list (useful for the admin preview
    /// UI), whereas the ID is the stable identifier used by the Phase 4
    /// `experiment_assignments_mv` to scope exposures to the experiment's
    /// bound rule.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fired_rule_id: Option<RuleId>,
    pub rule_traces: Vec<RuleTrace>,
    pub rollout_debug: Option<RolloutDebug>,
}

// ── Main entry point ──────────────────────────────────────────────────────────

/// Evaluate `flag` against each evaluation context and return a per-context
/// result with full rule-trace breakdown.
///
/// `segment_definitions` contains the full definition for every segment
/// referenced by this flag's rules. For each context, the core resolves
/// segment membership via `SegmentEvaluator`, then runs rule evaluation.
///
/// `pre_resolved_list_memberships` supplies extra per-context segment IDs (one
/// `HashSet<SegmentId>` per evaluation context, aligned by index). These are
/// merged with the rule-based resolved segments before evaluation. Use this to
/// inject list-segment membership results obtained externally (e.g. via a
/// ScyllaDB batch check in the flag service) without having to load full
/// include/exclude sets into memory.  Pass an empty slice when not needed.
///
/// Does not record events or affect any counters — preview only.
pub fn evaluate_preview(
    flag: &Flag,
    evaluation_contexts: &[EvaluationContext],
    segment_definitions: &[SegmentDefinition],
    env_id: EnvironmentId,
    pre_resolved_list_memberships: &[HashSet<SegmentId>],
) -> Vec<ContextPreviewResult> {
    // Phase 2 of `flag_eval_unify_20260522`: delegate to the unified core
    // entry point. Each `EvaluationContext` here corresponds to one
    // independent bundle.
    //
    // Bug fix `feature-flag-utp` (cross-context hashing in preview): when
    // a single `EvaluationContext` carries N sub-contexts (the common
    // multi-context preview shape — user + device + application), we emit
    // ONE `ContextPreviewResult` PER SUB-CONTEXT, sharing the SAME bundle
    // for rule evaluation + percentage hashing. The previous behaviour
    // collapsed a multi-sub-context bundle into a single result (taking
    // only the first per-context entry from `evaluate_flag`), which
    // silently dropped the other sub-contexts from the UI's view; worse,
    // some upstream callers worked around that by splitting the incoming
    // flat list into N single-sub-context bundles, which destroyed the
    // cross-context relationship entirely (each result's `hash_input` then
    // resolved only against its own sub-context).
    //
    // The new contract: one result per sub-context, `context_index` is the
    // GLOBAL sub-context index across all input `EvaluationContext`s (so
    // gateways and callers can map results back to their flat input list
    // shape without an extra step). For a single-sub-context EC the
    // behaviour is identical to before.
    //
    // `project_id` is reserved for future hash-salt extensions; today the
    // hashing salt is `(flag_key, env_id, target_values)`, so we pass a
    // synthetic ProjectId (the flag's project).
    let project_id = flag.record.project_id;

    let mut out: Vec<ContextPreviewResult> = Vec::new();
    let mut global_idx: usize = 0;
    for (ec_idx, ec) in evaluation_contexts.iter().enumerate() {
        // Build a per-bundle ListMembershipIndex from the caller-supplied
        // pre-resolved memberships (aligned by EvaluationContext index).
        // The set applies to the WHOLE bundle for that index, so we
        // register it under EVERY (context_type, context_key) tuple in
        // the bundle — the engine's `for ctx in bundle` lookup loop will
        // then merge it into the resolved segment set regardless of
        // which context the flag rule references.
        let mut memberships = ListMembershipIndex::new();
        if let Some(extra) = pre_resolved_list_memberships.get(ec_idx)
            && !extra.is_empty()
        {
            for ctx in &ec.contexts {
                memberships.insert(ctx.context_type.clone(), ctx.key.clone(), extra.clone());
            }
        }

        if ec.contexts.is_empty() {
            // Empty bundle — emit a single empty result that mirrors the
            // disabled-flag default. Matches the pre-fix behaviour for
            // empty inputs so existing callers see no change.
            out.push(from_flag_eval_result(flag, None, global_idx));
            global_idx += 1;
            continue;
        }

        let results = evaluate_flag(
            flag,
            &ec.contexts,
            segment_definitions,
            &memberships,
            env_id,
            project_id,
            TraceLevel::Full,
        );

        // feature-flag-utp: emit ONE ContextPreviewResult per sub-context in
        // the bundle. `evaluate_flag` returns one FlagEvaluationResult per
        // sub-context (in input order), each evaluated against the FULL
        // bundle — so rule conditions and cross-context percentage hashing are
        // preserved and all results for a bundle share the same outcome.
        // `context_index` is the GLOBAL sub-context index across every input
        // EvaluationContext, so callers can map results back to their flat
        // input list. (A non-empty `ec.contexts` always yields ≥1 result; the
        // guard keeps a defensive single empty result if that ever changes.)
        if results.is_empty() {
            out.push(from_flag_eval_result(flag, None, global_idx));
            global_idx += 1;
        } else {
            for r in results {
                out.push(from_flag_eval_result(flag, Some(r), global_idx));
                global_idx += 1;
            }
        }
    }
    out
}

/// Remap a `FlagEvaluationResult` (core type) into a `ContextPreviewResult`
/// (preview RPC type) for the given evaluation-context index.
fn from_flag_eval_result(
    flag: &Flag,
    result: Option<crate::evaluation::types::FlagEvaluationResult>,
    context_index: usize,
) -> ContextPreviewResult {
    let Some(r) = result else {
        // Empty bundle — fall back to the flag's default variant payload.
        let (variant_key, variant_value) = flag
            .get_default_variant()
            .map(|v| (v.key.clone(), variant_value_to_json(&v.value)))
            .unwrap_or_else(|| (String::new(), serde_json::Value::Null));
        return ContextPreviewResult {
            context_index,
            variant_key,
            variant_value,
            fired_rule_index: None,
            fired_rule_name: None,
            fired_rule_id: None,
            rule_traces: vec![],
            rollout_debug: None,
        };
    };

    let fired_rule_index = match &r.outcome {
        EvalOutcome::RuleMatch { rule_index } => Some(*rule_index),
        _ => None,
    };
    let (fired_rule_name, fired_rule_id, rule_traces, rollout_debug) = if let Some(trace) = r.trace
    {
        (
            trace.fired_rule_name,
            trace.fired_rule_id,
            trace.rule_traces,
            trace.rollout_debug,
        )
    } else {
        (None, None, vec![], None)
    };

    ContextPreviewResult {
        context_index,
        variant_key: r.variant_key,
        variant_value: variant_value_to_json(&r.variant_value),
        fired_rule_index,
        fired_rule_name,
        fired_rule_id,
        rule_traces,
        rollout_debug,
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Recursively build a `ConditionNode` tree from a `ConditionExpr`, preserving
/// the AND / OR / NOT group structure so the admin UI can mirror it.
pub(super) fn build_condition_tree(
    expr: &ConditionExpr,
    input: &EvaluationInput<'_>,
) -> ConditionNode {
    match expr {
        ConditionExpr::Leaf(cond) => {
            let result = evaluate_leaf(cond, input).unwrap_or(false);
            ConditionNode::Leaf {
                predicate: condition_to_predicate(cond),
                result,
            }
        }
        ConditionExpr::And(children) => {
            let child_nodes: Vec<ConditionNode> = children
                .iter()
                .map(|c| build_condition_tree(c, input))
                .collect();
            let result = child_nodes.iter().all(|n| n.result());
            ConditionNode::And {
                result,
                children: child_nodes,
            }
        }
        ConditionExpr::Or(children) => {
            let child_nodes: Vec<ConditionNode> = children
                .iter()
                .map(|c| build_condition_tree(c, input))
                .collect();
            let result = child_nodes.iter().any(|n| n.result());
            ConditionNode::Or {
                result,
                children: child_nodes,
            }
        }
        ConditionExpr::Not(inner) => {
            let child = build_condition_tree(inner, input);
            let result = !child.result();
            ConditionNode::Not {
                result,
                child: Box::new(child),
            }
        }
    }
}

fn condition_to_predicate(cond: &Condition) -> String {
    match cond {
        Condition::Eq {
            context_type,
            param,
            value,
        } => {
            format!("{context_type}.{param} == {value}")
        }
        Condition::Ne {
            context_type,
            param,
            value,
        } => {
            format!("{context_type}.{param} != {value}")
        }
        Condition::Lt {
            context_type,
            param,
            value,
        } => {
            format!("{context_type}.{param} < {value}")
        }
        Condition::Lte {
            context_type,
            param,
            value,
        } => {
            format!("{context_type}.{param} <= {value}")
        }
        Condition::Gt {
            context_type,
            param,
            value,
        } => {
            format!("{context_type}.{param} > {value}")
        }
        Condition::Gte {
            context_type,
            param,
            value,
        } => {
            format!("{context_type}.{param} >= {value}")
        }
        Condition::Contains {
            context_type,
            param,
            substr,
        } => {
            format!("{context_type}.{param} contains \"{substr}\"")
        }
        Condition::StartsWith {
            context_type,
            param,
            prefix,
        } => {
            format!("{context_type}.{param} starts_with \"{prefix}\"")
        }
        Condition::EndsWith {
            context_type,
            param,
            suffix,
        } => {
            format!("{context_type}.{param} ends_with \"{suffix}\"")
        }
        Condition::SemverGte {
            context_type,
            param,
            version,
        } => {
            format!("{context_type}.{param} semver >= {version}")
        }
        Condition::SemverTilde {
            context_type,
            param,
            version,
        } => {
            format!("{context_type}.{param} semver ~{version}")
        }
        Condition::SemverCaret {
            context_type,
            param,
            version,
        } => {
            format!("{context_type}.{param} semver ^{version}")
        }
        Condition::InSegment(id) => format!("in segment {id}"),
        Condition::NotInSegment(id) => format!("not in segment {id}"),
        Condition::FlagEvaluatedAs {
            flag_id,
            variant_id,
        } => {
            format!("flag {flag_id} evaluated as {variant_id}")
        }
    }
}

fn variant_value_to_json(v: &VariantValue) -> serde_json::Value {
    match v {
        VariantValue::BoolValue(b) => serde_json::Value::Bool(*b),
        VariantValue::IntValue(i) => serde_json::json!(i),
        VariantValue::DoubleValue(d) => serde_json::json!(d),
        VariantValue::StrValue(s) => serde_json::Value::String(s.clone()),
        VariantValue::JsonValue(j) => j.clone(),
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::{Context, ParameterValue};
    use crate::flag::{Flag, FlagRecord, FlagRule, FlagValueType};
    use crate::id::{FlagId, FlagKey, ProjectId, RuleId, SegmentId, VariantId};
    use crate::rule_engine::condition::Condition;
    use crate::rule_engine::types::TargetField;
    use crate::rule_engine::types::{ConditionExpr, PercentageTarget, Rule, RuleOutput};
    use crate::variants::{Variant, VariantValue};
    use chrono::Utc;
    use uuid::Uuid;

    fn make_bool_flag(enabled: bool) -> (Flag, VariantId, VariantId) {
        let flag_id = FlagId::new();
        let project_id = ProjectId::new();
        let on_id = VariantId::new();
        let off_id = VariantId::new();

        let record = FlagRecord {
            id: flag_id,
            project_id,
            key: FlagKey::new("my-flag").unwrap(),
            name: "My Flag".to_string(),
            description: String::new(),
            value_type: FlagValueType::Bool,
            enabled,
            default_variant_id: Some(off_id),
            default_rule_distribution: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            deleted_at: None,
            version: 1,
        };

        let variants = vec![
            Variant {
                id: on_id,
                key: "on".to_string(),
                value: VariantValue::BoolValue(true),
            },
            Variant {
                id: off_id,
                key: "off".to_string(),
                value: VariantValue::BoolValue(false),
            },
        ];

        let flag = Flag {
            record,
            hashing_config: vec![],
            rules: vec![],
            variants,
        };
        (flag, on_id, off_id)
    }

    fn beta_rule(on_id: VariantId, flag_id: FlagId) -> FlagRule {
        FlagRule {
            flag_id,
            rule_index: 0,
            rule: Rule {
                id: RuleId::new(),
                name: Some("beta users".to_string()),
                condition: ConditionExpr::Leaf(Condition::Eq {
                    context_type: "user".to_string(),
                    param: "beta".to_string(),
                    value: ParameterValue::Bool(true),
                }),
                output: RuleOutput::Variant(on_id),
            },
        }
    }

    fn env_id() -> EnvironmentId {
        EnvironmentId::from_uuid(Uuid::nil())
    }

    // ── Disabled flag ─────────────────────────────────────────────────────────

    #[test]
    fn disabled_flag_returns_default_with_no_traces() {
        let (flag, _, _) = make_bool_flag(false);
        let ec = EvaluationContext::new().with_context(
            Context::new("user", "u1").with_parameter("beta", ParameterValue::Bool(true)),
        );
        let results = evaluate_preview(&flag, &[ec], &[], env_id(), &[]);

        assert_eq!(results.len(), 1);
        let r = &results[0];
        assert_eq!(r.variant_key, "off");
        assert_eq!(r.fired_rule_index, None);
        assert!(r.rule_traces.is_empty());
        assert!(r.rollout_debug.is_none());
    }

    // ── Rule match ────────────────────────────────────────────────────────────

    #[test]
    fn matching_rule_returns_correct_variant_and_trace() {
        let (mut flag, on_id, _) = make_bool_flag(true);
        let rule = beta_rule(on_id, flag.record.id);
        flag.rules.push(rule);

        let ec = EvaluationContext::new().with_context(
            Context::new("user", "u1").with_parameter("beta", ParameterValue::Bool(true)),
        );
        let results = evaluate_preview(&flag, &[ec], &[], env_id(), &[]);

        let r = &results[0];
        assert_eq!(r.variant_key, "on");
        assert_eq!(r.fired_rule_index, Some(0));
        assert_eq!(r.fired_rule_name, Some("beta users".to_string()));
        assert!(matches!(r.rule_traces[0].outcome, RuleOutcome::Match));
        let leaf = match r.rule_traces[0].condition_tree.as_ref().unwrap() {
            ConditionNode::Leaf { predicate, result } => (predicate.as_str(), *result),
            other => panic!("expected Leaf, got {:?}", other),
        };
        assert!(leaf.1, "condition result should be true");
        assert_eq!(leaf.0, "user.beta == true");
    }

    // ── No match → default variant ────────────────────────────────────────────

    #[test]
    fn no_matching_rule_returns_default_variant() {
        let (mut flag, on_id, _) = make_bool_flag(true);
        let rule = beta_rule(on_id, flag.record.id);
        flag.rules.push(rule);

        let ec = EvaluationContext::new().with_context(
            Context::new("user", "u1").with_parameter("beta", ParameterValue::Bool(false)),
        );
        let results = evaluate_preview(&flag, &[ec], &[], env_id(), &[]);

        let r = &results[0];
        assert_eq!(r.variant_key, "off");
        assert_eq!(r.fired_rule_index, None);
        assert!(r.fired_rule_id.is_none());
        assert!(matches!(r.rule_traces[0].outcome, RuleOutcome::NoMatch));
    }

    // ── fired_rule_id: surfaces matching rule UUID for eval-log attribution ──

    #[test]
    fn matching_rule_surfaces_fired_rule_id() {
        let (mut flag, on_id, _) = make_bool_flag(true);
        let rule = beta_rule(on_id, flag.record.id);
        let expected_rule_id = rule.rule.id;
        flag.rules.push(rule);

        let ec = EvaluationContext::new().with_context(
            Context::new("user", "u1").with_parameter("beta", ParameterValue::Bool(true)),
        );
        let results = evaluate_preview(&flag, &[ec], &[], env_id(), &[]);

        let r = &results[0];
        assert_eq!(r.variant_key, "on");
        assert_eq!(
            r.fired_rule_id,
            Some(expected_rule_id),
            "matched rule's UUID must be surfaced for the eval-log writer to attribute exposures"
        );
    }

    #[test]
    fn disabled_flag_has_no_fired_rule_id() {
        let (flag, _, _) = make_bool_flag(false);
        let ec = EvaluationContext::new().with_context(
            Context::new("user", "u1").with_parameter("beta", ParameterValue::Bool(true)),
        );
        let results = evaluate_preview(&flag, &[ec], &[], env_id(), &[]);
        assert!(results[0].fired_rule_id.is_none());
    }

    // ── Multiple rules: second matches; first is no_match ─────────────────────

    #[test]
    fn first_rule_no_match_second_rule_match_marks_remaining_skipped() {
        let (mut flag, on_id, _) = make_bool_flag(true);
        let flag_id = flag.record.id;
        let off2_id = VariantId::new();
        flag.variants.push(Variant {
            id: off2_id,
            key: "variant2".to_string(),
            value: VariantValue::BoolValue(false),
        });

        // Rule 0: beta users → on
        flag.rules.push(FlagRule {
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
                output: RuleOutput::Variant(on_id),
            },
        });

        // Rule 1: country == "US" → variant2
        flag.rules.push(FlagRule {
            flag_id,
            rule_index: 1,
            rule: Rule {
                id: RuleId::new(),
                name: Some("US users".to_string()),
                condition: ConditionExpr::Leaf(Condition::Eq {
                    context_type: "user".to_string(),
                    param: "country".to_string(),
                    value: ParameterValue::Str("US".to_string()),
                }),
                output: RuleOutput::Variant(off2_id),
            },
        });

        // Rule 2: always → on (should be skipped once rule 1 fires)
        flag.rules.push(FlagRule {
            flag_id,
            rule_index: 2,
            rule: Rule {
                id: RuleId::new(),
                name: None,
                condition: ConditionExpr::And(vec![]),
                output: RuleOutput::Variant(on_id),
            },
        });

        // User has country=US but beta=false → rule 0 misses, rule 1 fires, rule 2 skipped
        let ec = EvaluationContext::new().with_context(
            Context::new("user", "u1")
                .with_parameter("beta", ParameterValue::Bool(false))
                .with_parameter("country", ParameterValue::Str("US".to_string())),
        );
        let results = evaluate_preview(&flag, &[ec], &[], env_id(), &[]);

        let r = &results[0];
        assert_eq!(r.variant_key, "variant2");
        assert_eq!(r.fired_rule_index, Some(1));
        assert!(matches!(r.rule_traces[0].outcome, RuleOutcome::NoMatch));
        assert!(matches!(r.rule_traces[1].outcome, RuleOutcome::Match));
        assert!(matches!(r.rule_traces[2].outcome, RuleOutcome::Skipped));
    }

    // ── Percentage rollout → rollout_debug populated ──────────────────────────

    #[test]
    fn percentage_rollout_populates_rollout_debug() {
        let (mut flag, on_id, off_id) = make_bool_flag(true);

        flag.rules.push(FlagRule {
            flag_id: flag.record.id,
            rule_index: 0,
            rule: Rule {
                id: RuleId::new(),
                name: None,
                condition: ConditionExpr::And(vec![]),
                output: RuleOutput::Percentage {
                    targets: vec![PercentageTarget {
                        context_type: "user".to_string(),
                        field: TargetField::Key,
                    }],
                    weights: vec![(on_id, 5000), (off_id, 5000)],
                },
            },
        });

        let ec = EvaluationContext::new().with_context(Context::new("user", "u1"));
        let results = evaluate_preview(&flag, &[ec], &[], env_id(), &[]);

        let r = &results[0];
        assert!(r.rollout_debug.is_some());
        let debug = r.rollout_debug.as_ref().unwrap();
        assert!(!debug.hash_input.is_empty());
        assert!(debug.bucket < 10000);
        assert_eq!(debug.variant_ranges.len(), 2);
        assert_eq!(debug.variant_ranges[0].from, 0);
        assert_eq!(debug.variant_ranges[0].to, 4999);
        assert_eq!(debug.variant_ranges[1].from, 5000);
        assert_eq!(debug.variant_ranges[1].to, 9999);
    }

    // ── Multiple contexts → one result per context ────────────────────────────

    #[test]
    fn multiple_contexts_produce_one_result_each() {
        let (mut flag, on_id, _) = make_bool_flag(true);
        flag.rules.push(beta_rule(on_id, flag.record.id));

        let ec1 = EvaluationContext::new().with_context(
            Context::new("user", "u1").with_parameter("beta", ParameterValue::Bool(true)),
        );
        let ec2 = EvaluationContext::new().with_context(
            Context::new("user", "u2").with_parameter("beta", ParameterValue::Bool(false)),
        );

        let results = evaluate_preview(&flag, &[ec1, ec2], &[], env_id(), &[]);

        assert_eq!(results.len(), 2);
        assert_eq!(results[0].context_index, 0);
        assert_eq!(results[0].variant_key, "on");
        assert_eq!(results[1].context_index, 1);
        assert_eq!(results[1].variant_key, "off");
    }

    // ── condition_to_predicate helpers ────────────────────────────────────────

    #[test]
    fn predicate_formats_correctly() {
        assert_eq!(
            condition_to_predicate(&Condition::Eq {
                context_type: "user".to_string(),
                param: "country".to_string(),
                value: ParameterValue::Str("US".to_string()),
            }),
            "user.country == US"
        );
        assert_eq!(
            condition_to_predicate(&Condition::Contains {
                context_type: "user".to_string(),
                param: "email".to_string(),
                substr: "acme".to_string(),
            }),
            "user.email contains \"acme\""
        );
    }

    #[test]
    fn predicate_formats_all_variants() {
        let seg_id = SegmentId::from_uuid(Uuid::nil());
        let flag_id = FlagId::new();
        let variant_id = VariantId::new();

        let cases: &[(Condition, &str)] = &[
            (
                Condition::Ne {
                    context_type: "u".into(),
                    param: "x".into(),
                    value: ParameterValue::Bool(false),
                },
                "u.x != false",
            ),
            (
                Condition::Lt {
                    context_type: "u".into(),
                    param: "age".into(),
                    value: ParameterValue::Int(18),
                },
                "u.age < 18",
            ),
            (
                Condition::Lte {
                    context_type: "u".into(),
                    param: "age".into(),
                    value: ParameterValue::Int(18),
                },
                "u.age <= 18",
            ),
            (
                Condition::Gt {
                    context_type: "u".into(),
                    param: "score".into(),
                    value: ParameterValue::Int(100),
                },
                "u.score > 100",
            ),
            (
                Condition::Gte {
                    context_type: "u".into(),
                    param: "score".into(),
                    value: ParameterValue::Int(100),
                },
                "u.score >= 100",
            ),
            (
                Condition::StartsWith {
                    context_type: "u".into(),
                    param: "name".into(),
                    prefix: "Al".into(),
                },
                "u.name starts_with \"Al\"",
            ),
            (
                Condition::EndsWith {
                    context_type: "u".into(),
                    param: "name".into(),
                    suffix: "son".into(),
                },
                "u.name ends_with \"son\"",
            ),
            (
                Condition::SemverGte {
                    context_type: "app".into(),
                    param: "version".into(),
                    version: "2.0.0".into(),
                },
                "app.version semver >= 2.0.0",
            ),
            (
                Condition::SemverTilde {
                    context_type: "app".into(),
                    param: "version".into(),
                    version: "1.2.0".into(),
                },
                "app.version semver ~1.2.0",
            ),
            (
                Condition::SemverCaret {
                    context_type: "app".into(),
                    param: "version".into(),
                    version: "1.0.0".into(),
                },
                "app.version semver ^1.0.0",
            ),
            (
                Condition::NotInSegment(seg_id),
                &format!("not in segment {seg_id}"),
            ),
        ];
        for (cond, expected) in cases {
            assert_eq!(
                &condition_to_predicate(cond),
                expected,
                "failed for {expected}"
            );
        }
        // InSegment and FlagEvaluatedAs
        assert!(condition_to_predicate(&Condition::InSegment(seg_id)).starts_with("in segment "));
        assert!(
            condition_to_predicate(&Condition::FlagEvaluatedAs {
                flag_id,
                variant_id
            })
            .starts_with("flag ")
        );
    }

    // ── segment resolution ────────────────────────────────────────────────────

    #[test]
    fn list_segment_resolution_gates_in_segment_rule() {
        use crate::segment::{ContextList, ListBasedSegment, SegmentDefinition};
        use std::collections::HashMap;

        let (mut flag, on_id, _) = make_bool_flag(true);
        let seg_id = SegmentId::new();

        flag.rules.push(FlagRule {
            flag_id: flag.record.id,
            rule_index: 0,
            rule: Rule {
                id: RuleId::new(),
                name: None,
                condition: ConditionExpr::Leaf(Condition::InSegment(seg_id)),
                output: RuleOutput::Variant(on_id),
            },
        });

        let mut lists = HashMap::new();
        lists.insert(
            "user".to_string(),
            ContextList {
                include: ["u1".to_string()].into_iter().collect(),
                exclude: Default::default(),
            },
        );
        let segment_def = SegmentDefinition::ListBased(ListBasedSegment { id: seg_id, lists });

        // u1 is in segment → rule fires → "on"
        let ec_in = EvaluationContext::new().with_context(Context::new("user", "u1"));
        let results = evaluate_preview(
            &flag,
            &[ec_in],
            std::slice::from_ref(&segment_def),
            env_id(),
            &[],
        );
        assert_eq!(results[0].variant_key, "on");

        // u2 is NOT in segment → default "off"
        let ec_out = EvaluationContext::new().with_context(Context::new("user", "u2"));
        let results = evaluate_preview(&flag, &[ec_out], &[segment_def], env_id(), &[]);
        assert_eq!(results[0].variant_key, "off");
    }

    // ── build_condition_tree with AND / NOT ───────────────────────────────────

    #[test]
    fn trace_conditions_handles_and_and_not_expressions() {
        let (mut flag, on_id, _) = make_bool_flag(true);
        // Rule: AND(beta == true, NOT(country == "DE"))
        flag.rules.push(FlagRule {
            flag_id: flag.record.id,
            rule_index: 0,
            rule: Rule {
                id: RuleId::new(),
                name: None,
                condition: ConditionExpr::And(vec![
                    ConditionExpr::Leaf(Condition::Eq {
                        context_type: "user".to_string(),
                        param: "beta".to_string(),
                        value: ParameterValue::Bool(true),
                    }),
                    ConditionExpr::Not(Box::new(ConditionExpr::Leaf(Condition::Eq {
                        context_type: "user".to_string(),
                        param: "country".to_string(),
                        value: ParameterValue::Str("DE".to_string()),
                    }))),
                ]),
                output: RuleOutput::Variant(on_id),
            },
        });

        let ec = EvaluationContext::new().with_context(
            Context::new("user", "u1")
                .with_parameter("beta", ParameterValue::Bool(true))
                .with_parameter("country", ParameterValue::Str("US".to_string())),
        );
        let results = evaluate_preview(&flag, &[ec], &[], env_id(), &[]);
        let r = &results[0];
        assert_eq!(r.variant_key, "on");
        // The AND group should contain both leaf conditions.
        let children = match r.rule_traces[0].condition_tree.as_ref().unwrap() {
            ConditionNode::And { children, .. } => children,
            other => panic!("expected And node, got {:?}", other),
        };
        assert_eq!(children.len(), 2, "AND group must have two children");
    }

    // ── percentage rollout with Parameter hashing field ───────────────────────

    #[test]
    fn percentage_rollout_with_parameter_field() {
        let (mut flag, on_id, off_id) = make_bool_flag(true);
        flag.rules.push(FlagRule {
            flag_id: flag.record.id,
            rule_index: 0,
            rule: Rule {
                id: RuleId::new(),
                name: None,
                condition: ConditionExpr::And(vec![]),
                output: RuleOutput::Percentage {
                    targets: vec![PercentageTarget {
                        context_type: "user".to_string(),
                        field: TargetField::Parameter("account_id".to_string()),
                    }],
                    weights: vec![(on_id, 5000), (off_id, 5000)],
                },
            },
        });

        let ec = EvaluationContext::new().with_context(
            Context::new("user", "u1")
                .with_parameter("account_id", ParameterValue::Str("acct-123".to_string())),
        );
        let results = evaluate_preview(&flag, &[ec], &[], env_id(), &[]);
        let r = &results[0];
        assert!(r.rollout_debug.is_some());
        let debug = r.rollout_debug.as_ref().unwrap();
        assert!(debug.hash_input.contains("acct-123"));
    }

    // ── Regression: bug 1hu — list-segment via pre_resolved_list_memberships ───
    //
    // Tests the path used by the flag service, where list-segment membership is
    // resolved externally (via ScyllaDB) and injected as pre-resolved IDs rather
    // than loading full include/exclude sets into ListBasedSegment definitions.

    #[test]
    fn list_segment_via_pre_resolved_memberships_matches() {
        let (mut flag, on_id, _) = make_bool_flag(true);
        let seg_id = SegmentId::new();

        flag.rules.push(FlagRule {
            flag_id: flag.record.id,
            rule_index: 0,
            rule: Rule {
                id: RuleId::new(),
                name: None,
                condition: ConditionExpr::Leaf(Condition::InSegment(seg_id)),
                output: RuleOutput::Variant(on_id),
            },
        });

        // Pre-resolved: context 0 is in the segment, context 1 is not.
        let pre_resolved_in: HashSet<SegmentId> = [seg_id].into_iter().collect();
        let pre_resolved_out: HashSet<SegmentId> = HashSet::new();

        let ec_in = EvaluationContext::new().with_context(Context::new("user", "alice@acme.com"));
        let ec_out = EvaluationContext::new().with_context(Context::new("user", "spam@acme.com"));

        // No segment_definitions supplied — membership comes entirely from pre_resolved.
        let results = evaluate_preview(
            &flag,
            &[ec_in, ec_out],
            &[], // no in-process definitions
            env_id(),
            &[pre_resolved_in, pre_resolved_out],
        );

        assert_eq!(
            results[0].variant_key, "on",
            "alice (in segment) must match rule 0"
        );
        assert_eq!(results[0].fired_rule_index, Some(0));
        assert_eq!(
            results[1].variant_key, "off",
            "spam (not in segment) must fall through to default"
        );
        assert_eq!(results[1].fired_rule_index, None);
    }

    // ── Regression: bug wub — condition_tree populated for non-matching rules ────
    //
    // Before the fix, rule_traces[*].condition_tree was None for no_match rules.
    // After the fix, every rule emits its full condition tree regardless of outcome.

    #[test]
    fn no_match_rule_conditions_are_populated() {
        let (mut flag, on_id, _) = make_bool_flag(true);

        // Rule 0: AND(country == "US", beta == true) → on
        // Context: country == "US", beta == false  → rule should NOT match.
        flag.rules.push(FlagRule {
            flag_id: flag.record.id,
            rule_index: 0,
            rule: Rule {
                id: RuleId::new(),
                name: Some("us-beta".to_string()),
                condition: ConditionExpr::And(vec![
                    ConditionExpr::Leaf(Condition::Eq {
                        context_type: "user".to_string(),
                        param: "country".to_string(),
                        value: ParameterValue::Str("US".to_string()),
                    }),
                    ConditionExpr::Leaf(Condition::Eq {
                        context_type: "user".to_string(),
                        param: "beta".to_string(),
                        value: ParameterValue::Bool(true),
                    }),
                ]),
                output: RuleOutput::Variant(on_id),
            },
        });

        let ec = EvaluationContext::new().with_context(
            Context::new("user", "u1")
                .with_parameter("country", ParameterValue::Str("US".to_string()))
                .with_parameter("beta", ParameterValue::Bool(false)),
        );
        let results = evaluate_preview(&flag, &[ec], &[], env_id(), &[]);

        let r = &results[0];
        assert_eq!(r.variant_key, "off", "rule should not match");
        assert!(matches!(r.rule_traces[0].outcome, RuleOutcome::NoMatch));

        // The condition_tree must be present even though the rule didn't match.
        let children = match r.rule_traces[0].condition_tree.as_ref().unwrap() {
            ConditionNode::And { children, .. } => children,
            other => panic!("expected And node, got {:?}", other),
        };
        assert_eq!(children.len(), 2, "AND group must have two leaf children");

        // country == US → true; beta == true → false
        let find_leaf = |pred: &str| -> (bool, String) {
            children
                .iter()
                .find_map(|n| {
                    if let ConditionNode::Leaf { predicate, result } = n
                        && predicate.contains(pred)
                    {
                        return Some((*result, predicate.clone()));
                    }
                    None
                })
                .unwrap_or_else(|| panic!("{pred} leaf not found"))
        };
        let (country_result, _) = find_leaf("country");
        assert!(country_result, "country == US should be true");
        let (beta_result, _) = find_leaf("beta");
        assert!(
            !beta_result,
            "beta == true should be false for this context"
        );
    }

    // ── Phase 2 Task 2.2: default_rule_distribution in preview ──────────────

    use crate::rollout::{RolloutAllocation, RolloutDistribution};

    #[test]
    fn default_rule_distribution_assigns_via_hashing_in_preview() {
        // No rule on the flag → default-rule distribution fires.
        let (mut flag, _, _) = make_bool_flag(true);
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

        // 50/50 over 1000 distinct users should produce a balanced split.
        let evaluation_contexts: Vec<EvaluationContext> = (0..1000)
            .map(|i| {
                EvaluationContext::new()
                    .with_context(Context::new("user", format!("u{i}").as_str()))
            })
            .collect();
        let results = evaluate_preview(&flag, &evaluation_contexts, &[], env_id(), &[]);
        let on_count = results.iter().filter(|r| r.variant_key == "on").count();
        assert!(
            (450..=550).contains(&on_count),
            "default-rule 50/50 distribution should produce ~500 ON; got {on_count}"
        );

        // Preview must emit rollout_debug for transparency.
        assert!(
            results[0].rollout_debug.is_some(),
            "default-rule path must populate rollout_debug in preview"
        );
        // No rule fired.
        assert_eq!(results[0].fired_rule_index, None);
        assert!(results[0].fired_rule_id.is_none());
    }

    #[test]
    fn default_rule_distribution_none_falls_to_default_variant_in_preview() {
        let (flag, _, _) = make_bool_flag(true);
        // default_rule_distribution is None (set by make_bool_flag).
        let ec = EvaluationContext::new().with_context(Context::new("user", "alice"));
        let results = evaluate_preview(&flag, &[ec], &[], env_id(), &[]);
        assert_eq!(results[0].variant_key, "off"); // default variant
        assert!(results[0].rollout_debug.is_none());
    }

    #[test]
    fn default_rule_distribution_unknown_variant_falls_back_in_preview() {
        let (mut flag, _, _) = make_bool_flag(true);
        flag.record.default_rule_distribution = Some(RolloutDistribution {
            allocations: vec![RolloutAllocation {
                variant_key: "nonexistent".to_string(),
                percentage_bp: 10000,
            }],
        });

        let ec = EvaluationContext::new().with_context(Context::new("user", "alice"));
        let results = evaluate_preview(&flag, &[ec], &[], env_id(), &[]);
        // Falls back to default variant ("off") because variant_key isn't found.
        assert_eq!(results[0].variant_key, "off");
        // rollout_debug is still populated (the hash was computed, just the
        // mapping fell through) so the UI can show what would have happened.
        assert!(results[0].rollout_debug.is_some());
    }

    #[test]
    fn matching_rule_short_circuits_default_rule_distribution_in_preview() {
        let (mut flag, on_id, _) = make_bool_flag(true);
        // Custom rule that matches → must win even when distribution is set.
        flag.rules.push(beta_rule(on_id, flag.record.id));
        // Set a distribution that would otherwise route everything to "off".
        flag.record.default_rule_distribution = Some(RolloutDistribution {
            allocations: vec![RolloutAllocation {
                variant_key: "off".to_string(),
                percentage_bp: 10000,
            }],
        });

        let ec = EvaluationContext::new().with_context(
            Context::new("user", "u1").with_parameter("beta", ParameterValue::Bool(true)),
        );
        let results = evaluate_preview(&flag, &[ec], &[], env_id(), &[]);
        assert_eq!(results[0].variant_key, "on");
        assert_eq!(results[0].fired_rule_index, Some(0));
    }

    #[test]
    fn disabled_flag_short_circuits_default_rule_distribution_in_preview() {
        let (mut flag, _, _) = make_bool_flag(false);
        flag.record.default_rule_distribution = Some(RolloutDistribution {
            allocations: vec![RolloutAllocation {
                variant_key: "on".to_string(),
                percentage_bp: 10000,
            }],
        });
        let ec = EvaluationContext::new().with_context(Context::new("user", "alice"));
        let results = evaluate_preview(&flag, &[ec], &[], env_id(), &[]);
        // Even though the distribution would route all traffic to "on",
        // the disabled-flag branch serves the default variant ("off") and
        // never touches the distribution.
        assert_eq!(results[0].variant_key, "off");
        assert!(results[0].rollout_debug.is_none());
    }
}
