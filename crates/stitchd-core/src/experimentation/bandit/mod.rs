//! Bandit (adaptive & autonomous allocation) domain model.
//!
//! A bandit is a *mode* on the existing [`Experiment`](super::Experiment)
//! entity: it reuses the flag binding, metrics, exclusion groups, whole-flag
//! lock and the Bayesian stats core, and shifts traffic toward better-performing
//! variants instead of holding a fixed split.
//!
//! This module currently defines the *configuration* surface
//! ([`BanditConfig`], [`BanditCampaignConfig`] and friends). Allocation /
//! sampling logic lands in later phases.

pub mod epsilon;
pub mod thompson;
pub mod types;
pub mod ucb;

pub use epsilon::epsilon_greedy_weights;
pub use thompson::{BanditArm, GoalDirection, RewardPosterior, thompson_weights};
pub use types::{
    BanditAlgorithm, BanditCampaignConfig, BanditConfig, BanditConfigError, BudgetCap,
    ConstraintDirection, ContextualConfig, ExperimentMode, GuardrailConstraint, LifecyclePolicy,
    ObjectiveWeight, PropagationMode, RewardObjective, VariantDiscoveryPolicy,
};
pub use ucb::ucb_weights;
