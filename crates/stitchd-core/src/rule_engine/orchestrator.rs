use crate::id::{FlagId, VariantId};
use crate::rule_engine::dependency::{extract_flag_deps, topological_sort};
use crate::rule_engine::error::RuleEngineError;
use crate::rule_engine::eval_rules::evaluate_rules;
use crate::rule_engine::types::{EvaluationInput, Rule, RuleOutput};
use std::collections::{HashMap, HashSet};

/// Evaluate a set of flags in dependency order, accumulating results.
///
/// # Arguments
/// - `flags` — `(flag_id, rules)` pairs for every flag to evaluate
/// - `base_input` — shared contexts and pre-resolved segments; `evaluated_flags`
///   is ignored (the orchestrator builds its own from each flag's output)
///
/// Returns a map from every flag ID to `Some(variant_id)` if a rule matched,
/// or `None` if no rule matched (caller applies the flag's default variant).
pub fn evaluate_flags(
    flags: &[(FlagId, Vec<Rule>)],
    base_input: &EvaluationInput<'_>,
) -> Result<HashMap<FlagId, Option<VariantId>>, RuleEngineError> {
    // Build dependency graph: flag_id → set of flag_ids it depends on.
    let mut graph: HashMap<FlagId, HashSet<FlagId>> = HashMap::new();
    let flag_rules: HashMap<FlagId, &[Rule]> = flags
        .iter()
        .map(|(id, rules)| (*id, rules.as_slice()))
        .collect();

    for (flag_id, rules) in flags {
        let deps = extract_flag_deps(rules);
        graph.insert(*flag_id, deps);
    }

    let order = topological_sort(&graph)?;

    // Accumulate evaluated results; each flag can see all previously resolved flags.
    let mut results: HashMap<FlagId, Option<VariantId>> = HashMap::new();

    for flag_id in order {
        // Flags from the graph that aren't in flag_rules are transitive deps only
        // (referenced in conditions but not themselves in the evaluation set).
        let rules = match flag_rules.get(&flag_id) {
            Some(r) => *r,
            None => continue,
        };

        // Build per-flag input: inherit contexts + segments, inject accumulated results.
        let mut evaluated_flags: HashMap<FlagId, VariantId> = HashMap::new();
        for (id, opt) in &results {
            if let Some(variant) = opt {
                evaluated_flags.insert(*id, *variant);
            }
        }

        let input = EvaluationInput {
            contexts: base_input.contexts,
            resolved_segments: base_input.resolved_segments.clone(),
            evaluated_flags,
        };

        let output = evaluate_rules(rules, &input)?;
        let variant = match output {
            Some(RuleOutput::Variant(v)) => Some(*v),
            Some(RuleOutput::Percentage { .. }) => {
                // Percentage allocation in multi-flag context requires flag_key / project_id /
                // environment_id which are not available here. The orchestrator evaluates
                // simple Variant outputs; callers needing percentage must invoke
                // allocate_percentage() directly after rule matching.
                None
            }
            None => None,
        };

        results.insert(flag_id, variant);
    }

    Ok(results)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::{Context, ParameterValue};
    use crate::id::{FlagId, RuleId, VariantId};
    use crate::rule_engine::condition::Condition;
    use crate::rule_engine::types::{ConditionExpr, Rule, RuleOutput};

    fn variant_rule(cond: ConditionExpr, variant: VariantId) -> Rule {
        Rule {
            id: RuleId::new(),
            condition: cond,
            output: RuleOutput::Variant(variant),
        }
    }

    fn always_variant(variant: VariantId) -> Rule {
        variant_rule(ConditionExpr::And(vec![]), variant)
    }

    fn user_ctx(plan: &str) -> [Context; 1] {
        [Context::new("user", "u-1").with_parameter("plan", ParameterValue::Str(plan.into()))]
    }

    // ── Single flag, no cross-deps ────────────────────────────────────────────

    #[test]
    fn single_flag_variant_match() {
        let f = FlagId::new();
        let v = VariantId::new();
        let flags = [(f, vec![always_variant(v)])];
        let ctx = user_ctx("pro");
        let results = evaluate_flags(&flags, &EvaluationInput::new(&ctx)).unwrap();
        assert_eq!(results[&f], Some(v));
    }

    #[test]
    fn single_flag_no_match_returns_none() {
        let f = FlagId::new();
        let v = VariantId::new();
        let rule = variant_rule(
            ConditionExpr::Leaf(Condition::Eq {
                context_type: "user".into(),
                param: "plan".into(),
                value: ParameterValue::Str("free".into()),
            }),
            v,
        );
        let flags = [(f, vec![rule])];
        let ctx = user_ctx("pro");
        let results = evaluate_flags(&flags, &EvaluationInput::new(&ctx)).unwrap();
        assert_eq!(results[&f], None);
    }

    // ── Cross-flag resolution ─────────────────────────────────────────────────

    #[test]
    fn cross_flag_evaluates_in_order() {
        // Flag B depends on Flag A.  B's condition: FlagEvaluatedAs(A, v_a).
        let flag_a = FlagId::new();
        let flag_b = FlagId::new();
        let v_a = VariantId::new();
        let v_b = VariantId::new();

        let rules_a = vec![always_variant(v_a)];
        let rules_b = vec![variant_rule(
            ConditionExpr::Leaf(Condition::FlagEvaluatedAs {
                flag_id: flag_a,
                variant_id: v_a,
            }),
            v_b,
        )];

        let flags = [(flag_a, rules_a), (flag_b, rules_b)];
        let ctx: [Context; 0] = [];
        let results = evaluate_flags(&flags, &EvaluationInput::new(&ctx)).unwrap();

        assert_eq!(results[&flag_a], Some(v_a));
        assert_eq!(results[&flag_b], Some(v_b));
    }

    #[test]
    fn cross_flag_unresolved_flag_means_no_match() {
        // Flag A has no matching rule → None.
        // Flag B expects A = v_a, but A resolved to None → B's condition false.
        let flag_a = FlagId::new();
        let flag_b = FlagId::new();
        let v_a = VariantId::new();
        let v_b = VariantId::new();

        // A's rule only fires for "free" plan; we'll provide "pro".
        let rules_a = vec![variant_rule(
            ConditionExpr::Leaf(Condition::Eq {
                context_type: "user".into(),
                param: "plan".into(),
                value: ParameterValue::Str("free".into()),
            }),
            v_a,
        )];
        let rules_b = vec![variant_rule(
            ConditionExpr::Leaf(Condition::FlagEvaluatedAs {
                flag_id: flag_a,
                variant_id: v_a,
            }),
            v_b,
        )];

        let flags = [(flag_a, rules_a), (flag_b, rules_b)];
        let ctx = user_ctx("pro");
        let results = evaluate_flags(&flags, &EvaluationInput::new(&ctx)).unwrap();

        assert_eq!(results[&flag_a], None);
        assert_eq!(results[&flag_b], None);
    }

    // ── Cycle detection ───────────────────────────────────────────────────────

    #[test]
    fn cycle_returns_cyclic_error() {
        let flag_a = FlagId::new();
        let flag_b = FlagId::new();
        let v = VariantId::new();

        // A depends on B, B depends on A.
        let rules_a = vec![variant_rule(
            ConditionExpr::Leaf(Condition::FlagEvaluatedAs {
                flag_id: flag_b,
                variant_id: v,
            }),
            v,
        )];
        let rules_b = vec![variant_rule(
            ConditionExpr::Leaf(Condition::FlagEvaluatedAs {
                flag_id: flag_a,
                variant_id: v,
            }),
            v,
        )];

        let flags = [(flag_a, rules_a), (flag_b, rules_b)];
        let ctx: [Context; 0] = [];
        let result = evaluate_flags(&flags, &EvaluationInput::new(&ctx));
        assert!(matches!(
            result,
            Err(RuleEngineError::CyclicFlagDependency { .. })
        ));
    }

    // ── Transitive dep not in flag_rules (orchestrator line 43) ──────────────

    #[test]
    fn transitive_dep_not_in_evaluation_set_is_skipped() {
        // flag_b references flag_ext (external), but flag_ext has no rules in our set.
        // flag_ext should appear in topo order but be skipped (continue branch).
        let flag_ext = FlagId::new();
        let flag_b = FlagId::new();
        let v_b = VariantId::new();

        // flag_b fires if flag_ext evaluated as v_ext — but flag_ext is not in flags.
        // So flag_ext resolves to None, flag_b condition is false → None.
        let v_ext = VariantId::new();
        let rules_b = vec![variant_rule(
            ConditionExpr::Leaf(Condition::FlagEvaluatedAs {
                flag_id: flag_ext,
                variant_id: v_ext,
            }),
            v_b,
        )];

        let flags = [(flag_b, rules_b)];
        let ctx: [Context; 0] = [];
        let results = evaluate_flags(&flags, &EvaluationInput::new(&ctx)).unwrap();
        // flag_b has no match because flag_ext was never evaluated
        assert_eq!(results[&flag_b], None);
    }

    // ── Percentage arm in orchestrator (orchestrator line 68) ─────────────────

    #[test]
    fn percentage_rule_output_returns_none_in_orchestrator() {
        use crate::rule_engine::types::RuleOutput;

        let f = FlagId::new();
        let v1 = VariantId::new();
        let v2 = VariantId::new();

        // A rule with Percentage output — orchestrator maps this to None.
        let rule = Rule {
            id: crate::id::RuleId::new(),
            condition: ConditionExpr::And(vec![]), // always true
            output: RuleOutput::Percentage {
                targets: vec![crate::rule_engine::types::PercentageTarget {
                    context_type: "user".into(),
                    field: crate::rule_engine::types::TargetField::Key,
                }],
                weights: vec![(v1, 500), (v2, 500)],
            },
        };

        let flags = [(f, vec![rule])];
        let ctx: [Context; 1] = [Context::new("user", "u-1")];
        let results = evaluate_flags(&flags, &EvaluationInput::new(&ctx)).unwrap();
        assert_eq!(results[&f], None);
    }
}
