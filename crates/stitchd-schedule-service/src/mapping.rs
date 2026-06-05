//! Proto ↔ repository-row conversions for the schedule service.

use chrono::{DateTime, Utc};
use stitchd_db::{RunOutcome, ScheduledChangeRow, ScheduledChangeRunRow};
use stitchd_proto::schedule::v1 as pb;

/// Convert a UTC instant to epoch-milliseconds (0 for `None`).
#[must_use]
pub fn to_ms(dt: Option<DateTime<Utc>>) -> i64 {
    dt.map_or(0, |t| t.timestamp_millis())
}

/// Convert epoch-milliseconds to a UTC instant (`None` when 0).
#[must_use]
pub fn from_ms(ms: i64) -> Option<DateTime<Utc>> {
    if ms == 0 {
        None
    } else {
        DateTime::from_timestamp_millis(ms)
    }
}

/// Map the stored `entity_type` text to the proto enum discriminant.
#[must_use]
pub fn entity_type_to_proto(s: &str) -> i32 {
    match s {
        "flag" => pb::ScheduleEntityType::Flag as i32,
        "segment" => pb::ScheduleEntityType::Segment as i32,
        "experiment" => pb::ScheduleEntityType::Experiment as i32,
        _ => pb::ScheduleEntityType::Unspecified as i32,
    }
}

/// Map a proto entity-type enum to the stored text, if specified.
#[must_use]
pub fn entity_type_from_proto(v: i32) -> Option<&'static str> {
    match pb::ScheduleEntityType::try_from(v).ok()? {
        pb::ScheduleEntityType::Flag => Some("flag"),
        pb::ScheduleEntityType::Segment => Some("segment"),
        pb::ScheduleEntityType::Experiment => Some("experiment"),
        pb::ScheduleEntityType::Unspecified => None,
    }
}

/// Map the stored `schedule_kind` text to the proto enum discriminant.
#[must_use]
pub fn kind_to_proto(s: &str) -> i32 {
    match s {
        "one_shot" => pb::ScheduleKind::OneShot as i32,
        "recurring" => pb::ScheduleKind::Recurring as i32,
        _ => pb::ScheduleKind::Unspecified as i32,
    }
}

/// Map the stored `status` to the proto enum discriminant.
#[must_use]
pub fn status_to_proto(s: stitchd_db::ScheduleStatus) -> i32 {
    use stitchd_db::ScheduleStatus as S;
    match s {
        S::Pending => pb::ScheduleStatus::Pending as i32,
        S::Active => pb::ScheduleStatus::Active as i32,
        S::Paused => pb::ScheduleStatus::Paused as i32,
        S::Applied => pb::ScheduleStatus::Applied as i32,
        S::Failed => pb::ScheduleStatus::Failed as i32,
        S::Cancelled => pb::ScheduleStatus::Cancelled as i32,
    }
}

/// Map a proto status filter to the repo enum, if specified.
#[must_use]
pub fn status_from_proto(v: i32) -> Option<stitchd_db::ScheduleStatus> {
    use stitchd_db::ScheduleStatus as S;
    match pb::ScheduleStatus::try_from(v).ok()? {
        pb::ScheduleStatus::Pending => Some(S::Pending),
        pb::ScheduleStatus::Active => Some(S::Active),
        pb::ScheduleStatus::Paused => Some(S::Paused),
        pb::ScheduleStatus::Applied => Some(S::Applied),
        pb::ScheduleStatus::Failed => Some(S::Failed),
        pb::ScheduleStatus::Cancelled => Some(S::Cancelled),
        pb::ScheduleStatus::Unspecified => None,
    }
}

/// Map a run outcome to the proto enum discriminant.
#[must_use]
pub fn run_outcome_to_proto(o: RunOutcome) -> i32 {
    match o {
        RunOutcome::Applied => pb::ScheduleRunOutcome::Applied as i32,
        RunOutcome::Skipped => pb::ScheduleRunOutcome::Skipped as i32,
        RunOutcome::Failed => pb::ScheduleRunOutcome::Failed as i32,
    }
}

/// Convert a repository row to its proto representation. `runs` is populated
/// separately (empty for List, hydrated for Get).
#[must_use]
pub fn change_to_proto(
    row: &ScheduledChangeRow,
    runs: Vec<pb::ScheduledChangeRun>,
) -> pb::ScheduledChange {
    pb::ScheduledChange {
        id: row.id.to_string(),
        entity_type: entity_type_to_proto(&row.entity_type),
        entity_id: row.entity_id.to_string(),
        env_id: row.env_id.to_string(),
        mutation_payload_json: row.mutation_payload.to_string(),
        schedule_kind: kind_to_proto(&row.schedule_kind),
        scheduled_at_ms: to_ms(row.scheduled_at),
        rrule: row.rrule.clone().unwrap_or_default(),
        tz: row.tz.clone().unwrap_or_default(),
        status: status_to_proto(row.status),
        next_run_at_ms: to_ms(row.next_run_at),
        last_run_at_ms: to_ms(row.last_run_at),
        created_at_ms: to_ms(Some(row.created_at)),
        updated_at_ms: to_ms(Some(row.updated_at)),
        version: u64::try_from(row.version).unwrap_or(0),
        runs,
    }
}

/// Convert a run-history row to proto.
#[must_use]
pub fn run_to_proto(row: &ScheduledChangeRunRow) -> pb::ScheduledChangeRun {
    pb::ScheduledChangeRun {
        id: row.id.to_string(),
        fired_at_ms: to_ms(Some(row.fired_at)),
        outcome: run_outcome_to_proto(row.outcome),
        detail: row.detail.clone().unwrap_or_default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ms_roundtrip() {
        let now = Utc::now();
        // millisecond precision only.
        let ms = to_ms(Some(now));
        let back = from_ms(ms).unwrap();
        assert_eq!(back.timestamp_millis(), now.timestamp_millis());
        assert_eq!(to_ms(None), 0);
        assert!(from_ms(0).is_none());
    }

    #[test]
    fn entity_type_roundtrip() {
        assert_eq!(
            entity_type_from_proto(entity_type_to_proto("flag")),
            Some("flag")
        );
        assert_eq!(
            entity_type_from_proto(entity_type_to_proto("segment")),
            Some("segment")
        );
        assert_eq!(
            entity_type_from_proto(entity_type_to_proto("experiment")),
            Some("experiment")
        );
        assert_eq!(entity_type_from_proto(0), None);
    }

    #[test]
    fn status_roundtrip() {
        for s in [
            stitchd_db::ScheduleStatus::Pending,
            stitchd_db::ScheduleStatus::Active,
            stitchd_db::ScheduleStatus::Paused,
            stitchd_db::ScheduleStatus::Applied,
            stitchd_db::ScheduleStatus::Failed,
            stitchd_db::ScheduleStatus::Cancelled,
        ] {
            assert_eq!(status_from_proto(status_to_proto(s)), Some(s));
        }
        assert_eq!(status_from_proto(0), None);
    }

    #[test]
    fn kind_to_proto_maps_all_variants() {
        assert_eq!(kind_to_proto("one_shot"), pb::ScheduleKind::OneShot as i32);
        assert_eq!(
            kind_to_proto("recurring"),
            pb::ScheduleKind::Recurring as i32
        );
        assert_eq!(
            kind_to_proto("bogus"),
            pb::ScheduleKind::Unspecified as i32
        );
    }

    #[test]
    fn run_outcome_to_proto_maps_all_variants() {
        assert_eq!(
            run_outcome_to_proto(RunOutcome::Applied),
            pb::ScheduleRunOutcome::Applied as i32
        );
        assert_eq!(
            run_outcome_to_proto(RunOutcome::Skipped),
            pb::ScheduleRunOutcome::Skipped as i32
        );
        assert_eq!(
            run_outcome_to_proto(RunOutcome::Failed),
            pb::ScheduleRunOutcome::Failed as i32
        );
    }

    #[test]
    fn change_and_run_to_proto_map_rows() {
        use chrono::Utc;
        use stitchd_db::{ScheduleStatus, ScheduledChangeRow, ScheduledChangeRunRow};
        use uuid::Uuid;

        let now = Utc::now();
        let row = ScheduledChangeRow {
            id: Uuid::new_v4(),
            entity_type: "flag".to_string(),
            entity_id: Uuid::new_v4(),
            env_id: Uuid::new_v4(),
            mutation_payload: serde_json::json!({"k": "v"}),
            schedule_kind: "recurring".to_string(),
            scheduled_at: None,
            rrule: Some("DTSTART:20260101T000000Z\nRRULE:FREQ=DAILY".to_string()),
            tz: Some("UTC".to_string()),
            next_run_at: Some(now),
            last_run_at: Some(now),
            status: ScheduleStatus::Active,
            version: 3,
            created_at: now,
            updated_at: now,
            created_by: None,
        };
        let run = ScheduledChangeRunRow {
            id: Uuid::new_v4(),
            scheduled_change_id: row.id,
            fired_at: now,
            outcome: RunOutcome::Skipped,
            detail: Some("flag_locked_by_experiment:abc".to_string()),
        };
        let run_pb = run_to_proto(&run);
        assert_eq!(run_pb.outcome, pb::ScheduleRunOutcome::Skipped as i32);
        assert_eq!(run_pb.detail, "flag_locked_by_experiment:abc");

        let pb = change_to_proto(&row, vec![run_pb]);
        assert_eq!(pb.id, row.id.to_string());
        assert_eq!(pb.entity_type, pb::ScheduleEntityType::Flag as i32);
        assert_eq!(pb.schedule_kind, pb::ScheduleKind::Recurring as i32);
        assert_eq!(pb.status, pb::ScheduleStatus::Active as i32);
        assert_eq!(pb.tz, "UTC");
        assert_eq!(pb.version, 3);
        assert_eq!(pb.runs.len(), 1);
        assert_eq!(pb.scheduled_at_ms, 0);
        assert_eq!(pb.next_run_at_ms, now.timestamp_millis());
    }
}
