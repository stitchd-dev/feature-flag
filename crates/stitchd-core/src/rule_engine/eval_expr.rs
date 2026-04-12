use crate::rule_engine::error::RuleEngineError;
use crate::rule_engine::eval_leaf::evaluate_leaf;
use crate::rule_engine::types::{ConditionExpr, EvaluationInput};

/// Evaluate a [`ConditionExpr`] recursively against the provided [`EvaluationInput`].
///
/// # Evaluation rules
/// - `And([])` → `true`  (vacuously true)
/// - `Or([])` → `false` (vacuously false)
/// - `And` short-circuits on the first `false` child
/// - `Or` short-circuits on the first `true` child
/// - Errors propagate immediately without further evaluation
pub fn evaluate_expr(
    expr: &ConditionExpr,
    input: &EvaluationInput<'_>,
) -> Result<bool, RuleEngineError> {
    match expr {
        ConditionExpr::Leaf(cond) => evaluate_leaf(cond, input),

        ConditionExpr::And(children) => {
            for child in children {
                if !evaluate_expr(child, input)? {
                    return Ok(false);
                }
            }
            Ok(true)
        }

        ConditionExpr::Or(children) => {
            for child in children {
                if evaluate_expr(child, input)? {
                    return Ok(true);
                }
            }
            Ok(false)
        }

        ConditionExpr::Not(inner) => evaluate_expr(inner, input).map(|v| !v),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::{Context, ParameterValue};
    use crate::id::SegmentId;
    use crate::rule_engine::condition::Condition;
    use crate::rule_engine::types::{ConditionExpr, EvaluationInput};

    fn user_ctx(plan: &str) -> Context {
        Context::new("user", "u-1").with_parameter("plan", ParameterValue::Str(plan.to_string()))
    }

    fn pro_ctx() -> [Context; 1] {
        [user_ctx("pro")]
    }

    fn free_ctx() -> [Context; 1] {
        [user_ctx("free")]
    }

    fn plan_eq(plan: &str) -> ConditionExpr {
        ConditionExpr::Leaf(Condition::Eq {
            context_type: "user".into(),
            param: "plan".into(),
            value: ParameterValue::Str(plan.to_string()),
        })
    }

    // ── Vacuous cases ─────────────────────────────────────────────────────────

    #[test]
    fn and_empty_is_true() {
        let ctx: [Context; 0] = [];
        let expr = ConditionExpr::And(vec![]);
        assert_eq!(evaluate_expr(&expr, &EvaluationInput::new(&ctx)), Ok(true));
    }

    #[test]
    fn or_empty_is_false() {
        let ctx: [Context; 0] = [];
        let expr = ConditionExpr::Or(vec![]);
        assert_eq!(evaluate_expr(&expr, &EvaluationInput::new(&ctx)), Ok(false));
    }

    // ── Leaf passthrough ──────────────────────────────────────────────────────

    #[test]
    fn leaf_true() {
        let ctx = pro_ctx();
        assert_eq!(
            evaluate_expr(&plan_eq("pro"), &EvaluationInput::new(&ctx)),
            Ok(true)
        );
    }

    #[test]
    fn leaf_false() {
        let ctx = free_ctx();
        assert_eq!(
            evaluate_expr(&plan_eq("pro"), &EvaluationInput::new(&ctx)),
            Ok(false)
        );
    }

    // ── Not ───────────────────────────────────────────────────────────────────

    #[test]
    fn not_inverts_true_to_false() {
        let ctx = pro_ctx();
        let expr = ConditionExpr::Not(Box::new(plan_eq("pro")));
        assert_eq!(evaluate_expr(&expr, &EvaluationInput::new(&ctx)), Ok(false));
    }

    #[test]
    fn not_inverts_false_to_true() {
        let ctx = free_ctx();
        let expr = ConditionExpr::Not(Box::new(plan_eq("pro")));
        assert_eq!(evaluate_expr(&expr, &EvaluationInput::new(&ctx)), Ok(true));
    }

    // ── And ───────────────────────────────────────────────────────────────────

    #[test]
    fn and_all_true() {
        let ctx = [Context::new("user", "u-1")
            .with_parameter("plan", ParameterValue::Str("pro".into()))
            .with_parameter("age", ParameterValue::Int(20))];
        let expr = ConditionExpr::And(vec![
            plan_eq("pro"),
            ConditionExpr::Leaf(Condition::Gt {
                context_type: "user".into(),
                param: "age".into(),
                value: ParameterValue::Int(18),
            }),
        ]);
        assert_eq!(evaluate_expr(&expr, &EvaluationInput::new(&ctx)), Ok(true));
    }

    #[test]
    fn and_first_false_short_circuits() {
        // Second child references a missing context — if And short-circuits, no error.
        let ctx = free_ctx();
        let expr = ConditionExpr::And(vec![
            plan_eq("pro"), // false → short-circuit
            ConditionExpr::Leaf(Condition::Eq {
                context_type: "org".into(), // missing context
                param: "tier".into(),
                value: ParameterValue::Str("enterprise".into()),
            }),
        ]);
        assert_eq!(evaluate_expr(&expr, &EvaluationInput::new(&ctx)), Ok(false));
    }

    // ── Or ────────────────────────────────────────────────────────────────────

    #[test]
    fn or_first_true_short_circuits() {
        // Second child references a missing context — if Or short-circuits, no error.
        let ctx = pro_ctx();
        let expr = ConditionExpr::Or(vec![
            plan_eq("pro"), // true → short-circuit
            ConditionExpr::Leaf(Condition::Eq {
                context_type: "org".into(), // missing context
                param: "tier".into(),
                value: ParameterValue::Str("enterprise".into()),
            }),
        ]);
        assert_eq!(evaluate_expr(&expr, &EvaluationInput::new(&ctx)), Ok(true));
    }

    #[test]
    fn or_all_false_returns_false() {
        let ctx = free_ctx();
        let expr = ConditionExpr::Or(vec![plan_eq("pro"), plan_eq("enterprise")]);
        assert_eq!(evaluate_expr(&expr, &EvaluationInput::new(&ctx)), Ok(false));
    }

    // ── Compound nesting ──────────────────────────────────────────────────────

    #[test]
    fn not_and_inverts_compound() {
        // Not(And([pro, age>18])) with plan=pro, age=20 → Not(true) = false
        let ctx = [Context::new("user", "u-1")
            .with_parameter("plan", ParameterValue::Str("pro".into()))
            .with_parameter("age", ParameterValue::Int(20))];
        let expr = ConditionExpr::Not(Box::new(ConditionExpr::And(vec![
            plan_eq("pro"),
            ConditionExpr::Leaf(Condition::Gt {
                context_type: "user".into(),
                param: "age".into(),
                value: ParameterValue::Int(18),
            }),
        ])));
        assert_eq!(evaluate_expr(&expr, &EvaluationInput::new(&ctx)), Ok(false));
    }

    #[test]
    fn or_and_not_nested() {
        // Or([ And([pro, age>18]), Not(free) ])
        // plan=pro, age=20 → Or([true, true]) = true
        let ctx = [Context::new("user", "u-1")
            .with_parameter("plan", ParameterValue::Str("pro".into()))
            .with_parameter("age", ParameterValue::Int(20))];
        let expr = ConditionExpr::Or(vec![
            ConditionExpr::And(vec![
                plan_eq("pro"),
                ConditionExpr::Leaf(Condition::Gt {
                    context_type: "user".into(),
                    param: "age".into(),
                    value: ParameterValue::Int(18),
                }),
            ]),
            ConditionExpr::Not(Box::new(plan_eq("free"))),
        ]);
        assert_eq!(evaluate_expr(&expr, &EvaluationInput::new(&ctx)), Ok(true));
    }

    #[test]
    fn or_and_not_nested_all_false_branch() {
        // Or([ And([free, age>18]), Not(pro) ])
        // plan=pro, age=20 → Or([false, false]) = false
        let ctx = [Context::new("user", "u-1")
            .with_parameter("plan", ParameterValue::Str("pro".into()))
            .with_parameter("age", ParameterValue::Int(20))];
        let expr = ConditionExpr::Or(vec![
            ConditionExpr::And(vec![
                plan_eq("free"),
                ConditionExpr::Leaf(Condition::Gt {
                    context_type: "user".into(),
                    param: "age".into(),
                    value: ParameterValue::Int(18),
                }),
            ]),
            ConditionExpr::Not(Box::new(plan_eq("pro"))),
        ]);
        assert_eq!(evaluate_expr(&expr, &EvaluationInput::new(&ctx)), Ok(false));
    }

    // ── Segment inside And ────────────────────────────────────────────────────

    #[test]
    fn not_and_with_segment() {
        // Not(And([InSegment(s1), Eq("org","tier","free")]))
        // s1 not in resolved → And = false → Not = true
        let ctx: [Context; 0] = [];
        let s1 = SegmentId::new();
        let expr = ConditionExpr::Not(Box::new(ConditionExpr::And(vec![
            ConditionExpr::Leaf(Condition::InSegment(s1)),
            ConditionExpr::Leaf(Condition::Eq {
                context_type: "org".into(),
                param: "tier".into(),
                value: ParameterValue::Str("free".into()),
            }),
        ])));
        // s1 absent → And short-circuits at InSegment(false)
        assert_eq!(evaluate_expr(&expr, &EvaluationInput::new(&ctx)), Ok(true));
    }

    // ── Error propagation ─────────────────────────────────────────────────────

    #[test]
    fn error_in_and_propagates() {
        let ctx: [Context; 0] = [];
        let expr = ConditionExpr::And(vec![ConditionExpr::Leaf(Condition::Eq {
            context_type: "user".into(), // missing
            param: "plan".into(),
            value: ParameterValue::Str("pro".into()),
        })]);
        assert!(matches!(
            evaluate_expr(&expr, &EvaluationInput::new(&ctx)),
            Err(RuleEngineError::MissingContext { .. })
        ));
    }

    #[test]
    fn error_in_or_propagates() {
        let ctx: [Context; 0] = [];
        let expr = ConditionExpr::Or(vec![ConditionExpr::Leaf(Condition::Eq {
            context_type: "user".into(), // missing
            param: "plan".into(),
            value: ParameterValue::Str("pro".into()),
        })]);
        assert!(matches!(
            evaluate_expr(&expr, &EvaluationInput::new(&ctx)),
            Err(RuleEngineError::MissingContext { .. })
        ));
    }
}
