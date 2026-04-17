use crate::context::EvaluationContext;
use crate::flag::{Flag, Variant};
use crate::hashing::calculate_allocation;
use crate::id::{EnvironmentId, SegmentId};
use crate::rule_engine::error::RuleEngineError;
use crate::rule_engine::eval_rules::evaluate_rules;
use crate::rule_engine::types::{EvaluationInput, Rule, RuleOutput, TargetField};
use std::collections::{HashMap, HashSet};

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
                RuleOutput::Percentage { targets, weights } => {
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

                    let percentage = calculate_allocation(
                        flag.record.key.as_str(),
                        &environment_id.to_string(),
                        &target_values,
                    );

                    // Map 0.0-100.0 to 0-999 bucket
                    let bucket = ((percentage * 10.0).floor() as u32).min(999);

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

        // 5. Fallback to default variant
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

        let rules = vec![FlagRule {
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
            weights: vec![(v1_id, 500), (v2_id, 500)],
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
            weights: vec![(v1_id, 500), (v2_id, 500)],
        };

        // Provide user context but NOT org context
        let context = EvaluationContext::new().with_context(
            Context::new("user", "u1").with_parameter("beta", ParameterValue::Bool(true)),
        );
        let segments = HashSet::new();
        let env_id = EnvironmentId::from_uuid(Uuid::nil());

        let result = FlagEvaluator::evaluate(&flag, &context, &segments, env_id);
        assert!(matches!(result, Err(RuleEngineError::MissingContext { .. })));
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
            weights: vec![(v1_id, 500), (v2_id, 500)],
        };

        let context = EvaluationContext::new().with_context(
            Context::new("user", "u1").with_parameter("beta", ParameterValue::Bool(true)),
            // NOTE: no "nonexistent_param"
        );
        let segments = HashSet::new();
        let env_id = EnvironmentId::from_uuid(Uuid::nil());

        let result = FlagEvaluator::evaluate(&flag, &context, &segments, env_id);
        assert!(matches!(result, Err(RuleEngineError::MissingParameter { .. })));
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
        // Only 1 weight covering bucket 0..500; bucket 500..999 uncovered
        flag.rules[0].rule.output = RuleOutput::Percentage {
            targets: vec![PercentageTarget {
                context_type: "user".to_string(),
                field: TargetField::Key,
            }],
            weights: vec![(v1_id, 1)], // only covers bucket 0
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
        assert!(found_error, "Expected an Internal error from uncovered bucket");
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
            weights: vec![(nonexistent, 1000)],
        };

        let context = EvaluationContext::new().with_context(
            Context::new("user", "u1").with_parameter("beta", ParameterValue::Bool(true)),
        );
        let segments = HashSet::new();
        let env_id = EnvironmentId::from_uuid(Uuid::nil());

        let result = FlagEvaluator::evaluate(&flag, &context, &segments, env_id);
        assert!(matches!(result, Err(RuleEngineError::Internal(_))));
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
            weights: vec![(v1_id, 500), (v2_id, 500)],
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
}
