//! Bandit (adaptive allocation) domain types.
//!
//! These types describe the *configuration* of a bandit experiment — the
//! algorithm, propagation path, reward objective, lifecycle policy and
//! convergence thresholds — and of an autonomous optimization *campaign*.
//!
//! All allocation math is expressed in basis points (sum 10,000) and reuses
//! [`crate::rollout::RolloutDistribution`]; nothing here invents a new
//! allocation representation.
//!
//! Wire format: discriminated unions are serde-`tag`ged (`{"type": "..."}`)
//! to match the REST / protobuf-JSON representation used elsewhere in the
//! codebase.

use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

/// Whether an experiment uses a fixed split or adaptive (bandit) allocation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "lowercase")]
pub enum ExperimentMode {
    /// A classic fixed-split experiment (the historical default).
    #[default]
    Fixed,
    /// An adaptive multi-armed-bandit experiment.
    Bandit,
}

/// The bandit allocation algorithm and its algorithm-specific parameters.
///
/// Serde tag: `type` — e.g. `{"type":"thompson_sampling"}` or
/// `{"type":"epsilon_greedy","epsilon":0.1}`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum BanditAlgorithm {
    /// Thompson sampling (default): Monte-Carlo over the reward posteriors,
    /// weight ∝ probability-best.
    ThompsonSampling,
    /// Epsilon-greedy: `epsilon` of traffic spread across the floor, remainder
    /// to the current best arm. `0 <= epsilon <= 1`.
    EpsilonGreedy {
        /// Exploration fraction in `[0, 1]`.
        epsilon: f64,
    },
    /// Upper-confidence-bound: optimism-under-uncertainty index. `c > 0` is the
    /// exploration coefficient.
    Ucb {
        /// Exploration coefficient (`c > 0`).
        c: f64,
    },
    /// Contextual bandit: reward conditioned on context features. Parameters are
    /// kept minimal for now (a feature list placeholder); the per-context model
    /// rides on the flag snapshot at eval time.
    Contextual(ContextualConfig),
}

/// Minimal contextual-bandit configuration. The feature list names the context
/// attributes the per-context reward model is conditioned on.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ContextualConfig {
    /// Names of the context features the reward model conditions on. May be
    /// empty as a placeholder; populated in later phases.
    #[serde(default)]
    pub features: Vec<String>,
}

/// How recomputed bandit weights reach evaluation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum PropagationMode {
    /// Static-rewrite: each tick rewrites the bound rule's allocation; the eval
    /// path stays static / experiment-unaware (the default, invariant-preserving
    /// path).
    #[default]
    Static,
    /// Real-time: reward-model parameters ride on the flag snapshot and the
    /// variant is sampled per-context at eval time (required for contextual).
    Realtime,
}

/// The operator-selected autonomous lifecycle policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum LifecyclePolicy {
    /// Advisory (default): raise a "ready to commit" badge, take no automated
    /// action.
    #[default]
    Advisory,
    /// Auto-commit: lock allocation to the winner (flag still held).
    AutoCommit,
    /// Auto-rollout: commit to the winner, autonomously stop the experiment, and
    /// promote the winner into the standing rule, releasing the flag lock.
    AutoRollout,
}

/// A single objective metric and its weight within a scalarized reward.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ObjectiveWeight {
    /// The metric definition this weight applies to.
    #[cfg_attr(feature = "openapi", schema(value_type = String, format = Uuid))]
    pub metric_id: Uuid,
    /// The (finite) weight for this metric in the scalarized sum.
    pub weight: f64,
}

/// Direction a guardrail bound is enforced in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum ConstraintDirection {
    /// The metric must stay at or above `bound`.
    AtLeast,
    /// The metric must stay at or below `bound`.
    AtMost,
}

/// A guardrail constraint for a constrained-objective bandit: an arm whose
/// guardrail posterior violates this bound is down-weighted / excluded from
/// exploitation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct GuardrailConstraint {
    /// The guardrail metric definition this constraint applies to.
    #[cfg_attr(feature = "openapi", schema(value_type = String, format = Uuid))]
    pub metric_id: Uuid,
    /// The bound value.
    pub bound: f64,
    /// Whether the metric must stay above or below `bound`.
    pub direction: ConstraintDirection,
}

/// How reward is computed from one or more objective metrics.
///
/// Serde tag: `type` — `scalar` | `scalarized` | `constrained`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RewardObjective {
    /// A single scalar objective metric.
    Scalar {
        /// The objective metric definition.
        #[cfg_attr(feature = "openapi", schema(value_type = String, format = Uuid))]
        metric_id: Uuid,
    },
    /// A scalarized (goal-normalized weighted sum) reward across metrics.
    Scalarized {
        /// The per-metric weights; must be non-empty and finite.
        weights: Vec<ObjectiveWeight>,
    },
    /// A primary objective optimized subject to guardrail constraints.
    Constrained {
        /// The primary objective metric definition.
        #[cfg_attr(feature = "openapi", schema(value_type = String, format = Uuid))]
        primary_metric_id: Uuid,
        /// The guardrail constraints exploitation is subject to.
        constraints: Vec<GuardrailConstraint>,
    },
}

/// Errors raised when validating a [`BanditConfig`] or [`BanditCampaignConfig`].
#[derive(Debug, Clone, PartialEq, Error)]
pub enum BanditConfigError {
    /// `min_exploration_bp` exceeded the sane upper bound.
    #[error("min_exploration_bp {0} must be <= 5000")]
    MinExplorationTooHigh(u32),
    /// A probability threshold was not strictly inside `(0, 1)`.
    #[error("{field} must be in the open interval (0, 1) (got {value})")]
    ThresholdOutOfRange {
        /// The offending field name.
        field: &'static str,
        /// The offending value.
        value: f64,
    },
    /// An algorithm parameter was out of its valid range.
    #[error("algorithm parameter {field} out of range (got {value})")]
    AlgorithmParamOutOfRange {
        /// The offending field name.
        field: &'static str,
        /// The offending value.
        value: f64,
    },
    /// A scalarized reward had no weights.
    #[error("scalarized reward must have at least one objective weight")]
    EmptyScalarizedWeights,
    /// A scalarized weight was not finite.
    #[error("scalarized objective weight must be finite (got {0})")]
    NonFiniteWeight(f64),
    /// A constrained reward bound was not finite.
    #[error("constraint bound must be finite (got {0})")]
    NonFiniteBound(f64),
    /// `max_iterations` was zero.
    #[error("max_iterations must be >= 1")]
    ZeroMaxIterations,
    /// `drift_threshold` was not strictly inside `(0, 1)`.
    #[error("drift_threshold must be in the open interval (0, 1) (got {0})")]
    DriftThresholdOutOfRange(f64),
}

/// Full configuration for a bandit experiment, snapshotted onto the iteration
/// at start (mirrors the sequential-testing settings).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct BanditConfig {
    /// The allocation algorithm and its parameters.
    pub algorithm: BanditAlgorithm,
    /// How recomputed weights reach evaluation.
    #[serde(default)]
    pub propagation_mode: PropagationMode,
    /// Minimum basis points each arm is guaranteed (the exploration floor).
    pub min_exploration_bp: u32,
    /// How reward is computed from the objective metric(s).
    pub objective: RewardObjective,
    /// The autonomous lifecycle policy.
    #[serde(default)]
    pub lifecycle_policy: LifecyclePolicy,
    /// Posterior probability-to-be-best threshold for declaring convergence.
    /// Must be in `(0, 1)`.
    pub convergence_prob_threshold: f64,
}

impl BanditConfig {
    /// Validate the configuration's shape invariants.
    ///
    /// * `min_exploration_bp` must be `<= 5000` (sane floor; a tighter
    ///   `<= 10000 / num_variants` bound is enforced where variant count is
    ///   known).
    /// * `convergence_prob_threshold` must be in the open interval `(0, 1)`.
    /// * algorithm params (`epsilon`, `c`) must be in range.
    /// * a scalarized reward must have at least one finite weight; a constrained
    ///   reward's bounds must be finite.
    pub fn validate(&self) -> Result<(), BanditConfigError> {
        if self.min_exploration_bp > 5000 {
            return Err(BanditConfigError::MinExplorationTooHigh(
                self.min_exploration_bp,
            ));
        }
        validate_open_unit_interval(
            "convergence_prob_threshold",
            self.convergence_prob_threshold,
        )?;

        match &self.algorithm {
            BanditAlgorithm::ThompsonSampling => {}
            BanditAlgorithm::EpsilonGreedy { epsilon } => {
                if !epsilon.is_finite() || *epsilon < 0.0 || *epsilon > 1.0 {
                    return Err(BanditConfigError::AlgorithmParamOutOfRange {
                        field: "epsilon",
                        value: *epsilon,
                    });
                }
            }
            BanditAlgorithm::Ucb { c } => {
                if !c.is_finite() || *c <= 0.0 {
                    return Err(BanditConfigError::AlgorithmParamOutOfRange {
                        field: "c",
                        value: *c,
                    });
                }
            }
            BanditAlgorithm::Contextual(_) => {}
        }

        match &self.objective {
            RewardObjective::Scalar { .. } => {}
            RewardObjective::Scalarized { weights } => {
                if weights.is_empty() {
                    return Err(BanditConfigError::EmptyScalarizedWeights);
                }
                for w in weights {
                    if !w.weight.is_finite() {
                        return Err(BanditConfigError::NonFiniteWeight(w.weight));
                    }
                }
            }
            RewardObjective::Constrained { constraints, .. } => {
                for c in constraints {
                    if !c.bound.is_finite() {
                        return Err(BanditConfigError::NonFiniteBound(c.bound));
                    }
                }
            }
        }
        Ok(())
    }
}

/// Policy controlling how a campaign discovers / carries forward variants when
/// it spawns a successive iteration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum VariantDiscoveryPolicy {
    /// Carry the winner forward as the new control plus any newly-registered
    /// variants (the default perpetual-optimization behaviour).
    #[default]
    WinnerPlusNew,
    /// Carry only the winner forward (no auto-discovery of new variants).
    WinnerOnly,
}

/// A hard ceiling on the resources a campaign may consume across iterations.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct BudgetCap {
    /// Maximum cumulative enrolled units across all campaign iterations.
    /// `None` leaves the dimension uncapped (still bounded by `max_iterations`).
    #[serde(default)]
    pub max_total_units: Option<i64>,
}

/// Configuration for an autonomous optimization campaign, persisted in
/// `bandit_campaigns.config`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct BanditCampaignConfig {
    /// Hard cap on the number of iterations the campaign may spawn (`>= 1`).
    pub max_iterations: u32,
    /// Drift detector threshold: the winner's posterior degrading past this
    /// (relative to a challenger) reopens exploration. Must be in `(0, 1)`.
    pub drift_threshold: f64,
    /// How the campaign carries variants forward when spawning a new iteration.
    #[serde(default)]
    pub variant_discovery: VariantDiscoveryPolicy,
    /// Optional resource ceiling.
    #[serde(default)]
    pub budget_cap: Option<BudgetCap>,
}

impl BanditCampaignConfig {
    /// Validate the campaign configuration's shape invariants.
    pub fn validate(&self) -> Result<(), BanditConfigError> {
        if self.max_iterations == 0 {
            return Err(BanditConfigError::ZeroMaxIterations);
        }
        if !self.drift_threshold.is_finite()
            || self.drift_threshold <= 0.0
            || self.drift_threshold >= 1.0
        {
            return Err(BanditConfigError::DriftThresholdOutOfRange(
                self.drift_threshold,
            ));
        }
        Ok(())
    }
}

/// Validate that `value` is in the open interval `(0, 1)`.
fn validate_open_unit_interval(field: &'static str, value: f64) -> Result<(), BanditConfigError> {
    if !value.is_finite() || value <= 0.0 || value >= 1.0 {
        return Err(BanditConfigError::ThresholdOutOfRange { field, value });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid_config() -> BanditConfig {
        BanditConfig {
            algorithm: BanditAlgorithm::ThompsonSampling,
            propagation_mode: PropagationMode::Static,
            min_exploration_bp: 500,
            objective: RewardObjective::Scalar {
                metric_id: Uuid::nil(),
            },
            lifecycle_policy: LifecyclePolicy::Advisory,
            convergence_prob_threshold: 0.95,
        }
    }

    // ── serde round-trip ──────────────────────────────────────────────────

    #[test]
    fn bandit_config_round_trips() {
        let cfg = valid_config();
        let json = serde_json::to_string(&cfg).unwrap();
        let back: BanditConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(cfg, back);
    }

    #[test]
    fn algorithm_tagged_serde() {
        let json = serde_json::to_string(&BanditAlgorithm::EpsilonGreedy { epsilon: 0.1 }).unwrap();
        assert!(json.contains("\"type\":\"epsilon_greedy\""));
        assert!(json.contains("\"epsilon\":0.1"));
        let back: BanditAlgorithm = serde_json::from_str(&json).unwrap();
        assert_eq!(back, BanditAlgorithm::EpsilonGreedy { epsilon: 0.1 });
    }

    #[test]
    fn thompson_sampling_is_unit_tagged() {
        let json = serde_json::to_string(&BanditAlgorithm::ThompsonSampling).unwrap();
        assert_eq!(json, "{\"type\":\"thompson_sampling\"}");
    }

    #[test]
    fn contextual_round_trips() {
        let alg = BanditAlgorithm::Contextual(ContextualConfig {
            features: vec!["country".into(), "plan".into()],
        });
        let json = serde_json::to_string(&alg).unwrap();
        let back: BanditAlgorithm = serde_json::from_str(&json).unwrap();
        assert_eq!(alg, back);
    }

    #[test]
    fn reward_objective_variants_round_trip() {
        for obj in [
            RewardObjective::Scalar {
                metric_id: Uuid::nil(),
            },
            RewardObjective::Scalarized {
                weights: vec![ObjectiveWeight {
                    metric_id: Uuid::nil(),
                    weight: 0.7,
                }],
            },
            RewardObjective::Constrained {
                primary_metric_id: Uuid::nil(),
                constraints: vec![GuardrailConstraint {
                    metric_id: Uuid::nil(),
                    bound: 0.01,
                    direction: ConstraintDirection::AtMost,
                }],
            },
        ] {
            let json = serde_json::to_string(&obj).unwrap();
            let back: RewardObjective = serde_json::from_str(&json).unwrap();
            assert_eq!(obj, back);
        }
    }

    // ── defaults ──────────────────────────────────────────────────────────

    #[test]
    fn experiment_mode_default_is_fixed() {
        assert_eq!(ExperimentMode::default(), ExperimentMode::Fixed);
    }

    #[test]
    fn experiment_mode_serializes_lowercase() {
        assert_eq!(
            serde_json::to_string(&ExperimentMode::Bandit).unwrap(),
            "\"bandit\""
        );
        assert_eq!(
            serde_json::to_string(&ExperimentMode::Fixed).unwrap(),
            "\"fixed\""
        );
    }

    #[test]
    fn optional_fields_default_on_deserialize() {
        // propagation_mode + lifecycle_policy omitted → defaults applied.
        let json = r#"{
            "algorithm": {"type":"thompson_sampling"},
            "min_exploration_bp": 100,
            "objective": {"type":"scalar","metric_id":"00000000-0000-0000-0000-000000000000"},
            "convergence_prob_threshold": 0.9
        }"#;
        let cfg: BanditConfig = serde_json::from_str(json).unwrap();
        assert_eq!(cfg.propagation_mode, PropagationMode::Static);
        assert_eq!(cfg.lifecycle_policy, LifecyclePolicy::Advisory);
    }

    // ── validation ────────────────────────────────────────────────────────

    #[test]
    fn valid_config_passes() {
        assert!(valid_config().validate().is_ok());
    }

    #[test]
    fn rejects_high_min_exploration() {
        let mut cfg = valid_config();
        cfg.min_exploration_bp = 5001;
        assert_eq!(
            cfg.validate(),
            Err(BanditConfigError::MinExplorationTooHigh(5001))
        );
    }

    #[test]
    fn rejects_threshold_out_of_range() {
        for bad in [0.0, 1.0, -0.1, 1.5, f64::NAN] {
            let mut cfg = valid_config();
            cfg.convergence_prob_threshold = bad;
            assert!(matches!(
                cfg.validate(),
                Err(BanditConfigError::ThresholdOutOfRange { .. })
            ));
        }
    }

    #[test]
    fn rejects_bad_epsilon() {
        let mut cfg = valid_config();
        cfg.algorithm = BanditAlgorithm::EpsilonGreedy { epsilon: 1.5 };
        assert!(matches!(
            cfg.validate(),
            Err(BanditConfigError::AlgorithmParamOutOfRange {
                field: "epsilon",
                ..
            })
        ));
    }

    #[test]
    fn rejects_nonpositive_ucb_c() {
        let mut cfg = valid_config();
        cfg.algorithm = BanditAlgorithm::Ucb { c: 0.0 };
        assert!(matches!(
            cfg.validate(),
            Err(BanditConfigError::AlgorithmParamOutOfRange { field: "c", .. })
        ));
    }

    #[test]
    fn rejects_empty_scalarized_weights() {
        let mut cfg = valid_config();
        cfg.objective = RewardObjective::Scalarized { weights: vec![] };
        assert_eq!(
            cfg.validate(),
            Err(BanditConfigError::EmptyScalarizedWeights)
        );
    }

    #[test]
    fn rejects_nonfinite_weight() {
        let mut cfg = valid_config();
        cfg.objective = RewardObjective::Scalarized {
            weights: vec![ObjectiveWeight {
                metric_id: Uuid::nil(),
                weight: f64::INFINITY,
            }],
        };
        assert!(matches!(
            cfg.validate(),
            Err(BanditConfigError::NonFiniteWeight(_))
        ));
    }

    #[test]
    fn rejects_nonfinite_constraint_bound() {
        let mut cfg = valid_config();
        cfg.objective = RewardObjective::Constrained {
            primary_metric_id: Uuid::nil(),
            constraints: vec![GuardrailConstraint {
                metric_id: Uuid::nil(),
                bound: f64::NAN,
                direction: ConstraintDirection::AtLeast,
            }],
        };
        assert!(matches!(
            cfg.validate(),
            Err(BanditConfigError::NonFiniteBound(_))
        ));
    }

    // ── campaign config ───────────────────────────────────────────────────

    #[test]
    fn campaign_config_round_trips_and_validates() {
        let cfg = BanditCampaignConfig {
            max_iterations: 5,
            drift_threshold: 0.2,
            variant_discovery: VariantDiscoveryPolicy::WinnerPlusNew,
            budget_cap: Some(BudgetCap {
                max_total_units: Some(100_000),
            }),
        };
        let json = serde_json::to_string(&cfg).unwrap();
        let back: BanditCampaignConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(cfg, back);
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn campaign_rejects_zero_max_iterations() {
        let cfg = BanditCampaignConfig {
            max_iterations: 0,
            drift_threshold: 0.2,
            variant_discovery: VariantDiscoveryPolicy::default(),
            budget_cap: None,
        };
        assert_eq!(cfg.validate(), Err(BanditConfigError::ZeroMaxIterations));
    }

    #[test]
    fn campaign_rejects_bad_drift_threshold() {
        let cfg = BanditCampaignConfig {
            max_iterations: 3,
            drift_threshold: 1.0,
            variant_discovery: VariantDiscoveryPolicy::default(),
            budget_cap: None,
        };
        assert!(matches!(
            cfg.validate(),
            Err(BanditConfigError::DriftThresholdOutOfRange(_))
        ));
    }

    #[test]
    fn campaign_budget_cap_optional_on_deserialize() {
        let json = r#"{"max_iterations":3,"drift_threshold":0.1}"#;
        let cfg: BanditCampaignConfig = serde_json::from_str(json).unwrap();
        assert_eq!(cfg.variant_discovery, VariantDiscoveryPolicy::WinnerPlusNew);
        assert_eq!(cfg.budget_cap, None);
    }
}
