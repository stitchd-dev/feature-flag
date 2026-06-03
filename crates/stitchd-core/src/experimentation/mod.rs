//! Experimentation domain types.
//!
//! An [`Experiment`] is bound to a specific flag rule and progresses through
//! a lifecycle of [`ExperimentStatus`] values. Each transition **into**
//! [`ExperimentStatus::Running`] creates a new [`ExperimentIteration`] that
//! captures a snapshot of the experiment configuration at that moment.

pub mod stats;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::id::{
    EnvironmentId, ExclusionGroupId, ExperimentId, ExperimentIterationId, FlagId, MetricId, RuleId,
};
use crate::rollout::RolloutDistribution;

/// A mutually-exclusive experiment group within an environment.
///
/// Members of the same group are placed into disjoint basis-point bucket ranges
/// (carved out of `[0, 10000)`) using a shared [`salt`](ExclusionGroup::salt),
/// guaranteeing that a context enrolled in one member experiment cannot be
/// enrolled in any other member of the same group.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ExclusionGroup {
    /// Unique identifier.
    pub id: ExclusionGroupId,
    /// The environment this group belongs to.
    pub environment_id: EnvironmentId,
    /// Human-readable name.
    pub name: String,
    /// Optional description.
    pub description: Option<String>,
    /// Salt shared by all members, used to derive each context's group bucket.
    pub salt: String,
    /// The group's diversion (randomization) unit context type, e.g. `"user"`.
    /// Every member experiment must randomize on this unit so a given context
    /// hashes to ONE shared exclusion bucket across the group's flags — this
    /// invariant is what makes mutual exclusion hold. Immutable after creation.
    pub unit_context_type: String,
    /// Computed view: total basis points currently allocated to members
    /// (sum of all member bucket ranges).
    pub allocated_bp: u32,
    /// Computed view: free basis points remaining (`10000 - allocated_bp`).
    pub free_bp: u32,
    /// Optimistic-concurrency version counter.
    pub version: i64,
}

/// Errors that can occur during experiment operations.
#[derive(Debug, Error, PartialEq)]
pub enum ExperimentError {
    /// The requested status transition is not valid.
    #[error("Invalid status transition from {from:?} to {to:?}")]
    InvalidTransition {
        from: ExperimentStatus,
        to: ExperimentStatus,
    },
    /// Mutation is not allowed while the experiment is running (409 semantics).
    #[error("Experiment is running; mutations are not allowed")]
    MutationWhileRunning,
}

/// The lifecycle status of an experiment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, sqlx::Type)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
#[sqlx(type_name = "text", rename_all = "snake_case")]
pub enum ExperimentStatus {
    /// The experiment has been created but not yet started.
    Draft,
    /// The experiment is actively running and enrolling traffic.
    Running,
    /// The experiment has been temporarily paused.
    Paused,
    /// The experiment has been permanently stopped.
    Stopped,
}

impl ExperimentStatus {
    /// Returns the set of valid next statuses from this status.
    ///
    /// Valid transitions:
    /// - `draft → running`
    /// - `running → paused`
    /// - `running → stopped`
    /// - `paused → running`
    /// - `paused → stopped`
    /// - `stopped → running` (restart)
    pub fn allowed_transitions(&self) -> &'static [ExperimentStatus] {
        match self {
            ExperimentStatus::Draft => &[ExperimentStatus::Running],
            ExperimentStatus::Running => &[ExperimentStatus::Paused, ExperimentStatus::Stopped],
            ExperimentStatus::Paused => &[ExperimentStatus::Running, ExperimentStatus::Stopped],
            ExperimentStatus::Stopped => &[ExperimentStatus::Running],
        }
    }

    /// Returns `true` if transitioning to `to` from this status is valid.
    pub fn can_transition_to(&self, to: ExperimentStatus) -> bool {
        self.allowed_transitions().contains(&to)
    }
}

/// Returns `true` when the transition causes a new iteration to be created.
///
/// A new iteration is created on every transition **into** `running`.
pub fn creates_iteration(from: ExperimentStatus, to: ExperimentStatus) -> bool {
    to == ExperimentStatus::Running
        && matches!(
            from,
            ExperimentStatus::Draft | ExperimentStatus::Paused | ExperimentStatus::Stopped
        )
}

/// Validates that a mutation is allowed given the current experiment status.
///
/// Returns `Err(ExperimentError::MutationWhileRunning)` if `status == Running`.
pub fn validate_mutation(status: ExperimentStatus) -> Result<(), ExperimentError> {
    if status == ExperimentStatus::Running {
        Err(ExperimentError::MutationWhileRunning)
    } else {
        Ok(())
    }
}

/// Validates a status transition, returning `Ok(())` if valid.
pub fn validate_transition(
    from: ExperimentStatus,
    to: ExperimentStatus,
) -> Result<(), ExperimentError> {
    if from.can_transition_to(to) {
        Ok(())
    } else {
        Err(ExperimentError::InvalidTransition { from, to })
    }
}

/// An experiment record stored in the database.
///
/// An experiment is scoped to an environment and bound to a specific flag rule.
/// Soft-deleted experiments retain their record with `deleted_at` set.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Experiment {
    /// Unique identifier.
    pub id: ExperimentId,
    /// The environment this experiment belongs to.
    pub environment_id: EnvironmentId,
    /// The flag this experiment is bound to. Always populated — either the
    /// parent of `flag_rule_id` (when rule-bound) or chosen directly
    /// (when `targets_default_rule == true`).
    pub flag_id: FlagId,
    /// Human-readable key of the bound flag (resolved via JOIN; `None` when
    /// not available, e.g. when constructed from partial data).
    pub flag_key: Option<String>,
    /// Variant keys for the bound flag (resolved via JOIN on the variants
    /// table; empty when not populated, e.g. in test fixtures or partial data).
    pub variant_keys: Vec<String>,
    /// The flag rule this experiment is bound to, or `None` when the
    /// experiment binds to the flag's default-rule fall-through.
    pub flag_rule_id: Option<RuleId>,
    /// `true` when the experiment binds to the flag's default-rule
    /// percentage-distribution fall-through. XOR with `flag_rule_id`:
    /// exactly one of `flag_rule_id`/`targets_default_rule=true` is set.
    pub targets_default_rule: bool,
    /// Human-readable name.
    pub name: String,
    /// Optional description of the experiment.
    pub description: Option<String>,
    /// Optional hypothesis statement.
    pub hypothesis: Option<String>,
    /// References to `metric_definitions` rows used as this experiment's metrics
    /// (at least one required). Phase 7 cutover replaced raw `metric_keys`
    /// (event-key strings) with metric-definition UUIDs.
    pub metric_ids: Vec<MetricId>,
    /// References to `metric_definitions` rows treated as guardrails
    /// (computed identically to primaries but surfaced separately in the
    /// results UI).
    pub guardrail_metric_ids: Vec<MetricId>,
    /// Percentage of rule-matched contexts to enrol (0.1% granularity, default 100%).
    pub traffic_allocation: f64,
    /// Optional informational minimum sample size guardrail.
    pub min_sample_size: Option<i64>,
    /// CUPED pre-period window in days; `0` disables variance reduction.
    pub pre_period_days: u32,
    /// Analysis unit context types (e.g. `["user"]` or `["user","account"]`).
    /// At least one entry is required; validated against `context_type_registry`.
    pub unit_context_types: Vec<String>,
    /// Optional scheduled start time (persisted; enforcement out of scope).
    pub scheduled_start_at: Option<DateTime<Utc>>,
    /// Optional scheduled end time (persisted; enforcement out of scope).
    pub scheduled_end_at: Option<DateTime<Utc>>,
    /// Current lifecycle status.
    pub status: ExperimentStatus,
    /// When this record was created.
    pub created_at: DateTime<Utc>,
    /// When this record was last modified.
    pub updated_at: DateTime<Utc>,
    /// Set when the experiment is soft-deleted; `None` while active.
    pub deleted_at: Option<DateTime<Utc>>,
    /// Optimistic-concurrency version counter.
    pub version: i64,
    /// Exclusion-group membership. `None` when ungrouped.
    #[serde(default)]
    pub exclusion_group_id: Option<ExclusionGroupId>,
    /// Inclusive lower bound of the disjoint bucket range allocated within the
    /// exclusion group, in basis points. `None` when ungrouped.
    #[serde(default)]
    pub group_bucket_lo: Option<u16>,
    /// Exclusive upper bound of the disjoint bucket range allocated within the
    /// exclusion group, in basis points. `None` when ungrouped.
    #[serde(default)]
    pub group_bucket_hi: Option<u16>,
    /// Whether sequential (always-valid) testing is enabled for this experiment.
    #[serde(default)]
    pub sequential_testing_enabled: bool,
    /// Target type-I error rate (α) for the always-valid test. `0 < α < 1`.
    #[serde(default = "default_sequential_alpha")]
    pub sequential_alpha: f64,
    /// Mixing variance (τ²) for the mSPRT mixture. `None` = auto-derive at
    /// compute time; when set it must be `> 0`.
    #[serde(default)]
    pub sequential_tau_squared: Option<f64>,
    /// Minimum per-variant sample size before the sequential test is allowed
    /// to declare significance. `>= 0`.
    #[serde(default = "default_sequential_min_sample_size")]
    pub sequential_min_sample_size: i64,
}

/// Default α for sequential testing when not explicitly configured.
/// Matches the Postgres column default (`0.05`).
fn default_sequential_alpha() -> f64 {
    0.05
}

/// Default minimum sample size for sequential testing when not explicitly
/// configured. Matches the Postgres column default (`100`).
fn default_sequential_min_sample_size() -> i64 {
    100
}

/// A snapshot of an experiment configuration at the start of a run period.
///
/// A new iteration is created on each transition **into** `running`.
/// Once `ended_at` is set the iteration is immutable.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ExperimentIteration {
    /// Unique identifier.
    pub id: ExperimentIterationId,
    /// The experiment this iteration belongs to.
    pub experiment_id: ExperimentId,
    /// Snapshot of the parent experiment's `flag_id` at iteration start
    /// (denormalised for the attribution + assignment hot paths).
    pub flag_id: FlagId,
    /// Sequential iteration number within the experiment (1-based).
    pub iteration_number: i32,
    /// When this iteration started (i.e. when the transition to running occurred).
    pub started_at: DateTime<Utc>,
    /// When this iteration ended; `None` while the experiment is still running.
    pub ended_at: Option<DateTime<Utc>>,
    /// Snapshot of metric definition IDs at the moment this iteration started.
    pub metric_ids: Vec<MetricId>,
    /// Snapshot of guardrail metric IDs at iteration start.
    pub guardrail_metric_ids: Vec<MetricId>,
    /// Snapshot of traffic allocation at the moment this iteration started.
    pub traffic_allocation: f64,
    /// Snapshot of minimum sample size at the moment this iteration started.
    pub min_sample_size: Option<i64>,
    /// Snapshot of `targets_default_rule` at iteration start.
    pub targets_default_rule: bool,
    /// Snapshot of `pre_period_days` (CUPED) at iteration start.
    pub pre_period_days: u32,
    /// Snapshot of analysis unit context types at iteration start.
    pub unit_context_types: Vec<String>,
    /// Snapshot of the parent flag's `default_rule_distribution` at iteration
    /// start — `Some` only when `targets_default_rule == true`; preserved here
    /// so SRM's expected-allocation calculation does not depend on the live
    /// flag row.
    pub default_rule_distribution: Option<RolloutDistribution>,
    /// Snapshot of exclusion-group membership at iteration start. `None` when
    /// ungrouped.
    #[serde(default)]
    pub exclusion_group_id: Option<ExclusionGroupId>,
    /// Snapshot of the inclusive lower bound of the disjoint bucket range
    /// allocated within the exclusion group, in basis points. `None` when
    /// ungrouped.
    #[serde(default)]
    pub group_bucket_lo: Option<u16>,
    /// Snapshot of the exclusive upper bound of the disjoint bucket range
    /// allocated within the exclusion group, in basis points. `None` when
    /// ungrouped.
    #[serde(default)]
    pub group_bucket_hi: Option<u16>,
    /// Snapshot of `sequential_testing_enabled` at iteration start.
    #[serde(default)]
    pub sequential_testing_enabled: bool,
    /// Snapshot of `sequential_alpha` at iteration start.
    #[serde(default = "default_sequential_alpha")]
    pub sequential_alpha: f64,
    /// Snapshot of `sequential_tau_squared` at iteration start. `None` =
    /// auto-derive at compute time.
    #[serde(default)]
    pub sequential_tau_squared: Option<f64>,
    /// Snapshot of `sequential_min_sample_size` at iteration start.
    #[serde(default = "default_sequential_min_sample_size")]
    pub sequential_min_sample_size: i64,
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── ExperimentStatus::allowed_transitions ────────────────────────────────

    #[test]
    fn draft_can_only_transition_to_running() {
        let transitions = ExperimentStatus::Draft.allowed_transitions();
        assert_eq!(transitions, &[ExperimentStatus::Running]);
    }

    #[test]
    fn running_can_transition_to_paused_and_stopped() {
        let transitions = ExperimentStatus::Running.allowed_transitions();
        assert!(transitions.contains(&ExperimentStatus::Paused));
        assert!(transitions.contains(&ExperimentStatus::Stopped));
        assert_eq!(transitions.len(), 2);
    }

    #[test]
    fn paused_can_transition_to_running_and_stopped() {
        let transitions = ExperimentStatus::Paused.allowed_transitions();
        assert!(transitions.contains(&ExperimentStatus::Running));
        assert!(transitions.contains(&ExperimentStatus::Stopped));
        assert_eq!(transitions.len(), 2);
    }

    #[test]
    fn stopped_can_only_transition_to_running() {
        let transitions = ExperimentStatus::Stopped.allowed_transitions();
        assert_eq!(transitions, &[ExperimentStatus::Running]);
    }

    // ── can_transition_to ────────────────────────────────────────────────────

    #[test]
    fn draft_cannot_transition_to_paused() {
        assert!(!ExperimentStatus::Draft.can_transition_to(ExperimentStatus::Paused));
    }

    #[test]
    fn draft_cannot_transition_to_stopped() {
        assert!(!ExperimentStatus::Draft.can_transition_to(ExperimentStatus::Stopped));
    }

    #[test]
    fn draft_cannot_transition_to_draft() {
        assert!(!ExperimentStatus::Draft.can_transition_to(ExperimentStatus::Draft));
    }

    #[test]
    fn running_cannot_transition_to_draft() {
        assert!(!ExperimentStatus::Running.can_transition_to(ExperimentStatus::Draft));
    }

    #[test]
    fn running_cannot_transition_to_running() {
        assert!(!ExperimentStatus::Running.can_transition_to(ExperimentStatus::Running));
    }

    // ── validate_transition ──────────────────────────────────────────────────

    #[test]
    fn valid_transition_draft_to_running_returns_ok() {
        assert!(validate_transition(ExperimentStatus::Draft, ExperimentStatus::Running).is_ok());
    }

    #[test]
    fn valid_transition_running_to_paused_returns_ok() {
        assert!(validate_transition(ExperimentStatus::Running, ExperimentStatus::Paused).is_ok());
    }

    #[test]
    fn valid_transition_running_to_stopped_returns_ok() {
        assert!(validate_transition(ExperimentStatus::Running, ExperimentStatus::Stopped).is_ok());
    }

    #[test]
    fn valid_transition_paused_to_running_returns_ok() {
        assert!(validate_transition(ExperimentStatus::Paused, ExperimentStatus::Running).is_ok());
    }

    #[test]
    fn valid_transition_paused_to_stopped_returns_ok() {
        assert!(validate_transition(ExperimentStatus::Paused, ExperimentStatus::Stopped).is_ok());
    }

    #[test]
    fn valid_transition_stopped_to_running_returns_ok() {
        assert!(validate_transition(ExperimentStatus::Stopped, ExperimentStatus::Running).is_ok());
    }

    #[test]
    fn invalid_transition_draft_to_paused_returns_err() {
        let result = validate_transition(ExperimentStatus::Draft, ExperimentStatus::Paused);
        assert_eq!(
            result,
            Err(ExperimentError::InvalidTransition {
                from: ExperimentStatus::Draft,
                to: ExperimentStatus::Paused,
            })
        );
    }

    #[test]
    fn invalid_transition_draft_to_stopped_returns_err() {
        let result = validate_transition(ExperimentStatus::Draft, ExperimentStatus::Stopped);
        assert_eq!(
            result,
            Err(ExperimentError::InvalidTransition {
                from: ExperimentStatus::Draft,
                to: ExperimentStatus::Stopped,
            })
        );
    }

    #[test]
    fn invalid_transition_running_to_draft_returns_err() {
        let result = validate_transition(ExperimentStatus::Running, ExperimentStatus::Draft);
        assert!(matches!(
            result,
            Err(ExperimentError::InvalidTransition { .. })
        ));
    }

    // ── creates_iteration ────────────────────────────────────────────────────

    #[test]
    fn draft_to_running_creates_iteration() {
        assert!(creates_iteration(
            ExperimentStatus::Draft,
            ExperimentStatus::Running
        ));
    }

    #[test]
    fn paused_to_running_creates_iteration() {
        assert!(creates_iteration(
            ExperimentStatus::Paused,
            ExperimentStatus::Running
        ));
    }

    #[test]
    fn stopped_to_running_creates_iteration() {
        assert!(creates_iteration(
            ExperimentStatus::Stopped,
            ExperimentStatus::Running
        ));
    }

    #[test]
    fn running_to_paused_does_not_create_iteration() {
        assert!(!creates_iteration(
            ExperimentStatus::Running,
            ExperimentStatus::Paused
        ));
    }

    #[test]
    fn running_to_stopped_does_not_create_iteration() {
        assert!(!creates_iteration(
            ExperimentStatus::Running,
            ExperimentStatus::Stopped
        ));
    }

    #[test]
    fn draft_to_paused_does_not_create_iteration() {
        assert!(!creates_iteration(
            ExperimentStatus::Draft,
            ExperimentStatus::Paused
        ));
    }

    // ── validate_mutation ────────────────────────────────────────────────────

    #[test]
    fn mutation_allowed_when_draft() {
        assert!(validate_mutation(ExperimentStatus::Draft).is_ok());
    }

    #[test]
    fn mutation_allowed_when_paused() {
        assert!(validate_mutation(ExperimentStatus::Paused).is_ok());
    }

    #[test]
    fn mutation_allowed_when_stopped() {
        assert!(validate_mutation(ExperimentStatus::Stopped).is_ok());
    }

    #[test]
    fn mutation_rejected_when_running() {
        assert_eq!(
            validate_mutation(ExperimentStatus::Running),
            Err(ExperimentError::MutationWhileRunning)
        );
    }

    // ── Struct construction sanity checks ────────────────────────────────────

    #[test]
    fn experiment_struct_can_be_constructed() {
        let exp = Experiment {
            id: ExperimentId::new(),
            environment_id: EnvironmentId::new(),
            flag_id: FlagId::new(),
            flag_key: None,
            variant_keys: vec![],
            flag_rule_id: Some(RuleId::new()),
            targets_default_rule: false,
            name: "My Experiment".to_string(),
            description: Some("A/B test of checkout flow".to_string()),
            hypothesis: Some("Variant B increases conversion".to_string()),
            metric_ids: vec![MetricId::new()],
            guardrail_metric_ids: vec![],
            traffic_allocation: 100.0,
            min_sample_size: Some(1000),
            pre_period_days: 0,
            unit_context_types: vec!["user".into()],
            scheduled_start_at: None,
            scheduled_end_at: None,
            status: ExperimentStatus::Draft,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            deleted_at: None,
            version: 1,
            exclusion_group_id: None,
            group_bucket_lo: None,
            group_bucket_hi: None,
            sequential_testing_enabled: false,
            sequential_alpha: 0.05,
            sequential_tau_squared: None,
            sequential_min_sample_size: 100,
        };
        assert_eq!(exp.status, ExperimentStatus::Draft);
        assert_eq!(exp.metric_ids.len(), 1);
        assert!(exp.flag_rule_id.is_some());
        assert!(!exp.targets_default_rule);
    }

    #[test]
    fn experiment_iteration_struct_can_be_constructed() {
        let iter = ExperimentIteration {
            id: ExperimentIterationId::new(),
            experiment_id: ExperimentId::new(),
            flag_id: FlagId::new(),
            iteration_number: 1,
            started_at: Utc::now(),
            ended_at: None,
            metric_ids: vec![MetricId::new()],
            guardrail_metric_ids: vec![],
            traffic_allocation: 50.0,
            min_sample_size: None,
            targets_default_rule: false,
            pre_period_days: 0,
            unit_context_types: vec!["user".into()],
            default_rule_distribution: None,
            exclusion_group_id: None,
            group_bucket_lo: None,
            group_bucket_hi: None,
            sequential_testing_enabled: false,
            sequential_alpha: 0.05,
            sequential_tau_squared: None,
            sequential_min_sample_size: 100,
        };
        assert_eq!(iter.iteration_number, 1);
        assert!(iter.ended_at.is_none());
    }

    // ── Exclusion-group field serde back-compat ──────────────────────────────

    #[test]
    fn experiment_deserializes_without_exclusion_fields() {
        // Legacy serialized JSON lacking the 3 new fields → None.
        let json = serde_json::to_string(&Experiment {
            id: ExperimentId::new(),
            environment_id: EnvironmentId::new(),
            flag_id: FlagId::new(),
            flag_key: None,
            variant_keys: vec![],
            flag_rule_id: Some(RuleId::new()),
            targets_default_rule: false,
            name: "exp".to_string(),
            description: None,
            hypothesis: None,
            metric_ids: vec![MetricId::new()],
            guardrail_metric_ids: vec![],
            traffic_allocation: 100.0,
            min_sample_size: None,
            pre_period_days: 0,
            unit_context_types: vec!["user".into()],
            scheduled_start_at: None,
            scheduled_end_at: None,
            status: ExperimentStatus::Draft,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            deleted_at: None,
            version: 1,
            exclusion_group_id: None,
            group_bucket_lo: None,
            group_bucket_hi: None,
            sequential_testing_enabled: false,
            sequential_alpha: 0.05,
            sequential_tau_squared: None,
            sequential_min_sample_size: 100,
        })
        .unwrap();
        // Strip the new fields to simulate a legacy payload.
        let mut value: serde_json::Value = serde_json::from_str(&json).unwrap();
        let obj = value.as_object_mut().unwrap();
        obj.remove("exclusion_group_id");
        obj.remove("group_bucket_lo");
        obj.remove("group_bucket_hi");
        let exp: Experiment = serde_json::from_value(value).unwrap();
        assert!(exp.exclusion_group_id.is_none());
        assert!(exp.group_bucket_lo.is_none());
        assert!(exp.group_bucket_hi.is_none());
    }

    #[test]
    fn experiment_iteration_deserializes_without_exclusion_fields() {
        let json = serde_json::to_string(&ExperimentIteration {
            id: ExperimentIterationId::new(),
            experiment_id: ExperimentId::new(),
            flag_id: FlagId::new(),
            iteration_number: 1,
            started_at: Utc::now(),
            ended_at: None,
            metric_ids: vec![MetricId::new()],
            guardrail_metric_ids: vec![],
            traffic_allocation: 100.0,
            min_sample_size: None,
            targets_default_rule: false,
            pre_period_days: 0,
            unit_context_types: vec!["user".into()],
            default_rule_distribution: None,
            exclusion_group_id: None,
            group_bucket_lo: None,
            group_bucket_hi: None,
            sequential_testing_enabled: false,
            sequential_alpha: 0.05,
            sequential_tau_squared: None,
            sequential_min_sample_size: 100,
        })
        .unwrap();
        let mut value: serde_json::Value = serde_json::from_str(&json).unwrap();
        let obj = value.as_object_mut().unwrap();
        obj.remove("exclusion_group_id");
        obj.remove("group_bucket_lo");
        obj.remove("group_bucket_hi");
        let iter: ExperimentIteration = serde_json::from_value(value).unwrap();
        assert!(iter.exclusion_group_id.is_none());
        assert!(iter.group_bucket_lo.is_none());
        assert!(iter.group_bucket_hi.is_none());
    }
}
