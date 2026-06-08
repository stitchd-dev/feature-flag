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

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use stitchd_db::ScheduleStatus;
    use uuid::Uuid;

    /// An applier that records the entity-type it was routed and returns a tag.
    struct TaggedApplier(&'static str);
    #[async_trait]
    impl Applier for TaggedApplier {
        async fn apply(&self, _change: &ScheduledChangeRow) -> anyhow::Result<ApplyOutcome> {
            Ok(ApplyOutcome::Skipped(self.0.to_string()))
        }
    }

    fn row(entity_type: &str) -> ScheduledChangeRow {
        let now = Utc::now();
        ScheduledChangeRow {
            id: Uuid::new_v4(),
            entity_type: entity_type.to_string(),
            entity_id: Uuid::new_v4(),
            env_id: Uuid::new_v4(),
            mutation_payload: serde_json::json!({}),
            schedule_kind: "one_shot".to_string(),
            scheduled_at: Some(now),
            rrule: None,
            tz: None,
            next_run_at: Some(now),
            last_run_at: None,
            status: ScheduleStatus::Pending,
            version: 1,
            created_at: now,
            updated_at: now,
            created_by: None,
        }
    }

    fn dispatcher() -> Dispatcher<TaggedApplier, TaggedApplier, TaggedApplier> {
        Dispatcher::new(
            TaggedApplier("flag"),
            TaggedApplier("experiment"),
            TaggedApplier("segment"),
        )
    }

    #[tokio::test]
    async fn dispatch_routes_to_each_entity_applier() {
        let d = dispatcher();
        assert_eq!(
            d.apply(&row("flag")).await.unwrap(),
            ApplyOutcome::Skipped("flag".to_string())
        );
        assert_eq!(
            d.apply(&row("experiment")).await.unwrap(),
            ApplyOutcome::Skipped("experiment".to_string())
        );
        assert_eq!(
            d.apply(&row("segment")).await.unwrap(),
            ApplyOutcome::Skipped("segment".to_string())
        );
    }

    #[tokio::test]
    async fn dispatch_unknown_entity_type_is_failed_not_dropped() {
        let d = dispatcher();
        let outcome = d.apply(&row("widget")).await.unwrap();
        match outcome {
            ApplyOutcome::Failed(detail) => {
                assert!(detail.contains("unsupported entity type"));
                assert!(detail.contains("widget"));
            }
            other => panic!("expected Failed, got {other:?}"),
        }
    }
}
