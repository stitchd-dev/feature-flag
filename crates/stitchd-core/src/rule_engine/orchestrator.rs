use crate::id::{FlagId, VariantId};
use crate::prerequisite::FlagPrerequisite;
use crate::rule_engine::dependency::{extract_flag_deps, topological_sort};
use crate::rule_engine::error::RuleEngineError;
use crate::rule_engine::eval_rules::evaluate_rules;
use crate::rule_engine::types::{EvaluationInput, Rule, RuleOutput};
use std::collections::{HashMap, HashSet};

/// Build the flag-dependency graph used for topological ordering + cycle
/// detection.
///
/// An edge `flag → dep` means `flag` depends on `dep` and therefore `dep`
/// must be evaluated first. Two edge sources are merged:
/// 1. **`FlagEvaluatedAs` rule references** — extracted from each flag's rule
///    conditions (the existing cross-flag mechanism).
/// 2. **Prerequisite edges** — each flag's prerequisite gate references the
///    prerequisite flags it depends on; those must resolve before this flag so
///    the gate can read their variants. Including them here guarantees
///    prerequisite flags are evaluated first AND that a prerequisite *cycle*
///    is detected by the same Kahn's-algorithm pass that catches rule cycles.
///
/// `flags` carries one entry per flag: `(flag_id, rules, prerequisites)`.
pub fn build_dependency_graph(
    flags: &[(FlagId, Vec<Rule>, Vec<FlagPrerequisite>)],
) -> HashMap<FlagId, HashSet<FlagId>> {
    let mut graph: HashMap<FlagId, HashSet<FlagId>> = HashMap::new();
    for (flag_id, rules, prerequisites) in flags {
        let mut deps = extract_flag_deps(rules);
        for prereq in prerequisites {
            deps.insert(prereq.prerequisite_flag_id);
        }
        graph.insert(*flag_id, deps);
    }
    graph
}

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
    // No prerequisite edges — delegate to the prerequisite-aware path with an
    // empty prerequisite list per flag so cross-flag rule deps still drive the
    // topo-sort + cycle detection exactly as before.
    let with_prereqs: Vec<(FlagId, Vec<Rule>, Vec<FlagPrerequisite>)> = flags
        .iter()
        .map(|(id, rules)| (*id, rules.clone(), Vec::new()))
        .collect();
    evaluate_flags_with_prerequisites(&with_prereqs, base_input)
}

/// Evaluate a set of flags in dependency order, accounting for BOTH
/// `FlagEvaluatedAs` cross-flag rule references AND prerequisite-gate edges.
///
/// Identical contract to [`evaluate_flags`], but each flag also carries its
/// prerequisite list. Prerequisite edges are folded into the dependency graph
/// (see [`build_dependency_graph`]) so:
/// - every prerequisite flag is resolved *before* its dependents, and
/// - a prerequisite cycle is rejected with
///   [`RuleEngineError::CyclicFlagDependency`] by the same topo-sort pass.
///
/// Returns each flag's resolved variant (`Some`) or `None` when no rule
/// matched. NOTE: this orchestrator resolves variants from *rules only* — it
/// does not itself apply the prerequisite fallback gate (that lives in
/// `evaluation::engine::evaluate_one`); its job is to produce the
/// dependency-ordered resolved-variant map that the engine's gate consumes.
pub fn evaluate_flags_with_prerequisites(
    flags: &[(FlagId, Vec<Rule>, Vec<FlagPrerequisite>)],
    base_input: &EvaluationInput<'_>,
) -> Result<HashMap<FlagId, Option<VariantId>>, RuleEngineError> {
    let flag_rules: HashMap<FlagId, &[Rule]> = flags
        .iter()
        .map(|(id, rules, _)| (*id, rules.as_slice()))
        .collect();

    let graph = build_dependency_graph(flags);

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
            name: None,
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
            name: None,
            condition: ConditionExpr::And(vec![]), // always true
            output: RuleOutput::Percentage {
                targets: vec![crate::rule_engine::types::PercentageTarget {
                    context_type: "user".into(),
                    field: crate::rule_engine::types::TargetField::Key,
                }],
                weights: vec![(v1, 5000), (v2, 5000)],
                exclusion_gate: None,
            },
        };

        let flags = [(f, vec![rule])];
        let ctx: [Context; 1] = [Context::new("user", "u-1")];
        let results = evaluate_flags(&flags, &EvaluationInput::new(&ctx)).unwrap();
        assert_eq!(results[&f], None);
    }

    // ── Prerequisite edges in the dependency graph ────────────────────────────

    use crate::prerequisite::FlagPrerequisite;

    #[test]
    fn build_dependency_graph_merges_rule_and_prerequisite_edges() {
        // Flag A's RULE references flag R (FlagEvaluatedAs); its PREREQUISITE
        // references flag P. The graph must contain both edges A→R and A→P.
        let a = FlagId::new();
        let r = FlagId::new();
        let p = FlagId::new();
        let v = VariantId::new();
        let rules_a = vec![flag_eq_rule_local(r, v)];
        let prereqs_a = vec![FlagPrerequisite {
            prerequisite_flag_id: p,
            required_variant_id: v,
        }];
        let flags = [
            (a, rules_a, prereqs_a),
            (r, vec![], vec![]),
            (p, vec![], vec![]),
        ];
        let graph = build_dependency_graph(&flags);
        let a_deps = &graph[&a];
        assert!(a_deps.contains(&r), "rule edge A→R must be present");
        assert!(a_deps.contains(&p), "prerequisite edge A→P must be present");
        assert_eq!(a_deps.len(), 2);
    }

    #[test]
    fn prerequisite_edge_orders_prerequisite_before_dependent() {
        // B depends on A only via a PREREQUISITE (no rule reference). The
        // orchestrator must still evaluate A before B — proven because B's
        // rule reads A's resolved variant via FlagEvaluatedAs and matches.
        let flag_a = FlagId::new();
        let flag_b = FlagId::new();
        let v_a = VariantId::new();
        let v_b = VariantId::new();

        let rules_a = vec![always_variant(v_a)];
        // B's rule fires only if A resolved to v_a — which requires A first.
        let rules_b = vec![variant_rule(
            ConditionExpr::Leaf(Condition::FlagEvaluatedAs {
                flag_id: flag_a,
                variant_id: v_a,
            }),
            v_b,
        )];
        let prereqs_b = vec![FlagPrerequisite {
            prerequisite_flag_id: flag_a,
            required_variant_id: v_a,
        }];

        let flags = [(flag_b, rules_b, prereqs_b), (flag_a, rules_a, vec![])];
        let ctx: [Context; 0] = [];
        let results =
            evaluate_flags_with_prerequisites(&flags, &EvaluationInput::new(&ctx)).unwrap();
        assert_eq!(results[&flag_a], Some(v_a));
        assert_eq!(results[&flag_b], Some(v_b));
    }

    #[test]
    fn prerequisite_cycle_is_detected() {
        // A prereq-> B, B prereq-> A (no rule edges at all). The cycle must be
        // rejected by the same topo-sort pass.
        let flag_a = FlagId::new();
        let flag_b = FlagId::new();
        let v = VariantId::new();
        let flags = [
            (
                flag_a,
                vec![],
                vec![FlagPrerequisite {
                    prerequisite_flag_id: flag_b,
                    required_variant_id: v,
                }],
            ),
            (
                flag_b,
                vec![],
                vec![FlagPrerequisite {
                    prerequisite_flag_id: flag_a,
                    required_variant_id: v,
                }],
            ),
        ];
        let ctx: [Context; 0] = [];
        let result = evaluate_flags_with_prerequisites(&flags, &EvaluationInput::new(&ctx));
        assert!(matches!(
            result,
            Err(RuleEngineError::CyclicFlagDependency { .. })
        ));
    }

    #[test]
    fn transitive_prerequisite_chain_resolves_in_order() {
        // C prereq-> B prereq-> A. Evaluation order must be A, B, C. Each
        // flag's rule reads its prerequisite's resolved variant to prove order.
        let a = FlagId::new();
        let b = FlagId::new();
        let c = FlagId::new();
        let v_a = VariantId::new();
        let v_b = VariantId::new();
        let v_c = VariantId::new();

        let rules_a = vec![always_variant(v_a)];
        let rules_b = vec![variant_rule(
            ConditionExpr::Leaf(Condition::FlagEvaluatedAs {
                flag_id: a,
                variant_id: v_a,
            }),
            v_b,
        )];
        let rules_c = vec![variant_rule(
            ConditionExpr::Leaf(Condition::FlagEvaluatedAs {
                flag_id: b,
                variant_id: v_b,
            }),
            v_c,
        )];

        let flags = [
            (
                c,
                rules_c,
                vec![FlagPrerequisite {
                    prerequisite_flag_id: b,
                    required_variant_id: v_b,
                }],
            ),
            (
                b,
                rules_b,
                vec![FlagPrerequisite {
                    prerequisite_flag_id: a,
                    required_variant_id: v_a,
                }],
            ),
            (a, rules_a, vec![]),
        ];
        let ctx: [Context; 0] = [];
        let results =
            evaluate_flags_with_prerequisites(&flags, &EvaluationInput::new(&ctx)).unwrap();
        assert_eq!(results[&a], Some(v_a));
        assert_eq!(results[&b], Some(v_b));
        assert_eq!(results[&c], Some(v_c));
    }

    // Local helper mirroring dependency::flag_eq_rule for the merge test.
    fn flag_eq_rule_local(flag_id: FlagId, variant_id: VariantId) -> Rule {
        variant_rule(
            ConditionExpr::Leaf(Condition::FlagEvaluatedAs {
                flag_id,
                variant_id,
            }),
            VariantId::new(),
        )
    }
}
