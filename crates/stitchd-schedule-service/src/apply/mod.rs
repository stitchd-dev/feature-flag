//! Entity-agnostic apply dispatch seam.
//!
//! The scheduler claims a due [`ScheduledChangeRow`] and dispatches it, keyed on
//! `entity_type`, to the owning service's canonical mutation RPC. Each apply
//! returns an [`ApplyOutcome`] the scheduler records in the run history and uses
//! to decide the row's next state.
//!
//! Phase 3 implemented the `flag` arm ([`flag`]); Phase 5 fills in the
//! `experiment` ([`experiment`]) and `segment` ([`segment`]) arms. An entity type
//! the dispatcher does not recognize returns [`ApplyOutcome::Failed`] with a clear
//! "unsupported entity type" detail so a misrouted change is recorded rather than
//! silently dropped.

pub mod experiment;
pub mod flag;
pub mod segment;

use async_trait::async_trait;
use stitchd_db::ScheduledChangeRow;

/// The result of attempting to apply a due change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApplyOutcome {
    /// The mutation was applied successfully.
    Applied,
    /// The apply was deliberately skipped (recoverable; e.g. the target flag is
    /// locked by an experiment). The detail carries the reason — for a flag lock
    /// the `flag_locked_by_experiment:<uuid>` sentinel. For a **recurring**
    /// change the scheduler still advances to the next window.
    Skipped(String),
    /// The apply failed. The detail carries the error.
    Failed(String),
}

/// Dispatches a due change to the owning service's canonical mutation RPC.
#[async_trait]
pub trait Applier: Send + Sync {
    /// Apply `change` and report the outcome. Implementations MUST NOT return an
    /// `Err` for a domain skip (e.g. an experiment-locked flag) — those map to
    /// [`ApplyOutcome::Skipped`]; `Err` is reserved for unexpected transport /
    /// serialization failures the caller logs and treats as a failed run.
    async fn apply(&self, change: &ScheduledChangeRow) -> anyhow::Result<ApplyOutcome>;
}

/// Entity-type router: the top-level [`Applier`] the scheduler loop calls. Keyed
/// on `change.entity_type`, it forwards to the owning per-entity applier
/// (`flag` → [`flag::FlagApplier`], `experiment` → [`experiment::ExperimentApplier`],
/// `segment` → [`segment::SegmentApplier`]).
pub struct Dispatcher<F: Applier, E: Applier, S: Applier> {
    flag: F,
    experiment: E,
    segment: S,
}

impl<F: Applier, E: Applier, S: Applier> Dispatcher<F, E, S> {
    /// Construct a dispatcher over the per-entity appliers.
    pub const fn new(flag: F, experiment: E, segment: S) -> Self {
        Self {
            flag,
            experiment,
            segment,
        }
    }
}

#[async_trait]
impl<F: Applier, E: Applier, S: Applier> Applier for Dispatcher<F, E, S> {
    async fn apply(&self, change: &ScheduledChangeRow) -> anyhow::Result<ApplyOutcome> {
        match change.entity_type.as_str() {
            "flag" => self.flag.apply(change).await,
            "experiment" => self.experiment.apply(change).await,
            "segment" => self.segment.apply(change).await,
            other => Ok(ApplyOutcome::Failed(format!(
                "unsupported entity type '{other}'"
            ))),
        }
    }
}
