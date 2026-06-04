//! Core scheduler logic: claim due changes, apply them, advance state.
//!
//! The wall-clock is injected via the [`Clock`] trait and the apply path via
//! [`Applier`], so [`process_due_changes`] is unit-testable with a controllable
//! clock and a stub applier — no `tokio::time::sleep` dependence.
//!
//! ## Restart-safety / idempotency
//! All due rows are claimed inside one transaction with `FOR UPDATE SKIP LOCKED`
//! (see [`ScheduledChangeRepository::claim_due`]). Each claimed row's apply +
//! run-history append + state advance happen **inside that same transaction**,
//! so the row stays locked for the whole apply and is only released on commit.
//! A concurrent scheduler replica skips locked rows; if this process dies
//! mid-apply the transaction rolls back and the row is re-claimed next tick (a
//! missed tick catches up). One-shot rows transition to a terminal status and a
//! recurring row's `next_run_at` always moves strictly forward, so neither can
//! double-apply.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use stitchd_core::schedule::RecurrenceSpec;
use stitchd_db::{RunOutcome, ScheduleStatus, ScheduledChangeRepository, ScheduledChangeRow};
use tracing::{error, info, warn};

use crate::apply::{Applier, ApplyOutcome};

/// Injectable wall-clock so tests can drive the scheduler deterministically.
pub trait Clock: Send + Sync {
    /// Current instant (UTC).
    fn now(&self) -> DateTime<Utc>;
}

/// Production clock backed by [`Utc::now`].
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }
}

/// Summary of a single scheduler pass (returned for observability + tests).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TickStats {
    /// Rows claimed this pass.
    pub claimed: usize,
    /// Rows whose apply succeeded.
    pub applied: usize,
    /// Rows whose apply was skipped (recoverable, e.g. locked flag).
    pub skipped: usize,
    /// Rows whose apply failed.
    pub failed: usize,
}

/// Run one scheduler pass: claim every change due as of `clock.now()`, apply
/// each, and advance its state.
///
/// Returns the per-pass [`TickStats`]. Repository / transport failures on an
/// individual row are logged and counted but never abort the pass; the row's
/// transaction is rolled back so it is re-attempted next tick.
///
/// # Errors
/// Returns an error only if the initial claim transaction cannot be opened or
/// committed; per-row failures are absorbed.
pub async fn process_due_changes<C: Clock, A: Applier>(
    repo: &ScheduledChangeRepository,
    applier: &A,
    clock: &C,
    batch: i64,
) -> anyhow::Result<TickStats> {
    let now = clock.now();
    let mut stats = TickStats::default();

    let mut tx = repo.begin().await?;
    let due = repo.claim_due(&mut tx, now, batch).await?;
    stats.claimed = due.len();

    for change in &due {
        let outcome = match applier.apply(change).await {
            Ok(o) => o,
            Err(e) => {
                // Unexpected transport / serialization error — treat as a failed
                // run for this row but keep processing the rest of the batch.
                warn!(change_id = %change.id, "apply error: {e}");
                ApplyOutcome::Failed(e.to_string())
            }
        };

        if let Err(e) = record_outcome(repo, &mut tx, change, &outcome, now).await {
            // A DB error while recording for one row should not abort the batch;
            // it will be re-claimed next tick. Log and continue.
            error!(change_id = %change.id, "failed to record run outcome: {e}");
            continue;
        }

        match outcome {
            ApplyOutcome::Applied => stats.applied += 1,
            ApplyOutcome::Skipped(_) => stats.skipped += 1,
            ApplyOutcome::Failed(_) => stats.failed += 1,
        }
    }

    tx.commit().await?;
    if stats.claimed > 0 {
        info!(
            claimed = stats.claimed,
            applied = stats.applied,
            skipped = stats.skipped,
            failed = stats.failed,
            "scheduler tick complete"
        );
    }
    Ok(stats)
}

/// Append the run-history row and advance the change's state inside the claiming
/// transaction, per its kind + the apply outcome.
async fn record_outcome(
    repo: &ScheduledChangeRepository,
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    change: &ScheduledChangeRow,
    outcome: &ApplyOutcome,
    now: DateTime<Utc>,
) -> anyhow::Result<()> {
    let (run_outcome, detail): (RunOutcome, Option<String>) = match outcome {
        ApplyOutcome::Applied => (RunOutcome::Applied, None),
        ApplyOutcome::Skipped(reason) => (RunOutcome::Skipped, Some(reason.clone())),
        ApplyOutcome::Failed(reason) => (RunOutcome::Failed, Some(reason.clone())),
    };
    repo.append_run_tx(tx, change.id, run_outcome, detail.as_deref())
        .await?;

    let is_recurring = change.schedule_kind == "recurring";
    if is_recurring {
        // A recurring change always advances to its next window — even when the
        // apply was skipped (e.g. locked flag) or failed: the spec requires
        // recurring schedules to proceed to the next window (A9). An exhausted
        // recurrence (None) is marked applied by the repo.
        let next = compute_next_recurring(change, now);
        repo.advance_recurring(tx, change.id, now, next).await?;
    } else {
        // One-shot: terminal. Applied → applied; skipped/failed → failed (the
        // run-history row carries the reason / lock sentinel).
        let status = match outcome {
            ApplyOutcome::Applied => ScheduleStatus::Applied,
            ApplyOutcome::Skipped(_) | ApplyOutcome::Failed(_) => ScheduleStatus::Failed,
        };
        repo.finalize_one_shot(tx, change.id, status, now).await?;
    }
    Ok(())
}

/// Recompute a recurring change's next firing instant strictly after `now`
/// using core's DST-correct RRULE math. Returns `None` (→ row marked applied) on
/// an exhausted or unparseable rule; an unparseable rule is logged.
fn compute_next_recurring(
    change: &ScheduledChangeRow,
    now: DateTime<Utc>,
) -> Option<DateTime<Utc>> {
    let rrule = change.rrule.clone()?;
    let tz = change.tz.clone().unwrap_or_else(|| "UTC".to_string());
    let spec = RecurrenceSpec { rrule, tz };
    match spec.next_occurrence(now) {
        Ok(next) => next,
        Err(e) => {
            warn!(change_id = %change.id, "invalid recurrence rule, retiring: {e}");
            None
        }
    }
}

/// A no-op applier used when a real apply path is unavailable for an entity type
/// in a stripped-down configuration. Not used in production wiring.
pub struct NoopApplier;

#[async_trait]
impl Applier for NoopApplier {
    async fn apply(&self, _change: &ScheduledChangeRow) -> anyhow::Result<ApplyOutcome> {
        Ok(ApplyOutcome::Applied)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// A clock pinned to a fixed instant.
    struct FixedClock(DateTime<Utc>);
    impl Clock for FixedClock {
        fn now(&self) -> DateTime<Utc> {
            self.0
        }
    }

    /// An applier that returns a scripted outcome and records the changes it saw.
    struct StubApplier {
        outcome: ApplyOutcome,
        seen: Mutex<Vec<uuid::Uuid>>,
    }
    impl StubApplier {
        fn new(outcome: ApplyOutcome) -> Self {
            Self {
                outcome,
                seen: Mutex::new(Vec::new()),
            }
        }
    }
    #[async_trait]
    impl Applier for StubApplier {
        async fn apply(&self, change: &ScheduledChangeRow) -> anyhow::Result<ApplyOutcome> {
            self.seen.lock().unwrap().push(change.id);
            Ok(self.outcome.clone())
        }
    }

    use sqlx::PgPool;
    use stitchd_db::NewScheduledChange;
    use stitchd_db::ScheduleKind;
    use uuid::Uuid;

    fn one_shot(at: DateTime<Utc>) -> NewScheduledChange {
        NewScheduledChange {
            entity_type: "flag".to_string(),
            entity_id: Uuid::new_v4(),
            env_id: Uuid::new_v4(),
            mutation_payload: serde_json::json!({}),
            schedule_kind: ScheduleKind::OneShot,
            scheduled_at: Some(at),
            rrule: None,
            tz: None,
            next_run_at: Some(at),
            created_by: None,
        }
    }

    fn recurring(next: DateTime<Utc>, rrule: &str) -> NewScheduledChange {
        NewScheduledChange {
            entity_type: "flag".to_string(),
            entity_id: Uuid::new_v4(),
            env_id: Uuid::new_v4(),
            mutation_payload: serde_json::json!({}),
            schedule_kind: ScheduleKind::Recurring,
            scheduled_at: None,
            rrule: Some(rrule.to_string()),
            tz: Some("UTC".to_string()),
            next_run_at: Some(next),
            created_by: None,
        }
    }

    #[sqlx::test(migrations = "../stitchd-db/migrations")]
    async fn one_shot_applied_transitions_to_applied(pool: PgPool) {
        let repo = ScheduledChangeRepository::new(pool);
        let now = Utc::now();
        let row = repo
            .create(&one_shot(now - chrono::Duration::minutes(1)))
            .await
            .unwrap();

        let applier = StubApplier::new(ApplyOutcome::Applied);
        let clock = FixedClock(now);
        let stats = process_due_changes(&repo, &applier, &clock, 100)
            .await
            .unwrap();
        assert_eq!(stats.claimed, 1);
        assert_eq!(stats.applied, 1);

        let after = repo.get(row.id).await.unwrap();
        assert_eq!(after.status, ScheduleStatus::Applied);
        assert!(after.next_run_at.is_none());
        let runs = repo.list_runs(row.id).await.unwrap();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].outcome, RunOutcome::Applied);
    }

    #[sqlx::test(migrations = "../stitchd-db/migrations")]
    async fn one_shot_skipped_transitions_to_failed_with_reason(pool: PgPool) {
        let repo = ScheduledChangeRepository::new(pool);
        let now = Utc::now();
        let row = repo
            .create(&one_shot(now - chrono::Duration::minutes(1)))
            .await
            .unwrap();

        let sentinel = "flag_locked_by_experiment:11111111-1111-1111-1111-111111111111";
        let applier = StubApplier::new(ApplyOutcome::Skipped(sentinel.to_string()));
        let clock = FixedClock(now);
        let stats = process_due_changes(&repo, &applier, &clock, 100)
            .await
            .unwrap();
        assert_eq!(stats.skipped, 1);

        let after = repo.get(row.id).await.unwrap();
        assert_eq!(after.status, ScheduleStatus::Failed);
        let runs = repo.list_runs(row.id).await.unwrap();
        assert_eq!(runs[0].outcome, RunOutcome::Skipped);
        assert_eq!(runs[0].detail.as_deref(), Some(sentinel));
    }

    #[sqlx::test(migrations = "../stitchd-db/migrations")]
    async fn recurring_applied_recomputes_next_run(pool: PgPool) {
        let repo = ScheduledChangeRepository::new(pool);
        // Daily 09:00 UTC; due "now" is set in the past so it is claimed.
        let now = Utc::now();
        let row = repo
            .create(&recurring(
                now - chrono::Duration::minutes(1),
                "DTSTART;TZID=UTC:20200101T090000\nRRULE:FREQ=DAILY",
            ))
            .await
            .unwrap();

        let applier = StubApplier::new(ApplyOutcome::Applied);
        let clock = FixedClock(now);
        process_due_changes(&repo, &applier, &clock, 100)
            .await
            .unwrap();

        let after = repo.get(row.id).await.unwrap();
        assert_eq!(
            after.status,
            ScheduleStatus::Active,
            "recurring stays active"
        );
        let next = after.next_run_at.expect("next computed");
        assert!(next > now, "next_run_at moves strictly forward");
        assert!(after.last_run_at.is_some());
    }

    #[sqlx::test(migrations = "../stitchd-db/migrations")]
    async fn recurring_skipped_still_advances_window(pool: PgPool) {
        // Spec A9: a locked-flag skip on a recurring schedule must still proceed
        // to the next window.
        let repo = ScheduledChangeRepository::new(pool);
        let now = Utc::now();
        let row = repo
            .create(&recurring(
                now - chrono::Duration::minutes(1),
                "DTSTART;TZID=UTC:20200101T090000\nRRULE:FREQ=DAILY",
            ))
            .await
            .unwrap();

        let applier = StubApplier::new(ApplyOutcome::Skipped(
            "flag_locked_by_experiment:x".to_string(),
        ));
        let clock = FixedClock(now);
        process_due_changes(&repo, &applier, &clock, 100)
            .await
            .unwrap();

        let after = repo.get(row.id).await.unwrap();
        assert_eq!(
            after.status,
            ScheduleStatus::Active,
            "recurring proceeds after skip"
        );
        assert!(after.next_run_at.expect("next computed") > now);
        let runs = repo.list_runs(row.id).await.unwrap();
        assert_eq!(runs[0].outcome, RunOutcome::Skipped);
    }

    #[sqlx::test(migrations = "../stitchd-db/migrations")]
    async fn recurring_exhausted_marks_applied(pool: PgPool) {
        let repo = ScheduledChangeRepository::new(pool);
        let now = Utc::now();
        // COUNT=1 daily series anchored in the past: no occurrence after `now`.
        let row = repo
            .create(&recurring(
                now - chrono::Duration::minutes(1),
                "DTSTART;TZID=UTC:20200101T090000\nRRULE:FREQ=DAILY;COUNT=1",
            ))
            .await
            .unwrap();

        let applier = StubApplier::new(ApplyOutcome::Applied);
        let clock = FixedClock(now);
        process_due_changes(&repo, &applier, &clock, 100)
            .await
            .unwrap();

        let after = repo.get(row.id).await.unwrap();
        assert_eq!(after.status, ScheduleStatus::Applied);
        assert!(after.next_run_at.is_none());
    }

    #[sqlx::test(migrations = "../stitchd-db/migrations")]
    async fn not_yet_due_is_not_claimed(pool: PgPool) {
        let repo = ScheduledChangeRepository::new(pool);
        let now = Utc::now();
        repo.create(&one_shot(now + chrono::Duration::hours(1)))
            .await
            .unwrap();

        let applier = StubApplier::new(ApplyOutcome::Applied);
        let clock = FixedClock(now);
        let stats = process_due_changes(&repo, &applier, &clock, 100)
            .await
            .unwrap();
        assert_eq!(stats.claimed, 0);
    }

    #[sqlx::test(migrations = "../stitchd-db/migrations")]
    async fn second_tick_does_not_reapply_one_shot(pool: PgPool) {
        // Idempotency: re-running the pass must not re-fire an applied one-shot.
        let repo = ScheduledChangeRepository::new(pool);
        let now = Utc::now();
        let row = repo
            .create(&one_shot(now - chrono::Duration::minutes(1)))
            .await
            .unwrap();

        let applier = StubApplier::new(ApplyOutcome::Applied);
        let clock = FixedClock(now);
        process_due_changes(&repo, &applier, &clock, 100)
            .await
            .unwrap();
        let stats2 = process_due_changes(&repo, &applier, &clock, 100)
            .await
            .unwrap();
        assert_eq!(stats2.claimed, 0, "applied one-shot is not re-claimed");

        let runs = repo.list_runs(row.id).await.unwrap();
        assert_eq!(runs.len(), 1, "only one run recorded");
    }
}
