use crate::id::{EnvironmentId, FlagKey, ProjectId, VariantId};
use crate::rule_engine::error::RuleEngineError;
use crate::rule_engine::types::{EvaluationInput, PercentageTarget, TargetField};
use siphasher::sip::SipHasher13;
use std::hash::{Hash, Hasher};

/// Resolve each [`PercentageTarget`] to its string representation.
///
/// Returns an ordered `Vec<String>` matching the declaration order of `targets`.
/// `MissingContext` or `MissingParameter` is returned on lookup failure.
pub fn resolve_targets(
    targets: &[PercentageTarget],
    input: &EvaluationInput<'_>,
) -> Result<Vec<String>, RuleEngineError> {
    targets.iter().map(|t| resolve_one(t, input)).collect()
}

fn resolve_one(
    target: &PercentageTarget,
    input: &EvaluationInput<'_>,
) -> Result<String, RuleEngineError> {
    let ctx = input.find_context(&target.context_type).ok_or_else(|| {
        RuleEngineError::MissingContext {
            context_type: target.context_type.clone(),
        }
    })?;

    match &target.field {
        TargetField::Key => Ok(ctx.key.clone()),
        TargetField::Parameter(name) => {
            let val =
                ctx.parameters
                    .get(name)
                    .ok_or_else(|| RuleEngineError::MissingParameter {
                        param: name.clone(),
                    })?;
            Ok(val.to_string())
        }
    }
}

/// Hash resolved target values + stable salt, then bucket into the weighted
/// variant set.
///
/// # Arguments
/// - `targets` — one or more percentage targets (must be non-empty)
/// - `weights` — `(VariantId, weight)` pairs; weights are basis points
///   and must sum to 10000
/// - `input` — evaluation input for context lookup
/// - `flag_key` — used as part of the stable hash salt
/// - `project_id` — stable salt component
/// - `environment_id` — stable salt component
pub fn allocate_percentage(
    targets: &[PercentageTarget],
    weights: &[(VariantId, u32)],
    input: &EvaluationInput<'_>,
    flag_key: &FlagKey,
    project_id: ProjectId,
    environment_id: EnvironmentId,
) -> Result<VariantId, RuleEngineError> {
    if targets.is_empty() {
        return Err(RuleEngineError::EmptyPercentageTargets);
    }

    let total: u32 = weights.iter().map(|(_, w)| w).sum();
    if total != 10000 {
        return Err(RuleEngineError::InvalidWeights);
    }

    let resolved = resolve_targets(targets, input)?;

    // Build hash input: pipe-separated resolved values + salt fields.
    let hash_input = format!(
        "{}|{}|{}|{}",
        resolved.join("|"),
        flag_key.as_str(),
        project_id,
        environment_id,
    );

    let bucket = siphash_bucket(&hash_input);

    // Walk cumulative weights to find the matching variant.
    let mut cumulative: u32 = 0;
    for (variant_id, weight) in weights {
        cumulative += weight;
        if bucket < cumulative {
            return Ok(*variant_id);
        }
    }

    // Unreachable when weights sum to 10000 and bucket is in [0, 10000).
    unreachable!("bucket {bucket} not covered by cumulative weights summing to {total}")
}

/// Compute SipHash-1-3 of `input` and return `hash mod 10000`.
fn siphash_bucket(input: &str) -> u32 {
    let mut hasher = SipHasher13::new();
    input.hash(&mut hasher);
    (hasher.finish() % 10000) as u32
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::{Context, ParameterValue};
    use crate::id::{EnvironmentId, FlagKey, ProjectId, VariantId};
    use crate::rule_engine::types::{EvaluationInput, PercentageTarget, TargetField};

    fn env() -> (FlagKey, ProjectId, EnvironmentId) {
        (
            FlagKey::new("my-flag").unwrap(),
            ProjectId::new(),
            EnvironmentId::new(),
        )
    }

    fn user_ctx(key: &str) -> Context {
        Context::new("user", key).with_parameter("tier", ParameterValue::Str("gold".into()))
    }

    fn fifty_fifty(v1: VariantId, v2: VariantId) -> Vec<(VariantId, u32)> {
        vec![(v1, 5000), (v2, 5000)]
    }

    // ── resolve_targets ───────────────────────────────────────────────────────

    #[test]
    fn resolve_key_target() {
        let ctx = [user_ctx("user-abc")];
        let input = EvaluationInput::new(&ctx);
        let targets = [PercentageTarget {
            context_type: "user".into(),
            field: TargetField::Key,
        }];
        let resolved = resolve_targets(&targets, &input).unwrap();
        assert_eq!(resolved, vec!["user-abc"]);
    }

    #[test]
    fn resolve_parameter_target() {
        let ctx = [user_ctx("u-1")];
        let input = EvaluationInput::new(&ctx);
        let targets = [PercentageTarget {
            context_type: "user".into(),
            field: TargetField::Parameter("tier".into()),
        }];
        let resolved = resolve_targets(&targets, &input).unwrap();
        assert_eq!(resolved, vec!["gold"]);
    }

    #[test]
    fn resolve_missing_context_returns_error() {
        let ctx: [Context; 0] = [];
        let input = EvaluationInput::new(&ctx);
        let targets = [PercentageTarget {
            context_type: "user".into(),
            field: TargetField::Key,
        }];
        assert_eq!(
            resolve_targets(&targets, &input),
            Err(RuleEngineError::MissingContext {
                context_type: "user".into()
            })
        );
    }

    #[test]
    fn resolve_missing_parameter_returns_error() {
        let ctx = [Context::new("user", "u-1")];
        let input = EvaluationInput::new(&ctx);
        let targets = [PercentageTarget {
            context_type: "user".into(),
            field: TargetField::Parameter("missing".into()),
        }];
        assert_eq!(
            resolve_targets(&targets, &input),
            Err(RuleEngineError::MissingParameter {
                param: "missing".into()
            })
        );
    }

    // ── allocate_percentage: error cases ─────────────────────────────────────

    #[test]
    fn empty_targets_returns_error() {
        let ctx: [Context; 0] = [];
        let input = EvaluationInput::new(&ctx);
        let v = VariantId::new();
        let (fk, pid, eid) = env();
        let result = allocate_percentage(&[], &[(v, 10000)], &input, &fk, pid, eid);
        assert_eq!(result, Err(RuleEngineError::EmptyPercentageTargets));
    }

    #[test]
    fn invalid_weights_returns_error() {
        let ctx = [user_ctx("u-1")];
        let input = EvaluationInput::new(&ctx);
        let v = VariantId::new();
        let targets = [PercentageTarget {
            context_type: "user".into(),
            field: TargetField::Key,
        }];
        let (fk, pid, eid) = env();
        let result = allocate_percentage(&targets, &[(v, 9999)], &input, &fk, pid, eid);
        assert_eq!(result, Err(RuleEngineError::InvalidWeights));
    }

    #[test]
    fn weights_summing_to_zero_is_invalid() {
        let ctx = [user_ctx("u-1")];
        let input = EvaluationInput::new(&ctx);
        let targets = [PercentageTarget {
            context_type: "user".into(),
            field: TargetField::Key,
        }];
        let (fk, pid, eid) = env();
        let result = allocate_percentage(&targets, &[], &input, &fk, pid, eid);
        assert_eq!(result, Err(RuleEngineError::InvalidWeights));
    }

    // ── allocate_percentage: determinism ──────────────────────────────────────

    #[test]
    fn same_inputs_always_produce_same_variant() {
        let ctx = [user_ctx("user-stable")];
        let v1 = VariantId::new();
        let v2 = VariantId::new();
        let weights = fifty_fifty(v1, v2);
        let targets = [PercentageTarget {
            context_type: "user".into(),
            field: TargetField::Key,
        }];
        let (fk, pid, eid) = env();

        let first = allocate_percentage(
            &targets,
            &weights,
            &EvaluationInput::new(&ctx),
            &fk,
            pid,
            eid,
        )
        .unwrap();
        let second = allocate_percentage(
            &targets,
            &weights,
            &EvaluationInput::new(&ctx),
            &fk,
            pid,
            eid,
        )
        .unwrap();
        assert_eq!(first, second);
    }

    #[test]
    fn different_keys_can_produce_different_buckets() {
        // Run enough distinct users to statistically expect at least one difference.
        let v1 = VariantId::new();
        let v2 = VariantId::new();
        let weights = fifty_fifty(v1, v2);
        let (fk, pid, eid) = env();

        let mut seen = std::collections::HashSet::new();
        for i in 0..20 {
            let key = format!("user-{i}");
            let ctx = [Context::new("user", &key)];
            let targets = [PercentageTarget {
                context_type: "user".into(),
                field: TargetField::Key,
            }];
            let r = allocate_percentage(
                &targets,
                &weights,
                &EvaluationInput::new(&ctx),
                &fk,
                pid,
                eid,
            )
            .unwrap();
            seen.insert(r);
        }
        assert!(
            seen.len() > 1,
            "expected both variants to appear across 20 users"
        );
    }

    // ── allocate_percentage: multi-target ordering ────────────────────────────

    #[test]
    fn multi_target_ordering_affects_bucket() {
        // Hashing [user_key | org_tier] vs [org_tier | user_key] produces different inputs.
        let ctx = [
            Context::new("user", "u-1"),
            Context::new("org", "o-1").with_parameter("tier", ParameterValue::Str("gold".into())),
        ];
        let v1 = VariantId::new();
        let v2 = VariantId::new();
        let weights = fifty_fifty(v1, v2);
        let (fk, pid, eid) = env();

        let targets_ab = [
            PercentageTarget {
                context_type: "user".into(),
                field: TargetField::Key,
            },
            PercentageTarget {
                context_type: "org".into(),
                field: TargetField::Parameter("tier".into()),
            },
        ];
        let targets_ba = [
            PercentageTarget {
                context_type: "org".into(),
                field: TargetField::Parameter("tier".into()),
            },
            PercentageTarget {
                context_type: "user".into(),
                field: TargetField::Key,
            },
        ];

        let result_ab = allocate_percentage(
            &targets_ab,
            &weights,
            &EvaluationInput::new(&ctx),
            &fk,
            pid,
            eid,
        )
        .unwrap();
        let result_ba = allocate_percentage(
            &targets_ba,
            &weights,
            &EvaluationInput::new(&ctx),
            &fk,
            pid,
            eid,
        )
        .unwrap();
        // They may or may not differ (hash collisions possible), but we verify both complete without error.
        let _ = (result_ab, result_ba);
        // Verify that swapping order changes the hash string (we can check directly).
        let resolved_ab = resolve_targets(&targets_ab, &EvaluationInput::new(&ctx)).unwrap();
        let resolved_ba = resolve_targets(&targets_ba, &EvaluationInput::new(&ctx)).unwrap();
        assert_ne!(
            resolved_ab, resolved_ba,
            "target ordering must change hash input"
        );
    }
}
