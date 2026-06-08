//! Scheduled-change domain types and DST-aware recurrence math.
//!
//! A scheduled change applies a mutation to a flag, segment, or experiment at a
//! future time — either [one-shot](ScheduleKind::OneShot) (a single instant) or
//! [recurring](ScheduleKind::Recurring) ([`RecurrenceSpec`], an RFC-5545 RRULE
//! evaluated in an IANA timezone). All instants are canonical UTC; the IANA zone
//! is retained so the recurring wall-clock time tracks DST transitions.
//!
//! These are pure types plus [`RecurrenceSpec::next_occurrence`], a deterministic
//! recurrence calculation with no I/O. Persistence, dispatch, and the scheduler
//! loop live in `stitchd-db` / `stitchd-schedule-service` (Phase 3).

use crate::id::{EnvironmentId, FlagId};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Whether a scheduled change fires once or repeats.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum ScheduleKind {
    /// Fire exactly once at a specific instant.
    OneShot,
    /// Fire repeatedly per a [`RecurrenceSpec`].
    Recurring,
}

/// Lifecycle status of a scheduled change. One-shot: `Pending → Applied |
/// Failed | Cancelled`. Recurring: `Active ⇄ Paused`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum ScheduleStatus {
    /// One-shot, not yet fired.
    Pending,
    /// Recurring, currently firing.
    Active,
    /// Recurring, temporarily suspended.
    Paused,
    /// One-shot, fired successfully.
    Applied,
    /// One-shot, fired but the apply failed terminally.
    Failed,
    /// Cancelled before firing.
    Cancelled,
}

/// A recurring schedule: an RFC-5545 recurrence string evaluated in an IANA
/// timezone.
///
/// `rrule` is the full RFC-5545 body including a `DTSTART` line that anchors the
/// series wall-clock time and carries the `TZID`, e.g.:
///
/// ```text
/// DTSTART;TZID=America/New_York:20260301T090000
/// RRULE:FREQ=WEEKLY;BYDAY=MO,TU,WE,TH,FR
/// ```
///
/// `tz` records the IANA zone name redundantly for display / validation; the
/// authoritative zone for the math is the `TZID` embedded in `rrule`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct RecurrenceSpec {
    /// RFC-5545 `DTSTART` + `RRULE` body.
    pub rrule: String,
    /// IANA timezone name (e.g. `America/New_York`).
    pub tz: String,
}

/// Errors computing a recurrence occurrence.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RecurrenceError {
    /// The RRULE string could not be parsed or validated.
    #[error("invalid recurrence rule: {0}")]
    InvalidRule(String),
}

impl RecurrenceSpec {
    /// Returns the first occurrence strictly after `after`, or `None` when the
    /// rule has no further occurrences (e.g. a `COUNT`/`UNTIL`-bounded series
    /// that has run out).
    ///
    /// The result is in canonical UTC. Because the underlying RRULE carries an
    /// IANA `TZID`, recurring wall-clock times are resolved in that zone, so the
    /// returned UTC instant shifts correctly across DST boundaries (e.g. a
    /// "09:00 America/New_York" series is 13:00 UTC under EST and 13:00→13:00
    /// stays 09:00 *local* across the spring-forward, i.e. 14:00 UTC → 13:00 UTC
    /// offset change).
    pub fn next_occurrence(
        &self,
        after: DateTime<Utc>,
    ) -> Result<Option<DateTime<Utc>>, RecurrenceError> {
        use rrule::{RRuleSet, Tz};
        use std::str::FromStr;

        let set = RRuleSet::from_str(&self.rrule)
            .map_err(|e| RecurrenceError::InvalidRule(e.to_string()))?;

        // `RRuleSet::all` treats the `after` bound as INCLUSIVE. We want the
        // first occurrence STRICTLY after `after` (so re-querying at the exact
        // instant a change just fired yields the *next* window, not the same
        // one). RRULE occurrences are second-granular, so bumping the bound by
        // one second gives strict "after" semantics without skipping a fire.
        let strict_after = after + chrono::Duration::seconds(1);
        let after_in_tz = strict_after.with_timezone(&Tz::UTC);
        let set = set.after(after_in_tz);

        // `all(1)` returns at most one occurrence and reports whether the search
        // was limited; a single result is all we need for "next".
        let result = set.all(1);
        Ok(result
            .dates
            .into_iter()
            .next()
            .map(|dt| dt.with_timezone(&Utc)))
    }
}

/// Which kind of entity a scheduled change targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum ScheduleEntityType {
    /// A feature flag.
    Flag,
    /// A segment.
    Segment,
    /// An experiment.
    Experiment,
}

/// A lightweight summary of a scheduled change — the read-model shape surfaced
/// in listings and the admin UI. The full mutation payload and run history live
/// in the persistence layer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ScheduledChange {
    /// Kind of entity targeted.
    pub entity_type: ScheduleEntityType,
    /// Environment the change applies in.
    pub env_id: EnvironmentId,
    /// One-shot vs recurring.
    pub schedule_kind: ScheduleKind,
    /// Current lifecycle status.
    pub status: ScheduleStatus,
    /// One-shot fire instant (UTC), if one-shot.
    pub scheduled_at: Option<DateTime<Utc>>,
    /// Recurrence spec, if recurring.
    pub recurrence: Option<RecurrenceSpec>,
    /// Next computed firing instant (UTC), if any is scheduled.
    pub next_run_at: Option<DateTime<Utc>>,
    /// Last actual firing instant (UTC), if it has ever fired.
    pub last_run_at: Option<DateTime<Utc>>,
}

/// A typed reference to a flag whose schedule/prerequisite relationships are
/// tracked. (Kept minimal in Phase 1; richer cross-entity edges land in later
/// phases on the persistence side.)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct FlagScheduleRef {
    /// The flag the schedule targets.
    pub flag_id: FlagId,
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    /// A weekday-09:00 America/New_York series, asked for the next occurrence
    /// just BEFORE the US spring-forward (2026-03-08 02:00 local). At that point
    /// the zone is still EST (UTC-5), so 09:00 local = 14:00 UTC.
    ///
    /// Then asked again AFTER the spring-forward: the zone is EDT (UTC-4), so the
    /// same 09:00 *wall-clock* time is now 13:00 UTC. The UTC instant shifting by
    /// one hour while the local time stays at 09:00 is exactly the DST-correct
    /// behaviour we require.
    #[test]
    fn weekday_window_is_dst_correct_across_spring_forward() {
        let spec = RecurrenceSpec {
            rrule: "DTSTART;TZID=America/New_York:20260302T090000\n\
                    RRULE:FREQ=WEEKLY;BYDAY=MO,TU,WE,TH,FR"
                .to_string(),
            tz: "America/New_York".to_string(),
        };

        // Before DST change: Thursday 2026-03-05 06:00 UTC (01:00 EST). The next
        // weekday fire is that same Thursday 09:00 EST = 14:00 UTC (EST = UTC-5).
        let before = Utc.with_ymd_and_hms(2026, 3, 5, 6, 0, 0).unwrap();
        let next_before = spec
            .next_occurrence(before)
            .expect("rule valid")
            .expect("has an occurrence");
        assert_eq!(
            next_before,
            Utc.with_ymd_and_hms(2026, 3, 5, 14, 0, 0).unwrap(),
            "pre-DST 09:00 EST should be 14:00 UTC"
        );

        // After DST change (spring-forward was Sun 2026-03-08 02:00→03:00 local).
        // Ask from Mon 2026-03-09 06:00 UTC. The next weekday fire is Mon
        // 2026-03-09 09:00 EDT = 13:00 UTC — one hour earlier in UTC, same local
        // (EDT = UTC-4).
        let after = Utc.with_ymd_and_hms(2026, 3, 9, 6, 0, 0).unwrap();
        let next_after = spec
            .next_occurrence(after)
            .expect("rule valid")
            .expect("has an occurrence");
        assert_eq!(
            next_after,
            Utc.with_ymd_and_hms(2026, 3, 9, 13, 0, 0).unwrap(),
            "post-DST 09:00 EDT should be 13:00 UTC (offset shifted, local unchanged)"
        );

        // The UTC hour-of-day genuinely shifted between the two fires (14:00 vs
        // 13:00) while both are 09:00 local — proof the math tracked the DST
        // offset change rather than holding the UTC instant fixed.
        assert_eq!(
            next_before.time(),
            chrono::NaiveTime::from_hms_opt(14, 0, 0).unwrap()
        );
        assert_eq!(
            next_after.time(),
            chrono::NaiveTime::from_hms_opt(13, 0, 0).unwrap()
        );
    }

    #[test]
    fn next_occurrence_is_strictly_after_the_boundary() {
        // A daily 09:00 UTC series; asking exactly at a fire instant should
        // return the NEXT day's fire, not the same one.
        let spec = RecurrenceSpec {
            rrule: "DTSTART;TZID=UTC:20260101T090000\nRRULE:FREQ=DAILY".to_string(),
            tz: "UTC".to_string(),
        };
        let at_fire = Utc.with_ymd_and_hms(2026, 6, 1, 9, 0, 0).unwrap();
        let next = spec
            .next_occurrence(at_fire)
            .expect("valid")
            .expect("has occurrence");
        assert_eq!(next, Utc.with_ymd_and_hms(2026, 6, 2, 9, 0, 0).unwrap());
    }

    #[test]
    fn count_bounded_rule_runs_out() {
        // Only two occurrences exist; asking after the last returns None.
        let spec = RecurrenceSpec {
            rrule: "DTSTART;TZID=UTC:20260101T090000\nRRULE:FREQ=DAILY;COUNT=2".to_string(),
            tz: "UTC".to_string(),
        };
        // After 2026-01-02 09:00 (the 2nd and last occurrence) → None.
        let after_last = Utc.with_ymd_and_hms(2026, 1, 2, 9, 0, 1).unwrap();
        let next = spec.next_occurrence(after_last).expect("valid");
        assert!(next.is_none(), "exhausted series should yield None");
    }

    #[test]
    fn invalid_rule_is_an_error() {
        let spec = RecurrenceSpec {
            rrule: "this is not an rrule".to_string(),
            tz: "UTC".to_string(),
        };
        let res = spec.next_occurrence(Utc::now());
        assert!(matches!(res, Err(RecurrenceError::InvalidRule(_))));
    }

    #[test]
    fn schedule_kind_and_status_round_trip() {
        for kind in [ScheduleKind::OneShot, ScheduleKind::Recurring] {
            let json = serde_json::to_string(&kind).unwrap();
            let back: ScheduleKind = serde_json::from_str(&json).unwrap();
            assert_eq!(kind, back);
        }
        assert_eq!(
            serde_json::to_string(&ScheduleKind::OneShot).unwrap(),
            "\"one_shot\""
        );
        for status in [
            ScheduleStatus::Pending,
            ScheduleStatus::Active,
            ScheduleStatus::Paused,
            ScheduleStatus::Applied,
            ScheduleStatus::Failed,
            ScheduleStatus::Cancelled,
        ] {
            let json = serde_json::to_string(&status).unwrap();
            let back: ScheduleStatus = serde_json::from_str(&json).unwrap();
            assert_eq!(status, back);
        }
    }

    #[test]
    fn scheduled_change_summary_round_trips() {
        let change = ScheduledChange {
            entity_type: ScheduleEntityType::Flag,
            env_id: EnvironmentId::new(),
            schedule_kind: ScheduleKind::OneShot,
            status: ScheduleStatus::Pending,
            scheduled_at: Some(Utc.with_ymd_and_hms(2026, 7, 1, 0, 0, 0).unwrap()),
            recurrence: None,
            next_run_at: Some(Utc.with_ymd_and_hms(2026, 7, 1, 0, 0, 0).unwrap()),
            last_run_at: None,
        };
        let json = serde_json::to_string(&change).unwrap();
        let back: ScheduledChange = serde_json::from_str(&json).unwrap();
        assert_eq!(change, back);
    }
}
