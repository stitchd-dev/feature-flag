use crate::context::EvaluationContext;
use crate::flag::{Flag, Variant};
use crate::rule_engine::eval_rules::evaluate_rules;
use crate::rule_engine::types::{EvaluationInput, Rule, RuleOutput};
use crate::rule_engine::error::RuleEngineError;
use std::collections::{HashMap, HashSet};
use crate::id::SegmentId;

/// A high-level evaluator for feature flags.
pub struct FlagEvaluator;

impl FlagEvaluator {
    /// Evaluates a flag against the given context and resolved segments.
    pub fn evaluate<'a>(
        flag: &'a Flag,
        context: &EvaluationContext,
        resolved_segments: &HashSet<SegmentId>,
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
                        RuleEngineError::Internal(format!("Rule matched variant ID {} but it does not exist in the flag", variant_id))
                    });
                }
                RuleOutput::Percentage { targets: _, weights: _ } => {
                    // TODO: Implement percentage rollout logic using hashing
                    return Err(RuleEngineError::Internal("Percentage rollout not yet implemented in FlagEvaluator".to_string()));
                }
            }
        }

        // 5. Fallback to default variant
        flag.get_default_variant().ok_or_else(|| {
            RuleEngineError::Internal("No rules matched and flag has no default variant".to_string())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::flag::{FlagRecord, FlagValueType, VariantValue, FlagRule};
    use crate::id::{FlagId, FlagKey, ProjectId, VariantId, RuleId};
    use crate::context::{Context, ParameterValue};
    use crate::rule_engine::types::ConditionExpr;
    use crate::rule_engine::condition::Condition;
    use chrono::Utc;

    fn setup_flag() -> Flag {
        let flag_id = FlagId::new();
        let project_id = ProjectId::new();
        let v1_id = VariantId::new();
        let v2_id = VariantId::new();

        let record = FlagRecord {
            id: flag_id,
            project_id,
            key: FlagKey::new("test-flag").unwrap(),
            value_type: FlagValueType::Bool,
            enabled: true,
            default_variant_id: Some(v2_id),
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

        let rules = vec![
            FlagRule {
                flag_id,
                rule_index: 0,
                rule: Rule {
                    id: RuleId::new(),
                    condition: ConditionExpr::Leaf(Condition::Eq {
                        context_type: "user".to_string(),
                        param: "beta".to_string(),
                        value: ParameterValue::Bool(true),
                    }),
                    output: RuleOutput::Variant(v1_id),
                },
            },
        ];

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
            Context::new("user", "u1").with_parameter("beta", ParameterValue::Bool(true))
        );
        let segments = HashSet::new();

        let result = FlagEvaluator::evaluate(&flag, &context, &segments).unwrap();
        assert_eq!(result.key, "on");
    }

    #[test]
    fn test_evaluate_no_match_fallback() {
        let flag = setup_flag();
        let context = EvaluationContext::new().with_context(
            Context::new("user", "u1").with_parameter("beta", ParameterValue::Bool(false))
        );
        let segments = HashSet::new();

        let result = FlagEvaluator::evaluate(&flag, &context, &segments).unwrap();
        assert_eq!(result.key, "off");
    }

    #[test]
    fn test_evaluate_disabled_fallback() {
        let mut flag = setup_flag();
        flag.record.enabled = false;
        let context = EvaluationContext::new().with_context(
            Context::new("user", "u1").with_parameter("beta", ParameterValue::Bool(true))
        );
        let segments = HashSet::new();

        let result = FlagEvaluator::evaluate(&flag, &context, &segments).unwrap();
        assert_eq!(result.key, "off");
    }

    #[test]
    fn test_evaluate_segment_match() {
        let mut flag = setup_flag();
        let segment_id = SegmentId::new();
        let v1_id = flag.variants[0].id;

        // Add a segment rule
        flag.rules.insert(0, FlagRule {
            flag_id: flag.record.id,
            rule_index: -1,
            rule: Rule {
                id: RuleId::new(),
                condition: ConditionExpr::Leaf(Condition::InSegment(segment_id)),
                output: RuleOutput::Variant(v1_id),
            },
        });

        let context = EvaluationContext::new();
        let mut segments = HashSet::new();
        segments.insert(segment_id);

        let result = FlagEvaluator::evaluate(&flag, &context, &segments).unwrap();
        assert_eq!(result.key, "on");
    }
}
