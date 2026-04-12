use crate::rule_engine::error::RuleEngineError;
use crate::rule_engine::eval_expr::evaluate_expr;
use crate::rule_engine::types::{EvaluationInput, Rule, RuleOutput};

/// Evaluate an ordered list of [`Rule`]s against the provided [`EvaluationInput`].
///
/// Returns a reference to the [`RuleOutput`] of the **first matching rule**, or
/// `None` if no rule matches. Errors propagate immediately.
pub fn evaluate_rules<'r>(
    rules: &'r [Rule],
    input: &EvaluationInput<'_>,
) -> Result<Option<&'r RuleOutput>, RuleEngineError> {
    for rule in rules {
        if evaluate_expr(&rule.condition, input)? {
            return Ok(Some(&rule.output));
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::{Context, ParameterValue};
    use crate::id::{RuleId, VariantId};
    use crate::rule_engine::condition::Condition;
    use crate::rule_engine::types::{ConditionExpr, Rule, RuleOutput};

    fn pro_rule(variant: VariantId) -> Rule {
        Rule {
            id: RuleId::new(),
            condition: ConditionExpr::Leaf(Condition::Eq {
                context_type: "user".into(),
                param: "plan".into(),
                value: ParameterValue::Str("pro".into()),
            }),
            output: RuleOutput::Variant(variant),
        }
    }

    fn free_rule(variant: VariantId) -> Rule {
        Rule {
            id: RuleId::new(),
            condition: ConditionExpr::Leaf(Condition::Eq {
                context_type: "user".into(),
                param: "plan".into(),
                value: ParameterValue::Str("free".into()),
            }),
            output: RuleOutput::Variant(variant),
        }
    }

    fn always_rule(variant: VariantId) -> Rule {
        Rule {
            id: RuleId::new(),
            condition: ConditionExpr::And(vec![]), // always true
            output: RuleOutput::Variant(variant),
        }
    }

    fn user_ctx(plan: &str) -> [Context; 1] {
        [Context::new("user", "u-1").with_parameter("plan", ParameterValue::Str(plan.into()))]
    }

    // ── No match ─────────────────────────────────────────────────────────────

    #[test]
    fn empty_rules_returns_none() {
        let ctx = user_ctx("pro");
        let result = evaluate_rules(&[], &EvaluationInput::new(&ctx));
        assert_eq!(result, Ok(None));
    }

    #[test]
    fn no_matching_rule_returns_none() {
        let v = VariantId::new();
        let rules = [pro_rule(v)];
        let ctx = user_ctx("free");
        let result = evaluate_rules(&rules, &EvaluationInput::new(&ctx));
        assert_eq!(result, Ok(None));
    }

    // ── First match wins ──────────────────────────────────────────────────────

    #[test]
    fn first_matching_rule_wins() {
        let v1 = VariantId::new();
        let v2 = VariantId::new();
        let rules = [pro_rule(v1), always_rule(v2)];
        let ctx = user_ctx("pro");
        let result = evaluate_rules(&rules, &EvaluationInput::new(&ctx));
        assert_eq!(result, Ok(Some(&RuleOutput::Variant(v1))));
    }

    #[test]
    fn second_rule_matches_when_first_does_not() {
        let v1 = VariantId::new();
        let v2 = VariantId::new();
        let rules = [free_rule(v1), pro_rule(v2)];
        let ctx = user_ctx("pro");
        let result = evaluate_rules(&rules, &EvaluationInput::new(&ctx));
        assert_eq!(result, Ok(Some(&RuleOutput::Variant(v2))));
    }

    #[test]
    fn fallthrough_always_rule_matches_last() {
        let v1 = VariantId::new();
        let v_default = VariantId::new();
        let rules = [free_rule(v1), always_rule(v_default)];
        let ctx = user_ctx("pro"); // neither free → falls to always_rule
        let result = evaluate_rules(&rules, &EvaluationInput::new(&ctx));
        assert_eq!(result, Ok(Some(&RuleOutput::Variant(v_default))));
    }

    // ── Error stops evaluation ────────────────────────────────────────────────

    #[test]
    fn error_in_rule_stops_evaluation() {
        let v_unreachable = VariantId::new();
        // First rule references missing context
        let error_rule = Rule {
            id: RuleId::new(),
            condition: ConditionExpr::Leaf(Condition::Eq {
                context_type: "org".into(), // not provided
                param: "tier".into(),
                value: ParameterValue::Str("enterprise".into()),
            }),
            output: RuleOutput::Variant(v_unreachable),
        };
        let rules = [error_rule, always_rule(v_unreachable)];
        let ctx = user_ctx("pro");
        let result = evaluate_rules(&rules, &EvaluationInput::new(&ctx));
        assert!(matches!(
            result,
            Err(RuleEngineError::MissingContext { .. })
        ));
    }
}
