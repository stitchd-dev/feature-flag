//! Bandit (adaptive & autonomous allocation) domain model.
//!
//! A bandit is a *mode* on the existing [`Experiment`](super::Experiment)
//! entity: it reuses the flag binding, metrics, exclusion groups, whole-flag
//! lock and the Bayesian stats core, and shifts traffic toward better-performing
//! variants instead of holding a fixed split.
//!
//! This module defines the *configuration* surface ([`BanditConfig`],
//! [`BanditCampaignConfig`] and friends) and the pure-math allocation core:
//! the [`thompson`], [`epsilon`] and [`ucb`] allocators (over the Bayesian
//! posteriors), the [`allocation`] basis-point normaliser with a min-exploration
//! floor, and the multi-objective [`reward`] combiner.

pub mod allocation;
pub mod epsilon;
pub mod reward;
pub mod thompson;
pub mod types;
pub mod ucb;

pub use allocation::{NormalizeError, normalize_to_distribution};
pub use epsilon::epsilon_greedy_weights;
pub use reward::{
    CombinedArm, MetricRewards, allocate_exploitable, apply_exploitable_mask, combined_rewards,
    reward_arms,
};
pub use thompson::{BanditArm, GoalDirection, RewardPosterior, thompson_weights};
pub use types::{
    BanditAlgorithm, BanditCampaignConfig, BanditConfig, BanditConfigError, BudgetCap,
    ConstraintDirection, ContextualConfig, ExperimentMode, GuardrailConstraint, LifecyclePolicy,
    ObjectiveWeight, PropagationMode, RewardObjective, VariantDiscoveryPolicy,
};
pub use ucb::ucb_weights;
