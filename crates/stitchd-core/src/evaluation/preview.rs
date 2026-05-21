//! Flag evaluation preview — runs evaluation with full rule-trace output.
//!
//! This module is used by the admin UI's "Preview" feature to let operators
//! test flag targeting rules against sample contexts without recording events.

use serde::{Deserialize, Serialize};
use std::collections::HashSet;

use crate::context::EvaluationContext;
use crate::flag::Flag;
use crate::hashing::calculate_allocation;
use crate::id::{EnvironmentId, RuleId, SegmentId};
use crate::rule_engine::condition::Condition;
use crate::rule_engine::eval_expr::evaluate_expr;
use crate::rule_engine::eval_leaf::evaluate_leaf;
use crate::rule_engine::types::{ConditionExpr, EvaluationInput, RuleOutput, TargetField};
use crate::segment::{SegmentDefinition, SegmentEvaluator};
use crate::variants::VariantValue;

// ── Output types ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConditionTrace {
    pub predicate: String,
    pub result: bool,
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
    pub conditions: Vec<ConditionTrace>,
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
    evaluation_contexts
        .iter()
        .enumerate()
        .map(|(idx, ec)| {
            // Resolve segment membership for this specific context from rule-based definitions.
            let mut resolved_segments = resolve_segments(ec, segment_definitions);
            // Merge any pre-resolved list-segment memberships supplied by the caller.
            if let Some(extra) = pre_resolved_list_memberships.get(idx) {
                resolved_segments.extend(extra.iter().copied());
            }
            evaluate_single(flag, ec, &resolved_segments, env_id, idx)
        })
        .collect()
}

/// Resolve which segments the context belongs to by evaluating each segment definition.
fn resolve_segments(
    ec: &EvaluationContext,
    definitions: &[SegmentDefinition],
) -> HashSet<SegmentId> {
    if definitions.is_empty() {
        return HashSet::new();
    }
    match SegmentEvaluator::evaluate_all(&ec.contexts, definitions) {
        Ok(results) => results
            .into_iter()
            .filter_map(|(id, result)| if result.matched { Some(id) } else { None })
            .collect(),
        Err(_) => HashSet::new(),
    }
}

// ── Per-context evaluation ────────────────────────────────────────────────────

fn evaluate_single(
    flag: &Flag,
    ec: &EvaluationContext,
    segments: &HashSet<SegmentId>,
    env_id: EnvironmentId,
    context_index: usize,
) -> ContextPreviewResult {
    let default_variant = flag.get_default_variant();
    let (default_key, default_value) = default_variant
        .map(|v| (v.key.clone(), variant_value_to_json(&v.value)))
        .unwrap_or_else(|| ("".to_string(), serde_json::Value::Null));

    // Disabled flag: return default immediately, no rule traces.
    if !flag.record.enabled {
        return ContextPreviewResult {
            context_index,
            variant_key: default_key,
            variant_value: default_value,
            fired_rule_index: None,
            fired_rule_name: None,
            fired_rule_id: None,
            rule_traces: vec![],
            rollout_debug: None,
        };
    }

    let input = EvaluationInput {
        contexts: &ec.contexts,
        resolved_segments: segments.clone(),
        evaluated_flags: std::collections::HashMap::new(),
    };

    let rules = &flag.rules;
    let mut traces: Vec<RuleTrace> = Vec::with_capacity(rules.len());
    let mut fired_rule_index: Option<usize> = None;
    let mut fired_rule_name: Option<String> = None;
    let mut fired_rule_id: Option<RuleId> = None;
    let mut result_variant_key = default_key.clone();
    let mut result_variant_value = default_value.clone();
    let mut rollout_debug: Option<RolloutDebug> = None;

    for (i, flag_rule) in rules.iter().enumerate() {
        let rule = &flag_rule.rule;

        if fired_rule_index.is_some() {
            // A previous rule already fired — this rule is skipped.
            traces.push(RuleTrace {
                rule_index: i,
                rule_name: rule.name.clone(),
                outcome: RuleOutcome::Skipped,
                conditions: vec![],
            });
            continue;
        }

        let matched = evaluate_expr(&rule.condition, &input).unwrap_or(false);

        if matched {
            // Collect leaf-condition traces for the fired rule.
            let conditions = trace_conditions(&rule.condition, &input);

            // Resolve the variant from the rule output.
            match &rule.output {
                RuleOutput::Variant(variant_id) => {
                    if let Some(v) = flag.get_variant(*variant_id) {
                        result_variant_key = v.key.clone();
                        result_variant_value = variant_value_to_json(&v.value);
                    }
                }
                RuleOutput::Percentage { targets, weights } => {
                    let mut target_values: Vec<String> = Vec::new();
                    for t in targets {
                        let val = ec
                            .get_context(&t.context_type)
                            .and_then(|ctx| match &t.field {
                                TargetField::Key => Some(ctx.key.clone()),
                                TargetField::Parameter(name) => {
                                    ctx.parameters.get(name).map(|v| v.to_string())
                                }
                            })
                            .unwrap_or_default();
                        target_values.push(val);
                    }

                    let flag_key = flag.record.key.as_str();
                    let env_str = env_id.to_string();
                    let percentage = calculate_allocation(flag_key, &env_str, &target_values);
                    let bucket = ((percentage * 10.0).floor() as u32).min(999);

                    // Build hash_input string the same way calculate_allocation does.
                    let mut hash_input = format!("{}{}", flag_key, env_str);
                    for t in &target_values {
                        hash_input.push_str(t);
                    }

                    // Build variant ranges from cumulative weights.
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

                    // Find which variant bucket falls into.
                    let mut cumulative_weight: u32 = 0;
                    for (variant_id, weight) in weights {
                        cumulative_weight += weight;
                        if bucket < cumulative_weight {
                            if let Some(v) = flag.get_variant(*variant_id) {
                                result_variant_key = v.key.clone();
                                result_variant_value = variant_value_to_json(&v.value);
                            }
                            break;
                        }
                    }

                    rollout_debug = Some(RolloutDebug {
                        hash_input,
                        bucket,
                        variant_ranges,
                    });
                }
            }

            fired_rule_index = Some(i);
            fired_rule_name = rule.name.clone();
            fired_rule_id = Some(rule.id);

            traces.push(RuleTrace {
                rule_index: i,
                rule_name: rule.name.clone(),
                outcome: RuleOutcome::Match,
                conditions,
            });
        } else {
            // Collect per-leaf traces even for non-matching rules so the UI can
            // show which specific conditions failed (bug wub).
            let conditions = trace_conditions(&rule.condition, &input);
            traces.push(RuleTrace {
                rule_index: i,
                rule_name: rule.name.clone(),
                outcome: RuleOutcome::NoMatch,
                conditions,
            });
        }
    }

    ContextPreviewResult {
        context_index,
        variant_key: result_variant_key,
        variant_value: result_variant_value,
        fired_rule_index,
        fired_rule_name,
        fired_rule_id,
        rule_traces: traces,
        rollout_debug,
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Recursively collect leaf-condition traces from an expression tree.
fn trace_conditions(expr: &ConditionExpr, input: &EvaluationInput<'_>) -> Vec<ConditionTrace> {
    let mut out = Vec::new();
    collect_leaf_traces(expr, input, &mut out);
    out
}

fn collect_leaf_traces(
    expr: &ConditionExpr,
    input: &EvaluationInput<'_>,
    out: &mut Vec<ConditionTrace>,
) {
    match expr {
        ConditionExpr::Leaf(cond) => {
            let result = evaluate_leaf(cond, input).unwrap_or(false);
            out.push(ConditionTrace {
                predicate: condition_to_predicate(cond),
                result,
            });
        }
        ConditionExpr::And(children) | ConditionExpr::Or(children) => {
            for child in children {
                collect_leaf_traces(child, input, out);
            }
        }
        ConditionExpr::Not(inner) => {
            collect_leaf_traces(inner, input, out);
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
        assert_eq!(r.rule_traces[0].conditions.len(), 1);
        assert!(r.rule_traces[0].conditions[0].result);
        assert_eq!(
            r.rule_traces[0].conditions[0].predicate,
            "user.beta == true"
        );
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
                    weights: vec![(on_id, 500), (off_id, 500)],
                },
            },
        });

        let ec = EvaluationContext::new().with_context(Context::new("user", "u1"));
        let results = evaluate_preview(&flag, &[ec], &[], env_id(), &[]);

        let r = &results[0];
        assert!(r.rollout_debug.is_some());
        let debug = r.rollout_debug.as_ref().unwrap();
        assert!(!debug.hash_input.is_empty());
        assert!(debug.bucket < 1000);
        assert_eq!(debug.variant_ranges.len(), 2);
        assert_eq!(debug.variant_ranges[0].from, 0);
        assert_eq!(debug.variant_ranges[0].to, 499);
        assert_eq!(debug.variant_ranges[1].from, 500);
        assert_eq!(debug.variant_ranges[1].to, 999);
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

    // ── collect_leaf_traces with AND / NOT ────────────────────────────────────

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
        // Both leaf conditions should appear in traces for the fired rule.
        assert_eq!(r.rule_traces[0].conditions.len(), 2);
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
                    weights: vec![(on_id, 500), (off_id, 500)],
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

    // ── Regression: bug wub — conditions populated for non-matching rules ───────
    //
    // Before the fix, rule_traces[*].conditions was empty for no_match rules.
    // After the fix, every leaf in the rule's condition tree appears in conditions.

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

        // Both leaves must now appear in conditions even though the rule didn't match.
        assert_eq!(
            r.rule_traces[0].conditions.len(),
            2,
            "no_match rule must emit per-leaf ConditionTrace entries"
        );
        // country == US → true; beta == true → false
        let country_trace = r.rule_traces[0]
            .conditions
            .iter()
            .find(|c| c.predicate.contains("country"))
            .expect("country leaf must be traced");
        assert!(country_trace.result, "country == US should be true");

        let beta_trace = r.rule_traces[0]
            .conditions
            .iter()
            .find(|c| c.predicate.contains("beta"))
            .expect("beta leaf must be traced");
        assert!(
            !beta_trace.result,
            "beta == true should be false for this context"
        );
    }
}
