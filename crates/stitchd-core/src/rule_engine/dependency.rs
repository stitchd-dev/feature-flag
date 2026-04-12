use crate::id::FlagId;
use crate::rule_engine::condition::Condition;
use crate::rule_engine::error::RuleEngineError;
use crate::rule_engine::types::{ConditionExpr, Rule};
use std::collections::{HashMap, HashSet, VecDeque};

// ── Dependency graph extraction ───────────────────────────────────────────────

/// Recursively walk a [`ConditionExpr`] tree and collect all [`FlagId`]s
/// referenced by [`Condition::FlagEvaluatedAs`] leaves.
pub fn extract_flag_deps(rules: &[Rule]) -> HashSet<FlagId> {
    let mut deps = HashSet::new();
    for rule in rules {
        collect_deps_expr(&rule.condition, &mut deps);
    }
    deps
}

fn collect_deps_expr(expr: &ConditionExpr, out: &mut HashSet<FlagId>) {
    match expr {
        ConditionExpr::Leaf(Condition::FlagEvaluatedAs { flag_id, .. }) => {
            out.insert(*flag_id);
        }
        ConditionExpr::Leaf(_) => {}
        ConditionExpr::And(children) | ConditionExpr::Or(children) => {
            for child in children {
                collect_deps_expr(child, out);
            }
        }
        ConditionExpr::Not(inner) => collect_deps_expr(inner, out),
    }
}

// ── Kahn's topological sort ───────────────────────────────────────────────────

/// Topologically sort a directed dependency graph using Kahn's algorithm.
///
/// `graph` maps each [`FlagId`] to the set of flags it **depends on** (i.e.
/// must be evaluated before it). A flag with no dependencies is a root node.
///
/// Returns flags in evaluation order (dependencies before dependents), or
/// [`RuleEngineError::CyclicFlagDependency`] if a cycle is detected.
pub fn topological_sort(
    graph: &HashMap<FlagId, HashSet<FlagId>>,
) -> Result<Vec<FlagId>, RuleEngineError> {
    // in_degree[node] = number of flags node depends on (i.e. must come before it).
    // reverse[dep]    = list of nodes that depend on dep.
    let mut in_degree: HashMap<FlagId, usize> = HashMap::new();
    let mut reverse: HashMap<FlagId, Vec<FlagId>> = HashMap::new();

    // Ensure every node in the graph has an entry, even nodes with no deps.
    for &node in graph.keys() {
        in_degree.entry(node).or_insert(0);
    }
    for (&node, deps) in graph {
        *in_degree.entry(node).or_insert(0) = deps.len();
        for &dep in deps {
            // Deps referenced in conditions may not be graph roots themselves.
            in_degree.entry(dep).or_insert(0);
            reverse.entry(dep).or_default().push(node);
        }
    }

    // Seed queue with nodes that have no dependencies.
    let mut queue: VecDeque<FlagId> = in_degree
        .iter()
        .filter(|(_, d)| **d == 0)
        .map(|(&n, _)| n)
        .collect();

    let mut order = Vec::with_capacity(in_degree.len());

    while let Some(node) = queue.pop_front() {
        order.push(node);
        if let Some(dependents) = reverse.get(&node) {
            for &dep in dependents {
                let count = in_degree.get_mut(&dep).unwrap();
                *count -= 1;
                if *count == 0 {
                    queue.push_back(dep);
                }
            }
        }
    }

    if order.len() == in_degree.len() {
        Ok(order)
    } else {
        // Remaining nodes with in_degree > 0 are part of a cycle.
        let involved: Vec<FlagId> = in_degree
            .into_iter()
            .filter(|(_, d)| *d > 0)
            .map(|(n, _)| n)
            .collect();
        Err(RuleEngineError::CyclicFlagDependency { involved })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::id::{FlagId, RuleId, VariantId};
    use crate::rule_engine::condition::Condition;
    use crate::rule_engine::types::{ConditionExpr, Rule, RuleOutput};

    fn flag_eq_rule(flag_id: FlagId, variant_id: VariantId) -> Rule {
        Rule {
            id: RuleId::new(),
            condition: ConditionExpr::Leaf(Condition::FlagEvaluatedAs {
                flag_id,
                variant_id,
            }),
            output: RuleOutput::Variant(VariantId::new()),
        }
    }

    fn no_dep_rule() -> Rule {
        Rule {
            id: RuleId::new(),
            condition: ConditionExpr::And(vec![]),
            output: RuleOutput::Variant(VariantId::new()),
        }
    }

    // ── extract_flag_deps ─────────────────────────────────────────────────────

    #[test]
    fn no_deps_returns_empty() {
        let rules = [no_dep_rule()];
        assert!(extract_flag_deps(&rules).is_empty());
    }

    #[test]
    fn single_dep_extracted() {
        let f = FlagId::new();
        let v = VariantId::new();
        let rules = [flag_eq_rule(f, v)];
        let deps = extract_flag_deps(&rules);
        assert_eq!(deps, HashSet::from([f]));
    }

    #[test]
    fn multiple_deps_from_nested_expr() {
        let f1 = FlagId::new();
        let f2 = FlagId::new();
        let v = VariantId::new();
        let rule = Rule {
            id: RuleId::new(),
            condition: ConditionExpr::And(vec![
                ConditionExpr::Leaf(Condition::FlagEvaluatedAs {
                    flag_id: f1,
                    variant_id: v,
                }),
                ConditionExpr::Not(Box::new(ConditionExpr::Leaf(Condition::FlagEvaluatedAs {
                    flag_id: f2,
                    variant_id: v,
                }))),
            ]),
            output: RuleOutput::Variant(v),
        };
        let deps = extract_flag_deps(&[rule]);
        assert_eq!(deps, HashSet::from([f1, f2]));
    }

    // ── topological_sort ──────────────────────────────────────────────────────

    #[test]
    fn single_node_no_deps() {
        let a = FlagId::new();
        let graph = HashMap::from([(a, HashSet::new())]);
        let order = topological_sort(&graph).unwrap();
        assert_eq!(order, vec![a]);
    }

    #[test]
    fn linear_chain_a_depends_on_b() {
        // A → B means A depends on B, so order should have B before A.
        let a = FlagId::new();
        let b = FlagId::new();
        let graph = HashMap::from([(a, HashSet::from([b])), (b, HashSet::new())]);
        let order = topological_sort(&graph).unwrap();
        let pos_a = order.iter().position(|&x| x == a).unwrap();
        let pos_b = order.iter().position(|&x| x == b).unwrap();
        assert!(pos_b < pos_a, "B must be evaluated before A");
    }

    #[test]
    fn diamond_dependency() {
        // D depends on B and C; B and C both depend on A.
        let a = FlagId::new();
        let b = FlagId::new();
        let c = FlagId::new();
        let d = FlagId::new();
        let graph = HashMap::from([
            (a, HashSet::new()),
            (b, HashSet::from([a])),
            (c, HashSet::from([a])),
            (d, HashSet::from([b, c])),
        ]);
        let order = topological_sort(&graph).unwrap();
        assert_eq!(order.len(), 4);
        let pos = |id: FlagId| order.iter().position(|&x| x == id).unwrap();
        assert!(pos(a) < pos(b));
        assert!(pos(a) < pos(c));
        assert!(pos(b) < pos(d));
        assert!(pos(c) < pos(d));
    }

    #[test]
    fn disconnected_graph() {
        let a = FlagId::new();
        let b = FlagId::new();
        let graph = HashMap::from([(a, HashSet::new()), (b, HashSet::new())]);
        let order = topological_sort(&graph).unwrap();
        assert_eq!(order.len(), 2);
        assert!(order.contains(&a));
        assert!(order.contains(&b));
    }

    #[test]
    fn direct_cycle_returns_error() {
        let a = FlagId::new();
        let b = FlagId::new();
        let graph = HashMap::from([(a, HashSet::from([b])), (b, HashSet::from([a]))]);
        let result = topological_sort(&graph);
        assert!(matches!(
            result,
            Err(RuleEngineError::CyclicFlagDependency { .. })
        ));
        if let Err(RuleEngineError::CyclicFlagDependency { involved }) = result {
            assert!(involved.contains(&a) || involved.contains(&b));
        }
    }

    #[test]
    fn indirect_cycle_returns_error() {
        let a = FlagId::new();
        let b = FlagId::new();
        let c = FlagId::new();
        // A → B → C → A (cycle).
        let graph = HashMap::from([
            (a, HashSet::from([b])),
            (b, HashSet::from([c])),
            (c, HashSet::from([a])),
        ]);
        let result = topological_sort(&graph);
        assert!(matches!(
            result,
            Err(RuleEngineError::CyclicFlagDependency { involved }) if involved.len() >= 2
        ));
    }
}
