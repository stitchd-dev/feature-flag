use crate::context::Context;
use crate::id::{FlagId, RuleId, SegmentId, VariantId};
use crate::rule_engine::condition::Condition;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

// ── ConditionExpr ─────────────────────────────────────────────────────────────

/// A recursive condition expression supporting arbitrary nesting of AND / OR / NOT.
///
/// # Evaluation rules
/// - `And([])` evaluates to `true`  (vacuously true)
/// - `Or([])` evaluates to `false` (vacuously false)
/// - `And` short-circuits on the first `false` child
/// - `Or` short-circuits on the first `true` child
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[cfg_attr(feature = "openapi", schema(no_recursion))]
pub enum ConditionExpr {
    /// A single leaf condition test.
    Leaf(Condition),
    /// All children must evaluate to `true`.
    And(Vec<ConditionExpr>),
    /// At least one child must evaluate to `true`.
    Or(Vec<ConditionExpr>),
    /// Inverts the inner expression.
    Not(Box<ConditionExpr>),
}

// ── PercentageTarget / TargetField ────────────────────────────────────────────

/// Identifies which field on which context to hash for percentage allocation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub enum TargetField {
    /// Use `Context::key`.
    Key,
    /// Use `Context::parameters[name]` (stringified via `Display`).
    Parameter(String),
}

/// One contributor to the percentage hash input.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct PercentageTarget {
    /// Which context type to look up.
    pub context_type: String,
    /// Which field within that context to use.
    pub field: TargetField,
}

// ── ExclusionGate ─────────────────────────────────────────────────────────────

/// A mutually-exclusive-group gate resident on a rule's percentage allocation.
///
/// When present, a context is admitted to the rule's percentage distribution
/// only if its exclusion-group bucket (computed from `group_salt`) falls in the
/// allocated `[bucket_lo, bucket_hi)` basis-point range. This is the
/// spec-mandated location for the gate: it lives on the rule output, not on the
/// experiment, so a context can be excluded before any percentage bucketing.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ExclusionGate {
    /// Salt used to derive the context's exclusion-group bucket (shared by all
    /// members of the same exclusion group).
    pub group_salt: String,
    /// The context type whose `key` is hashed to derive the exclusion-group
    /// bucket (the experiment's randomization unit, e.g. `"user"`). If no
    /// context of this type is present in the evaluated bundle, the context is
    /// held out (not enrolled) — a missing randomization unit cannot be bucketed.
    pub context_type: String,
    /// Inclusive lower bound of the allocated bucket range, in basis points.
    pub bucket_lo: u16,
    /// Exclusive upper bound of the allocated bucket range, in basis points.
    pub bucket_hi: u16,
}

// ── RealtimeBanditModel ───────────────────────────────────────────────────────

/// Reward-metric family for a real-time bandit posterior.
///
/// Selects how a [`VariantPosterior`] is sampled: a `Beta` family draws a rate
/// from `Beta(alpha, beta)` (conversion / funnel); a `Normal` family draws a
/// mean from `Normal(mu, sigma2)` (numeric / ratio).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum RewardFamily {
    /// Beta-Binomial posterior on a rate (conversion / funnel).
    Beta,
    /// Normal-Normal posterior on a mean (numeric / ratio).
    Normal,
}

/// Goal direction for a real-time bandit reward: whether a higher or lower
/// sampled reward should win the per-context argmax.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum BanditGoal {
    /// Higher reward is better — assign the argmax of the sampled draws.
    Increase,
    /// Lower reward is better — assign the argmin of the sampled draws.
    Decrease,
}

/// Posterior parameters for one variant of a real-time bandit.
///
/// For a [`RewardFamily::Beta`] model `alpha`/`beta` carry the posterior and
/// `mu`/`sigma2` are ignored; for a [`RewardFamily::Normal`] model `mu`/`sigma2`
/// carry the posterior and `alpha`/`beta` are ignored. The struct holds both so
/// a single shape carries either family (and so Phase 6 contextual models can
/// extend it without changing the wire format).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct VariantPosterior {
    /// The variant key this posterior routes traffic to.
    pub variant_key: String,
    /// Beta posterior `alpha` (Beta family).
    pub alpha: f64,
    /// Beta posterior `beta` (Beta family).
    pub beta: f64,
    /// Normal posterior mean (Normal family).
    pub mu: f64,
    /// Normal posterior variance (Normal family).
    pub sigma2: f64,
}

/// A snapshot-resident real-time bandit reward model on a rule's percentage
/// allocation.
///
/// When present on a [`RuleOutput::Percentage`], the evaluator draws one
/// posterior sample per variant — seeded deterministically from the unit
/// context's key plus `salt` — and assigns the goal-directed argmax variant,
/// *instead of* the static bucket→weight allocation. This mirrors
/// [`ExclusionGate`]: pure rule data that rides on the flag snapshot, sampled
/// with zero DB lookup at evaluation time. Absent on every non-realtime-bandit
/// rule, which keeps the static eval path byte-identical.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct RealtimeBanditModel {
    /// Salt mixed into the per-context sampling seed so the same unit produces a
    /// stable draw for this experiment and an independent one for another.
    pub salt: String,
    /// The context type whose `key` seeds the per-context draw (the experiment's
    /// randomization / diversion unit, e.g. `"user"`). If no context of this type
    /// is present in the evaluated bundle, the rule falls back to the static
    /// allocation — a missing unit cannot be sampled.
    pub unit_context_type: String,
    /// The reward-metric family driving the posteriors.
    pub family: RewardFamily,
    /// The goal direction used to pick the winning sampled variant.
    pub goal: BanditGoal,
    /// Per-variant posterior parameters; one entry per assignable variant.
    pub variants: Vec<VariantPosterior>,
}

// ── RuleOutput ────────────────────────────────────────────────────────────────

/// The outcome produced when a rule matches.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub enum RuleOutput {
    /// Assign the evaluation directly to this variant.
    Variant(VariantId),
    /// Hash one or more context fields to bucket into a weighted variant set.
    Percentage {
        /// At least one target is required; values are hashed in declaration order.
        targets: Vec<PercentageTarget>,
        /// `(variant_id, weight)` pairs; weights are basis points and must sum to 10000.
        weights: Vec<(VariantId, u32)>,
        /// Optional mutually-exclusive-group gate. When `Some`, contexts whose
        /// exclusion-group bucket falls outside the allocated range are excluded
        /// from this distribution. Absent in legacy serialized rules → `None`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        exclusion_gate: Option<ExclusionGate>,
        /// Optional snapshot-resident real-time bandit model. When `Some`, the
        /// variant is drawn per-context from these posteriors (a context-seeded
        /// Thompson draw) instead of the static `weights` bucketing. Absent in
        /// legacy serialized rules and on every non-realtime-bandit rule →
        /// `None`, which preserves the static eval path exactly.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        realtime_bandit: Option<RealtimeBanditModel>,
    },
}

// ── Rule ──────────────────────────────────────────────────────────────────────

/// A single rule: a condition expression paired with an output.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Rule {
    pub id: RuleId,
    /// Optional human-readable label. Ignored by the evaluator; UI metadata only.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub condition: ConditionExpr,
    pub output: RuleOutput,
}

impl ConditionExpr {
    /// Collect all `SegmentId`s referenced by `InSegment` or `NotInSegment` leaves.
    pub fn collect_segment_ids(&self, out: &mut HashSet<SegmentId>) {
        match self {
            Self::Leaf(Condition::InSegment(id)) | Self::Leaf(Condition::NotInSegment(id)) => {
                out.insert(*id);
            }
            Self::Leaf(_) => {}
            Self::And(children) | Self::Or(children) => {
                for child in children {
                    child.collect_segment_ids(out);
                }
            }
            Self::Not(inner) => inner.collect_segment_ids(out),
        }
    }
}

// ── EvaluationInput ───────────────────────────────────────────────────────────

/// All inputs required for a single rule-engine evaluation pass.
///
/// `contexts` is borrowed to avoid cloning at evaluation time.
pub struct EvaluationInput<'a> {
    /// Evaluation contexts, one per `context_type`; no two share the same type.
    pub contexts: &'a [Context],
    /// Segments that the evaluation subject has been pre-resolved into.
    pub resolved_segments: HashSet<SegmentId>,
    /// Results of previously evaluated flags (for cross-flag conditions).
    pub evaluated_flags: HashMap<FlagId, VariantId>,
}

impl<'a> EvaluationInput<'a> {
    /// Construct a new evaluation input.
    pub fn new(contexts: &'a [Context]) -> Self {
        Self {
            contexts,
            resolved_segments: HashSet::new(),
            evaluated_flags: HashMap::new(),
        }
    }

    /// Find the context with the given `context_type`, if any.
    pub fn find_context(&self, context_type: &str) -> Option<&Context> {
        self.contexts
            .iter()
            .find(|c| c.context_type == context_type)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::ParameterValue;
    use crate::id::SegmentId;
    use crate::rule_engine::condition::Condition;

    fn user_ctx() -> Context {
        Context::new("user", "u-1").with_parameter("plan", ParameterValue::Str("pro".to_string()))
    }

    #[test]
    fn evaluation_input_find_context() {
        let ctx = user_ctx();
        let input = EvaluationInput::new(std::slice::from_ref(&ctx));
        assert!(input.find_context("user").is_some());
        assert!(input.find_context("org").is_none());
    }

    #[test]
    fn condition_expr_leaf_roundtrip() {
        let expr = ConditionExpr::Leaf(Condition::InSegment(SegmentId::new()));
        let json = serde_json::to_string(&expr).unwrap();
        let back: ConditionExpr = serde_json::from_str(&json).unwrap();
        assert_eq!(expr, back);
    }

    #[test]
    fn condition_expr_not_box() {
        let inner = ConditionExpr::And(vec![]);
        let not = ConditionExpr::Not(Box::new(inner));
        assert!(matches!(not, ConditionExpr::Not(_)));
    }

    #[test]
    fn rule_output_variant_roundtrip() {
        let output = RuleOutput::Variant(VariantId::new());
        let json = serde_json::to_string(&output).unwrap();
        let back: RuleOutput = serde_json::from_str(&json).unwrap();
        assert_eq!(output, back);
    }

    #[test]
    fn percentage_target_field_key() {
        let t = PercentageTarget {
            context_type: "user".to_string(),
            field: TargetField::Key,
        };
        assert_eq!(t.field, TargetField::Key);
    }

    #[test]
    fn percentage_target_field_parameter() {
        let t = PercentageTarget {
            context_type: "org".to_string(),
            field: TargetField::Parameter("account_tier".to_string()),
        };
        assert!(matches!(t.field, TargetField::Parameter(_)));
    }

    #[test]
    fn exclusion_gate_serde_round_trips() {
        let gate = ExclusionGate {
            group_salt: "grp-salt".to_string(),
            context_type: "user".to_string(),
            bucket_lo: 0,
            bucket_hi: 2500,
        };
        let json = serde_json::to_string(&gate).unwrap();
        let back: ExclusionGate = serde_json::from_str(&json).unwrap();
        assert_eq!(gate, back);
    }

    #[test]
    fn percentage_deserializes_without_exclusion_gate() {
        // Legacy serialized JSONB lacking `exclusion_gate` → None.
        let json = r#"{"Percentage":{"targets":[],"weights":[]}}"#;
        let output: RuleOutput = serde_json::from_str(json).unwrap();
        match output {
            RuleOutput::Percentage { exclusion_gate, .. } => assert!(exclusion_gate.is_none()),
            other => panic!("expected Percentage, got {other:?}"),
        }
    }

    #[test]
    fn percentage_skips_serializing_none_exclusion_gate() {
        let output = RuleOutput::Percentage {
            targets: vec![],
            weights: vec![],
            exclusion_gate: None,
            realtime_bandit: None,
        };
        let json = serde_json::to_string(&output).unwrap();
        assert!(
            !json.contains("exclusion_gate"),
            "None gate should be skipped: {json}"
        );
    }

    #[test]
    fn percentage_round_trips_with_exclusion_gate() {
        let output = RuleOutput::Percentage {
            targets: vec![],
            weights: vec![(VariantId::new(), 10000)],
            exclusion_gate: Some(ExclusionGate {
                group_salt: "s".to_string(),
                context_type: "user".to_string(),
                bucket_lo: 2500,
                bucket_hi: 5000,
            }),
            realtime_bandit: None,
        };
        let json = serde_json::to_string(&output).unwrap();
        let back: RuleOutput = serde_json::from_str(&json).unwrap();
        assert_eq!(output, back);
    }

    #[test]
    fn percentage_round_trips_with_realtime_bandit() {
        let output = RuleOutput::Percentage {
            targets: vec![],
            weights: vec![(VariantId::new(), 10000)],
            exclusion_gate: None,
            realtime_bandit: Some(RealtimeBanditModel {
                salt: "exp-salt".to_string(),
                unit_context_type: "user".to_string(),
                family: RewardFamily::Beta,
                goal: BanditGoal::Increase,
                variants: vec![
                    VariantPosterior {
                        variant_key: "control".to_string(),
                        alpha: 10.0,
                        beta: 90.0,
                        mu: 0.0,
                        sigma2: 0.0,
                    },
                    VariantPosterior {
                        variant_key: "treatment".to_string(),
                        alpha: 30.0,
                        beta: 70.0,
                        mu: 0.0,
                        sigma2: 0.0,
                    },
                ],
            }),
        };
        let json = serde_json::to_string(&output).unwrap();
        let back: RuleOutput = serde_json::from_str(&json).unwrap();
        assert_eq!(output, back);
    }

    #[test]
    fn realtime_bandit_absent_in_legacy_serialized_rule() {
        // A legacy rule JSON (no realtime_bandit key) must decode with None.
        let legacy = r#"{"Percentage":{"targets":[],"weights":[]}}"#;
        let back: RuleOutput = serde_json::from_str(legacy).unwrap();
        match back {
            RuleOutput::Percentage {
                realtime_bandit,
                exclusion_gate,
                ..
            } => {
                assert!(realtime_bandit.is_none());
                assert!(exclusion_gate.is_none());
            }
            _ => panic!("expected Percentage"),
        }
    }

    #[test]
    fn realtime_bandit_omitted_when_none_on_serialize() {
        // skip_serializing_if keeps the field out of the JSON when None, so
        // legacy consumers never see an unexpected key.
        let output = RuleOutput::Percentage {
            targets: vec![],
            weights: vec![],
            exclusion_gate: None,
            realtime_bandit: None,
        };
        let json = serde_json::to_string(&output).unwrap();
        assert!(!json.contains("realtime_bandit"), "json was {json}");
    }
}
