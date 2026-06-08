//! Multi-objective reward combiner.
//!
//! Bandit allocators ([`super::thompson`], [`super::epsilon`], [`super::ucb`])
//! consume a per-arm reward. With a single objective metric that reward is just
//! that metric's posterior. With *multiple* objectives the operator picks one of
//! the [`RewardObjective`] combination modes (FR9):
//!
//! * [`RewardObjective::Scalar`] — passthrough: use the one metric's posterior
//!   directly (full posterior sampling preserved for Thompson).
//! * [`RewardObjective::Scalarized`] — standardise each metric's per-variant
//!   reward so "higher is better" (z-score across variants, goal-direction
//!   normalised), then take the weighted sum → a single scalar reward per
//!   variant.
//! * [`RewardObjective::Constrained`] — optimise the primary metric, but any
//!   variant whose guardrail-constraint posterior **violates** its bound is
//!   excluded from exploitation (it keeps only the exploration floor). The
//!   combiner returns a per-variant `exploitable` mask alongside the primary
//!   reward.
//!
//! ## Interface
//!
//! [`combined_rewards`] is the clean entry point: it returns one
//! [`CombinedArm`] per variant — `variant_key`, the scalar `reward`, and whether
//! the variant is `exploitable`. [`reward_arms`] adapts that into the
//! [`BanditArm`] + exploitable-mask pair the allocators consume.
//!
//! Two ways to apply the constrained-mode mask:
//! * [`allocate_exploitable`] — runs the allocator over *only* the eligible arms
//!   then merges excluded arms back at weight `0.0`. This is the robust path that
//!   works for **every** allocator, including the winner-take-all ones.
//! * [`apply_exploitable_mask`] — post-hoc zeroes non-exploitable arms' weights;
//!   fine for proportional allocators (Thompson) but it can starve a
//!   winner-take-all allocator if its sole winner is excluded.
//!
//! Either way, a zeroed arm drops to the exploration floor in
//! [`super::allocation::normalize_to_distribution`].
//!
//! Pure math: no RNG, no I/O.

use uuid::Uuid;

use super::thompson::{BanditArm, GoalDirection, RewardPosterior};
use super::types::{ConstraintDirection, GuardrailConstraint, RewardObjective};
use crate::experimentation::stats::VariantStats;

/// Per-metric input to the reward combiner: the metric's id, its optimisation
/// direction, and one reward posterior per variant (variant order is the
/// canonical arm order).
#[derive(Debug, Clone)]
pub struct MetricRewards {
    /// The metric definition this posterior set belongs to.
    pub metric_id: Uuid,
    /// Whether a higher or lower value of this metric is better.
    pub goal: GoalDirection,
    /// One `(variant_key, posterior)` per arm.
    pub arms: Vec<(String, RewardPosterior)>,
}

impl MetricRewards {
    /// The goal-normalised ("higher is better") posterior mean for each arm, in
    /// arm order. For [`GoalDirection::Decrease`] each mean is negated so a
    /// smaller raw metric yields a *larger* normalised reward.
    fn directed_means(&self) -> Vec<f64> {
        self.arms
            .iter()
            .map(|(_, p)| match self.goal {
                GoalDirection::Increase => p.mean(),
                GoalDirection::Decrease => -p.mean(),
            })
            .collect()
    }
}

/// One variant's combined reward and exploitation eligibility.
#[derive(Debug, Clone, PartialEq)]
pub struct CombinedArm {
    /// The variant key.
    pub variant_key: String,
    /// The scalar reward feeding the allocator (already goal-normalised so higher
    /// is better for the scalarized mode; the raw primary-metric posterior mean
    /// for scalar / constrained modes).
    pub reward: f64,
    /// Whether this variant may be *exploited*. `false` (constrained mode, bound
    /// violated) means the allocator should give it only the exploration floor.
    pub exploitable: bool,
}

/// Z-score standardise `values` across arms. When the spread is zero (all equal),
/// returns all-zeros so a flat metric contributes nothing to the weighted sum.
fn z_standardize(values: &[f64]) -> Vec<f64> {
    let n = values.len() as f64;
    if n == 0.0 {
        return Vec::new();
    }
    let mean = values.iter().sum::<f64>() / n;
    let var = values.iter().map(|v| (v - mean) * (v - mean)).sum::<f64>() / n;
    let sd = var.sqrt();
    if sd <= 0.0 {
        return vec![0.0; values.len()];
    }
    values.iter().map(|v| (v - mean) / sd).collect()
}

/// Does `mean` violate `constraint`?
///
/// * [`ConstraintDirection::AtLeast`]: violated when `mean < bound`.
/// * [`ConstraintDirection::AtMost`]: violated when `mean > bound`.
fn violates(mean: f64, constraint: &GuardrailConstraint) -> bool {
    match constraint.direction {
        ConstraintDirection::AtLeast => mean < constraint.bound,
        ConstraintDirection::AtMost => mean > constraint.bound,
    }
}

/// Combine one or more metric-reward sets into a per-variant scalar reward +
/// exploitable mask, per the chosen [`RewardObjective`].
///
/// `metrics` must be non-empty and every metric must list the SAME variants in
/// the SAME order (the canonical arm order). The variant order of the returned
/// vec follows the first metric.
///
/// Returns an empty vec if `metrics` is empty or the relevant metric is missing.
///
/// * **Scalar**: looks up the metric whose id matches `metric_id` (falling back
///   to the first metric); reward = that metric's directed posterior mean; all
///   arms exploitable.
/// * **Scalarized**: for each weighted metric, z-standardises its directed means
///   across arms and accumulates `weight · z`; reward = the weighted sum; all
///   arms exploitable. Metrics referenced by a weight but absent from `metrics`
///   are skipped.
/// * **Constrained**: reward = the primary metric's directed posterior mean; an
///   arm is non-exploitable if ANY constraint metric's posterior mean violates
///   its bound.
pub fn combined_rewards(
    objective: &RewardObjective,
    metrics: &[MetricRewards],
) -> Vec<CombinedArm> {
    if metrics.is_empty() {
        return Vec::new();
    }
    let find = |id: &Uuid| metrics.iter().find(|m| &m.metric_id == id);

    match objective {
        RewardObjective::Scalar { metric_id } => {
            let m = find(metric_id).unwrap_or(&metrics[0]);
            let means = m.directed_means();
            m.arms
                .iter()
                .zip(means)
                .map(|((key, _), reward)| CombinedArm {
                    variant_key: key.clone(),
                    reward,
                    exploitable: true,
                })
                .collect()
        }
        RewardObjective::Scalarized { weights } => {
            let base = &metrics[0];
            let n = base.arms.len();
            let mut combined = vec![0.0_f64; n];
            for ow in weights {
                if let Some(m) = find(&ow.metric_id) {
                    let z = z_standardize(&m.directed_means());
                    for (acc, zi) in combined.iter_mut().zip(z) {
                        *acc += ow.weight * zi;
                    }
                }
            }
            base.arms
                .iter()
                .zip(combined)
                .map(|((key, _), reward)| CombinedArm {
                    variant_key: key.clone(),
                    reward,
                    exploitable: true,
                })
                .collect()
        }
        RewardObjective::Constrained {
            primary_metric_id,
            constraints,
        } => {
            let primary = find(primary_metric_id).unwrap_or(&metrics[0]);
            let means = primary.directed_means();
            primary
                .arms
                .iter()
                .enumerate()
                .zip(means)
                .map(|((idx, (key, _)), reward)| {
                    let exploitable = !constraints.iter().any(|c| {
                        find(&c.metric_id)
                            .and_then(|m| m.arms.get(idx))
                            // Compare the RAW posterior mean against the bound (the
                            // constraint is stated in the metric's natural units).
                            .map(|(_, p)| violates(p.mean(), c))
                            .unwrap_or(false)
                    });
                    CombinedArm {
                        variant_key: key.clone(),
                        reward,
                        exploitable,
                    }
                })
                .collect()
        }
    }
}

/// One variant's per-objective posterior summary, for surfacing the multi-objective
/// tradeoff in the Bandit Results view (FR9 — operators see each objective, not
/// just the combined score).
#[derive(Debug, Clone, PartialEq)]
pub struct ObjectiveVariantPosterior {
    /// The variant key.
    pub variant_key: String,
    /// The raw posterior mean (in the metric's natural units — NOT goal-normalised).
    pub mean: f64,
    /// The 95% posterior credible-interval lower bound.
    pub ci_lower: f64,
    /// The 95% posterior credible-interval upper bound.
    pub ci_upper: f64,
    /// The effective sample size backing this arm's posterior.
    pub n: u64,
    /// `true` when this metric is a constrained-mode guardrail and this variant
    /// VIOLATES the bound (so the arm is held at the exploration floor). Always
    /// `false` for the primary / scalar / scalarized objective metrics.
    pub guardrail_violated: bool,
}

/// One objective metric's per-variant posterior summary.
#[derive(Debug, Clone, PartialEq)]
pub struct ObjectivePosterior {
    /// The metric definition this objective summary belongs to.
    pub metric_id: Uuid,
    /// The role this metric plays in the objective.
    pub role: ObjectiveRole,
    /// The optimisation direction of this metric.
    pub goal: GoalDirection,
    /// Per-variant posterior summary (in canonical arm order).
    pub variants: Vec<ObjectiveVariantPosterior>,
}

/// The role a metric plays inside a [`RewardObjective`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObjectiveRole {
    /// The single scalar objective metric.
    Scalar,
    /// A scalarised-mode weighted objective metric (carries its weight).
    Weighted(
        // weight scaled ×1000 so the role stays `Copy` without a float; the
        // surfacing layer divides back. (kept tiny — surfacing only.)
        i64,
    ),
    /// The constrained-mode primary objective.
    Primary,
    /// A constrained-mode guardrail constraint metric.
    Guardrail,
}

/// Build the per-objective per-variant posterior summaries for surfacing the
/// multi-objective tradeoff (FR9). Pure: derives means + 95% CIs from the same
/// posteriors the allocator consumed, plus the constrained-mode guardrail
/// violation flag per variant.
///
/// Returns one [`ObjectivePosterior`] per metric referenced by `objective` that
/// is present in `metrics`, in objective order (scalar/primary first, then
/// weighted/guardrail metrics). Metrics referenced but absent from `metrics` are
/// skipped. Returns empty when `metrics` is empty.
#[must_use]
pub fn objective_posteriors(
    objective: &RewardObjective,
    metrics: &[MetricRewards],
) -> Vec<ObjectivePosterior> {
    if metrics.is_empty() {
        return Vec::new();
    }
    let find = |id: &Uuid| metrics.iter().find(|m| &m.metric_id == id);

    let summarize = |m: &MetricRewards,
                     role: ObjectiveRole,
                     constraint: Option<&GuardrailConstraint>|
     -> ObjectivePosterior {
        let variants = m
            .arms
            .iter()
            .map(|(key, p)| {
                let (lo, hi) = p.ci95();
                ObjectiveVariantPosterior {
                    variant_key: key.clone(),
                    mean: p.mean(),
                    ci_lower: lo,
                    ci_upper: hi,
                    n: p.arm_n(),
                    guardrail_violated: constraint.is_some_and(|c| violates(p.mean(), c)),
                }
            })
            .collect();
        ObjectivePosterior {
            metric_id: m.metric_id,
            role,
            goal: m.goal,
            variants,
        }
    };

    match objective {
        RewardObjective::Scalar { metric_id } => find(metric_id)
            .or(metrics.first())
            .map(|m| summarize(m, ObjectiveRole::Scalar, None))
            .into_iter()
            .collect(),
        RewardObjective::Scalarized { weights } => weights
            .iter()
            .filter_map(|ow| {
                find(&ow.metric_id).map(|m| {
                    let scaled = (ow.weight * 1000.0).round() as i64;
                    summarize(m, ObjectiveRole::Weighted(scaled), None)
                })
            })
            .collect(),
        RewardObjective::Constrained {
            primary_metric_id,
            constraints,
        } => {
            let mut out = Vec::new();
            if let Some(m) = find(primary_metric_id).or(metrics.first()) {
                out.push(summarize(m, ObjectiveRole::Primary, None));
            }
            for c in constraints {
                if let Some(m) = find(&c.metric_id) {
                    out.push(summarize(m, ObjectiveRole::Guardrail, Some(c)));
                }
            }
            out
        }
    }
}

/// Adapt a [`RewardObjective`] + per-metric posteriors into the
/// `(arms, exploitable_mask)` pair the allocators consume.
///
/// * **Scalar / Constrained** preserve the primary metric's *real* posterior on
///   each arm (so Thompson keeps full uncertainty sampling) and return the goal
///   direction of that metric.
/// * **Scalarized** has no joint posterior, so each arm becomes a point
///   [`RewardPosterior::Numeric`] (variance 0) whose mean is the combined scalar
///   reward; the returned goal is [`GoalDirection::Increase`] because the
///   combined reward is already normalised so higher is better. Thompson over
///   zero-variance posteriors degenerates to argmax (winner-take-all), which is
///   the intended deterministic behaviour for a scalarized point reward.
///
/// The `exploitable` mask is the constrained-mode eligibility (all `true` for
/// scalar / scalarized).
pub fn reward_arms(
    objective: &RewardObjective,
    metrics: &[MetricRewards],
) -> (Vec<BanditArm>, GoalDirection, Vec<bool>) {
    if metrics.is_empty() {
        return (Vec::new(), GoalDirection::Increase, Vec::new());
    }
    let combined = combined_rewards(objective, metrics);
    let mask: Vec<bool> = combined.iter().map(|c| c.exploitable).collect();

    match objective {
        RewardObjective::Scalar { metric_id } => {
            let m = metrics
                .iter()
                .find(|m| &m.metric_id == metric_id)
                .unwrap_or(&metrics[0]);
            let arms = m
                .arms
                .iter()
                .map(|(key, p)| BanditArm {
                    variant_key: key.clone(),
                    posterior: p.clone(),
                })
                .collect();
            (arms, m.goal, mask)
        }
        RewardObjective::Constrained {
            primary_metric_id, ..
        } => {
            let m = metrics
                .iter()
                .find(|m| &m.metric_id == primary_metric_id)
                .unwrap_or(&metrics[0]);
            let arms = m
                .arms
                .iter()
                .map(|(key, p)| BanditArm {
                    variant_key: key.clone(),
                    posterior: p.clone(),
                })
                .collect();
            (arms, m.goal, mask)
        }
        RewardObjective::Scalarized { .. } => {
            // Point posteriors on the combined scalar reward (higher = better).
            let arms = combined
                .iter()
                .map(|c| BanditArm {
                    variant_key: c.variant_key.clone(),
                    posterior: RewardPosterior::Numeric(VariantStats {
                        sample_size: 1,
                        conversions: None,
                        mean: Some(c.reward),
                        variance: Some(0.0),
                        conversion_rate: None,
                        percentiles: None,
                    }),
                })
                .collect();
            (arms, GoalDirection::Increase, mask)
        }
    }
}

/// Zero the raw weight of every non-exploitable arm, leaving exploitable arms
/// untouched. The masked weights then flow into
/// [`super::allocation::normalize_to_distribution`], where a zeroed arm drops to
/// the exploration floor (it is never fully starved).
///
/// `raw_weights` and `exploitable` must be the same length and in the same arm
/// order; a length mismatch leaves the weights unchanged.
pub fn apply_exploitable_mask(
    raw_weights: &[(String, f64)],
    exploitable: &[bool],
) -> Vec<(String, f64)> {
    if raw_weights.len() != exploitable.len() {
        return raw_weights.to_vec();
    }
    raw_weights
        .iter()
        .zip(exploitable)
        .map(|((key, w), &ok)| (key.clone(), if ok { *w } else { 0.0 }))
        .collect()
}

/// Run an allocator over **only** the exploitable arms, then merge the excluded
/// arms back in at raw weight `0.0` (so they drop to the exploration floor in
/// [`super::allocation::normalize_to_distribution`]).
///
/// This is the robust constrained-mode pattern that works for *every* allocator,
/// including the winner-take-all ones ([`super::ucb`], `epsilon = 0`): excluding
/// non-exploitable arms *before* allocation means the winner is chosen among
/// eligible arms only — unlike post-hoc [`apply_exploitable_mask`], which can
/// zero a winner-take-all allocator's sole winner and leave nothing to exploit.
///
/// `allocator` is given the eligible arms (in their original relative order) and
/// must return one `(variant_key, weight)` per eligible arm. The returned vec is
/// in the original `arms` order. If no arm is exploitable, every arm gets `0.0`
/// (the normaliser then splits evenly above the floor).
pub fn allocate_exploitable<F>(
    arms: &[BanditArm],
    exploitable: &[bool],
    allocator: F,
) -> Vec<(String, f64)>
where
    F: FnOnce(&[BanditArm]) -> Vec<(String, f64)>,
{
    if arms.len() != exploitable.len() {
        return allocator(arms);
    }
    let eligible: Vec<BanditArm> = arms
        .iter()
        .zip(exploitable)
        .filter(|&(_, &ok)| ok)
        .map(|(a, _)| a.clone())
        .collect();

    if eligible.is_empty() {
        return arms.iter().map(|a| (a.variant_key.clone(), 0.0)).collect();
    }

    let eligible_weights = allocator(&eligible);
    // Map eligible weights back onto the full arm order; excluded arms get 0.0.
    arms.iter()
        .map(|a| {
            let w = eligible_weights
                .iter()
                .find(|(k, _)| k == &a.variant_key)
                .map(|(_, w)| *w)
                .unwrap_or(0.0);
            (a.variant_key.clone(), w)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::experimentation::bandit::thompson::thompson_weights;
    use crate::experimentation::bandit::ucb::ucb_weights;

    fn count_post(n: i64, conv: i64) -> RewardPosterior {
        RewardPosterior::Conversion(VariantStats {
            sample_size: n,
            conversions: Some(conv),
            mean: None,
            variance: None,
            conversion_rate: Some(if n > 0 { conv as f64 / n as f64 } else { 0.0 }),
            percentiles: None,
        })
    }

    fn numeric_post(n: i64, mean: f64, var: f64) -> RewardPosterior {
        RewardPosterior::Numeric(VariantStats {
            sample_size: n,
            conversions: None,
            mean: Some(mean),
            variance: Some(var),
            conversion_rate: None,
            percentiles: None,
        })
    }

    fn reward_of(arms: &[CombinedArm], key: &str) -> f64 {
        arms.iter().find(|c| c.variant_key == key).unwrap().reward
    }

    fn exploitable_of(arms: &[CombinedArm], key: &str) -> bool {
        arms.iter()
            .find(|c| c.variant_key == key)
            .unwrap()
            .exploitable
    }

    #[test]
    fn empty_metrics_returns_empty() {
        let obj = RewardObjective::Scalar {
            metric_id: Uuid::nil(),
        };
        assert!(combined_rewards(&obj, &[]).is_empty());
    }

    // ── Scalar passthrough ────────────────────────────────────────────────────

    #[test]
    fn scalar_passthrough_uses_metric_mean() {
        let id = Uuid::from_u128(1);
        let metrics = vec![MetricRewards {
            metric_id: id,
            goal: GoalDirection::Increase,
            arms: vec![
                ("a".into(), count_post(1000, 100)),
                ("b".into(), count_post(1000, 300)),
            ],
        }];
        let obj = RewardObjective::Scalar { metric_id: id };
        let c = combined_rewards(&obj, &metrics);
        // b has the higher rate → higher reward.
        assert!(reward_of(&c, "b") > reward_of(&c, "a"));
        assert!(exploitable_of(&c, "a") && exploitable_of(&c, "b"));
    }

    #[test]
    fn scalar_decrease_goal_negates() {
        let id = Uuid::from_u128(1);
        let metrics = vec![MetricRewards {
            metric_id: id,
            goal: GoalDirection::Decrease,
            arms: vec![
                ("low".into(), numeric_post(1000, 10.0, 1.0)),
                ("high".into(), numeric_post(1000, 90.0, 1.0)),
            ],
        }];
        let obj = RewardObjective::Scalar { metric_id: id };
        let c = combined_rewards(&obj, &metrics);
        // Lower raw value → higher directed reward.
        assert!(reward_of(&c, "low") > reward_of(&c, "high"));
    }

    // ── Scalarization ─────────────────────────────────────────────────────────

    /// Shifting the metric weights should flip which variant wins the combined
    /// reward.
    #[test]
    fn scalarization_weights_shift_winner() {
        let m1 = Uuid::from_u128(1);
        let m2 = Uuid::from_u128(2);
        // Variant a is great on metric 1, poor on metric 2; b is the reverse.
        let metrics = vec![
            MetricRewards {
                metric_id: m1,
                goal: GoalDirection::Increase,
                arms: vec![
                    ("a".into(), numeric_post(1000, 100.0, 1.0)),
                    ("b".into(), numeric_post(1000, 10.0, 1.0)),
                ],
            },
            MetricRewards {
                metric_id: m2,
                goal: GoalDirection::Increase,
                arms: vec![
                    ("a".into(), numeric_post(1000, 10.0, 1.0)),
                    ("b".into(), numeric_post(1000, 100.0, 1.0)),
                ],
            },
        ];
        use crate::experimentation::bandit::types::ObjectiveWeight;

        // Weight metric 1 heavily → a wins.
        let obj1 = RewardObjective::Scalarized {
            weights: vec![
                ObjectiveWeight {
                    metric_id: m1,
                    weight: 1.0,
                },
                ObjectiveWeight {
                    metric_id: m2,
                    weight: 0.0,
                },
            ],
        };
        let c1 = combined_rewards(&obj1, &metrics);
        assert!(reward_of(&c1, "a") > reward_of(&c1, "b"));

        // Weight metric 2 heavily → b wins.
        let obj2 = RewardObjective::Scalarized {
            weights: vec![
                ObjectiveWeight {
                    metric_id: m1,
                    weight: 0.0,
                },
                ObjectiveWeight {
                    metric_id: m2,
                    weight: 1.0,
                },
            ],
        };
        let c2 = combined_rewards(&obj2, &metrics);
        assert!(reward_of(&c2, "b") > reward_of(&c2, "a"));
    }

    #[test]
    fn scalarization_skips_absent_metric() {
        use crate::experimentation::bandit::types::ObjectiveWeight;
        let m1 = Uuid::from_u128(1);
        let metrics = vec![MetricRewards {
            metric_id: m1,
            goal: GoalDirection::Increase,
            arms: vec![
                ("a".into(), numeric_post(1000, 100.0, 1.0)),
                ("b".into(), numeric_post(1000, 10.0, 1.0)),
            ],
        }];
        let obj = RewardObjective::Scalarized {
            weights: vec![
                ObjectiveWeight {
                    metric_id: m1,
                    weight: 1.0,
                },
                ObjectiveWeight {
                    metric_id: Uuid::from_u128(999), // absent
                    weight: 5.0,
                },
            ],
        };
        let c = combined_rewards(&obj, &metrics);
        // Only m1 contributes → a (higher) wins.
        assert!(reward_of(&c, "a") > reward_of(&c, "b"));
    }

    // ── Constrained guardrails ────────────────────────────────────────────────

    /// A guardrail violation marks the otherwise-best arm non-exploitable.
    #[test]
    fn constrained_violation_marks_non_exploitable() {
        let primary = Uuid::from_u128(1);
        let guard = Uuid::from_u128(2);
        // b is best on the primary metric, but its guardrail (e.g. error rate,
        // at-most 0.05) is violated (0.20 > 0.05).
        let metrics = vec![
            MetricRewards {
                metric_id: primary,
                goal: GoalDirection::Increase,
                arms: vec![
                    ("a".into(), numeric_post(1000, 50.0, 1.0)),
                    ("b".into(), numeric_post(1000, 99.0, 1.0)),
                ],
            },
            MetricRewards {
                metric_id: guard,
                goal: GoalDirection::Decrease,
                arms: vec![
                    ("a".into(), numeric_post(1000, 0.01, 0.0001)),
                    ("b".into(), numeric_post(1000, 0.20, 0.0001)),
                ],
            },
        ];
        let obj = RewardObjective::Constrained {
            primary_metric_id: primary,
            constraints: vec![GuardrailConstraint {
                metric_id: guard,
                bound: 0.05,
                direction: ConstraintDirection::AtMost,
            }],
        };
        let c = combined_rewards(&obj, &metrics);
        assert!(exploitable_of(&c, "a"));
        assert!(!exploitable_of(&c, "b"), "b violates the guardrail");
        // Primary reward still reflects b being higher.
        assert!(reward_of(&c, "b") > reward_of(&c, "a"));
    }

    #[test]
    fn constrained_at_least_violation() {
        let primary = Uuid::from_u128(1);
        let guard = Uuid::from_u128(2);
        let metrics = vec![
            MetricRewards {
                metric_id: primary,
                goal: GoalDirection::Increase,
                arms: vec![("a".into(), numeric_post(1000, 50.0, 1.0))],
            },
            MetricRewards {
                metric_id: guard,
                goal: GoalDirection::Increase,
                arms: vec![("a".into(), numeric_post(1000, 0.5, 0.01))],
            },
        ];
        // Require guard >= 0.9; a has 0.5 → violated.
        let obj = RewardObjective::Constrained {
            primary_metric_id: primary,
            constraints: vec![GuardrailConstraint {
                metric_id: guard,
                bound: 0.9,
                direction: ConstraintDirection::AtLeast,
            }],
        };
        let c = combined_rewards(&obj, &metrics);
        assert!(!exploitable_of(&c, "a"));
    }

    // ── reward_arms + mask integration ────────────────────────────────────────

    #[test]
    fn reward_arms_scalar_preserves_posterior() {
        let id = Uuid::from_u128(1);
        let metrics = vec![MetricRewards {
            metric_id: id,
            goal: GoalDirection::Increase,
            arms: vec![
                ("a".into(), count_post(1000, 100)),
                ("b".into(), count_post(1000, 300)),
            ],
        }];
        let obj = RewardObjective::Scalar { metric_id: id };
        let (arms, goal, mask) = reward_arms(&obj, &metrics);
        assert_eq!(goal, GoalDirection::Increase);
        assert_eq!(mask, vec![true, true]);
        // Posterior preserved → Thompson sees real uncertainty.
        let w = thompson_weights(&arms, goal, 2000, 7);
        let wb = w.iter().find(|(k, _)| k == "b").unwrap().1;
        assert!(wb > 0.9, "b should dominate, got {wb}");
    }

    #[test]
    fn mask_zeros_non_exploitable_weight() {
        let raw = vec![("a".to_string(), 0.3), ("b".to_string(), 0.7)];
        let masked = apply_exploitable_mask(&raw, &[true, false]);
        assert_eq!(masked[0].1, 0.3);
        assert_eq!(masked[1].1, 0.0);
    }

    #[test]
    fn mask_length_mismatch_is_passthrough() {
        let raw = vec![("a".to_string(), 0.3)];
        let masked = apply_exploitable_mask(&raw, &[true, false]);
        assert_eq!(masked, raw);
    }

    /// End-to-end: constrained violation down-weights the otherwise-best arm to
    /// the floor through the allocator + mask + normaliser pipeline.
    #[test]
    fn constrained_best_arm_drops_to_floor() {
        use crate::experimentation::bandit::allocation::normalize_to_distribution;
        let primary = Uuid::from_u128(1);
        let guard = Uuid::from_u128(2);
        let metrics = vec![
            MetricRewards {
                metric_id: primary,
                goal: GoalDirection::Increase,
                arms: vec![
                    ("a".into(), count_post(10_000, 5_000)),
                    ("b".into(), count_post(10_000, 9_000)),
                ],
            },
            MetricRewards {
                metric_id: guard,
                goal: GoalDirection::Decrease,
                arms: vec![
                    ("a".into(), numeric_post(10_000, 0.01, 0.0001)),
                    ("b".into(), numeric_post(10_000, 0.50, 0.0001)),
                ],
            },
        ];
        let obj = RewardObjective::Constrained {
            primary_metric_id: primary,
            constraints: vec![GuardrailConstraint {
                metric_id: guard,
                bound: 0.05,
                direction: ConstraintDirection::AtMost,
            }],
        };
        let (arms, goal, mask) = reward_arms(&obj, &metrics);
        // Winner-take-all UCB: exclude non-exploitable arms BEFORE allocating so
        // the winner is chosen among eligible arms only.
        let masked =
            allocate_exploitable(&arms, &mask, |eligible| ucb_weights(eligible, goal, 0.001));
        let dist = normalize_to_distribution(&masked, 500).unwrap();
        let bp_b = dist
            .allocations
            .iter()
            .find(|x| x.variant_key == "b")
            .unwrap()
            .percentage_bp;
        let bp_a = dist
            .allocations
            .iter()
            .find(|x| x.variant_key == "a")
            .unwrap()
            .percentage_bp;
        // b violated the guardrail → only the floor; a takes the rest.
        assert_eq!(bp_b, 500, "b should be at the exploration floor");
        assert_eq!(bp_a, 9500);
    }

    /// Scalarized point posteriors flow through Thompson deterministically.
    #[test]
    fn scalarized_flows_through_thompson() {
        use crate::experimentation::bandit::types::ObjectiveWeight;
        let m1 = Uuid::from_u128(1);
        let metrics = vec![MetricRewards {
            metric_id: m1,
            goal: GoalDirection::Increase,
            arms: vec![
                ("a".into(), numeric_post(1000, 10.0, 1.0)),
                ("b".into(), numeric_post(1000, 100.0, 1.0)),
            ],
        }];
        let obj = RewardObjective::Scalarized {
            weights: vec![ObjectiveWeight {
                metric_id: m1,
                weight: 1.0,
            }],
        };
        let (arms, goal, _mask) = reward_arms(&obj, &metrics);
        assert_eq!(goal, GoalDirection::Increase);
        let w = thompson_weights(&arms, goal, 1000, 3);
        // b has higher standardized reward → wins (point posterior → argmax).
        let wb = w.iter().find(|(k, _)| k == "b").unwrap().1;
        assert_eq!(wb, 1.0);
    }

    #[test]
    fn allocate_exploitable_excludes_then_merges() {
        let arms = vec![
            BanditArm {
                variant_key: "a".into(),
                posterior: count_post(1000, 100),
            },
            BanditArm {
                variant_key: "b".into(),
                posterior: count_post(1000, 900),
            },
        ];
        // b is the obvious winner but excluded → a (the only eligible arm) wins.
        let w = allocate_exploitable(&arms, &[true, false], |elig| {
            ucb_weights(elig, GoalDirection::Increase, 0.001)
        });
        assert_eq!(w[0], ("a".to_string(), 1.0));
        assert_eq!(w[1], ("b".to_string(), 0.0));
    }

    #[test]
    fn allocate_exploitable_none_eligible_all_zero() {
        let arms = vec![
            BanditArm {
                variant_key: "a".into(),
                posterior: count_post(1000, 100),
            },
            BanditArm {
                variant_key: "b".into(),
                posterior: count_post(1000, 900),
            },
        ];
        let w = allocate_exploitable(&arms, &[false, false], |elig| {
            ucb_weights(elig, GoalDirection::Increase, 0.001)
        });
        assert_eq!(w[0].1, 0.0);
        assert_eq!(w[1].1, 0.0);
    }

    // ── Per-objective posterior surfacing (FR9) ──────────────────────────────

    fn pri_of(ps: &[ObjectivePosterior], role: ObjectiveRole) -> &ObjectivePosterior {
        ps.iter().find(|p| p.role == role).unwrap()
    }

    fn var_of<'a>(p: &'a ObjectivePosterior, key: &str) -> &'a ObjectiveVariantPosterior {
        p.variants.iter().find(|v| v.variant_key == key).unwrap()
    }

    #[test]
    fn objective_posteriors_scalar_surfaces_mean_and_ci() {
        let id = Uuid::from_u128(1);
        let metrics = vec![MetricRewards {
            metric_id: id,
            goal: GoalDirection::Increase,
            arms: vec![
                ("a".into(), count_post(1000, 100)),
                ("b".into(), count_post(1000, 300)),
            ],
        }];
        let obj = RewardObjective::Scalar { metric_id: id };
        let ps = objective_posteriors(&obj, &metrics);
        assert_eq!(ps.len(), 1);
        let p = pri_of(&ps, ObjectiveRole::Scalar);
        assert_eq!(p.metric_id, id);
        let b = var_of(p, "b");
        // ~30% rate, CI brackets the mean and stays in [0,1].
        assert!(b.ci_lower < b.mean && b.mean < b.ci_upper);
        assert!(b.ci_lower >= 0.0 && b.ci_upper <= 1.0);
        assert_eq!(b.n, 1000);
        assert!(!b.guardrail_violated);
    }

    #[test]
    fn objective_posteriors_scalarized_carries_each_weighted_metric() {
        use crate::experimentation::bandit::types::ObjectiveWeight;
        let m1 = Uuid::from_u128(1);
        let m2 = Uuid::from_u128(2);
        let metrics = vec![
            MetricRewards {
                metric_id: m1,
                goal: GoalDirection::Increase,
                arms: vec![("a".into(), numeric_post(1000, 100.0, 1.0))],
            },
            MetricRewards {
                metric_id: m2,
                goal: GoalDirection::Decrease,
                arms: vec![("a".into(), numeric_post(1000, 10.0, 1.0))],
            },
        ];
        let obj = RewardObjective::Scalarized {
            weights: vec![
                ObjectiveWeight {
                    metric_id: m1,
                    weight: 0.7,
                },
                ObjectiveWeight {
                    metric_id: m2,
                    weight: 0.3,
                },
            ],
        };
        let ps = objective_posteriors(&obj, &metrics);
        assert_eq!(ps.len(), 2);
        assert_eq!(ps[0].metric_id, m1);
        assert_eq!(ps[0].role, ObjectiveRole::Weighted(700));
        assert_eq!(ps[1].metric_id, m2);
        assert_eq!(ps[1].role, ObjectiveRole::Weighted(300));
        // Means are the RAW (non goal-normalised) values.
        assert!((var_of(&ps[0], "a").mean - 100.0).abs() < 1e-6);
        assert!((var_of(&ps[1], "a").mean - 10.0).abs() < 1e-6);
    }

    #[test]
    fn objective_posteriors_constrained_flags_guardrail_violation() {
        let primary = Uuid::from_u128(1);
        let guard = Uuid::from_u128(2);
        let metrics = vec![
            MetricRewards {
                metric_id: primary,
                goal: GoalDirection::Increase,
                arms: vec![
                    ("a".into(), numeric_post(1000, 50.0, 1.0)),
                    ("b".into(), numeric_post(1000, 99.0, 1.0)),
                ],
            },
            MetricRewards {
                metric_id: guard,
                goal: GoalDirection::Decrease,
                arms: vec![
                    ("a".into(), numeric_post(1000, 0.01, 0.0001)),
                    ("b".into(), numeric_post(1000, 0.20, 0.0001)),
                ],
            },
        ];
        let obj = RewardObjective::Constrained {
            primary_metric_id: primary,
            constraints: vec![GuardrailConstraint {
                metric_id: guard,
                bound: 0.05,
                direction: ConstraintDirection::AtMost,
            }],
        };
        let ps = objective_posteriors(&obj, &metrics);
        assert_eq!(ps.len(), 2);
        let prim = pri_of(&ps, ObjectiveRole::Primary);
        assert_eq!(prim.metric_id, primary);
        // Primary metric is never flagged as a guardrail violation.
        assert!(!var_of(prim, "b").guardrail_violated);

        let g = pri_of(&ps, ObjectiveRole::Guardrail);
        assert_eq!(g.metric_id, guard);
        // b violates the at-most-0.05 bound; a does not.
        assert!(var_of(g, "b").guardrail_violated);
        assert!(!var_of(g, "a").guardrail_violated);
    }

    #[test]
    fn objective_posteriors_empty_when_no_metrics() {
        let obj = RewardObjective::Scalar {
            metric_id: Uuid::nil(),
        };
        assert!(objective_posteriors(&obj, &[]).is_empty());
    }

    #[test]
    fn ci95_numeric_brackets_mean() {
        let p = numeric_post(1000, 50.0, 100.0);
        let (lo, hi) = p.ci95();
        assert!(lo < 50.0 && 50.0 < hi);
        // SE = sqrt(100/1000) ≈ 0.316; half-width ≈ 0.62.
        assert!((hi - lo - 2.0 * 1.959_964 * (100.0_f64 / 1000.0).sqrt()).abs() < 1e-6);
    }

    #[test]
    fn ci95_conversion_clamped_to_unit() {
        // A near-certain arm: CI stays inside [0,1].
        let p = count_post(10, 10);
        let (lo, hi) = p.ci95();
        assert!(lo >= 0.0 && hi <= 1.0);
    }

    #[test]
    fn allocate_exploitable_length_mismatch_runs_on_all() {
        let arms = vec![BanditArm {
            variant_key: "a".into(),
            posterior: count_post(1000, 100),
        }];
        let w = allocate_exploitable(&arms, &[true, false], |elig| {
            ucb_weights(elig, GoalDirection::Increase, 0.001)
        });
        assert_eq!(w.len(), 1);
        assert_eq!(w[0].1, 1.0);
    }
}
