//! Integration tests for the gRPC `ScheduleService` surface (`grpc.rs`).
//!
//! These exercise the proto request/response handlers end-to-end against a fresh
//! per-test PostgreSQL DB provisioned by `#[sqlx::test]`. Pure proto↔row mapping
//! is unit-tested in `src/mapping.rs`; here we cover the service handlers (create
//! one-shot + recurring, list by entity / env, get-with-runs, cancel/pause/resume,
//! list-due peek, and the invalid-argument validation branches).

use chrono::Utc;
use sqlx::PgPool;
use tonic::{Code, Request};
use uuid::Uuid;

use stitchd_db::{RunOutcome, ScheduledChangeRepository};
use stitchd_proto::schedule::v1::{
    self as pb, CancelScheduledChangeRequest, CreateScheduledChangeRequest,
    GetScheduledChangeRequest, ListDueChangesRequest, ListScheduledChangesRequest,
    PauseScheduledChangeRequest, ResumeScheduledChangeRequest,
    schedule_service_server::ScheduleService,
};
use stitchd_schedule_service::grpc::ScheduleServiceImpl;

fn one_shot_req(env_id: Uuid, entity_id: Uuid, at_ms: i64) -> CreateScheduledChangeRequest {
    CreateScheduledChangeRequest {
        entity_type: pb::ScheduleEntityType::Flag as i32,
        entity_id: entity_id.to_string(),
        env_id: env_id.to_string(),
        mutation_payload_json: r#"{"transition":"start"}"#.to_string(),
        schedule_kind: pb::ScheduleKind::OneShot as i32,
        scheduled_at_ms: at_ms,
        rrule: String::new(),
        tz: String::new(),
    }
}

#[sqlx::test(migrations = "../stitchd-db/migrations")]
async fn create_one_shot_then_get_with_runs(pool: PgPool) {
    let repo = ScheduledChangeRepository::new(pool);
    let svc = ScheduleServiceImpl::new(repo.clone());
    let env_id = Uuid::new_v4();
    let entity_id = Uuid::new_v4();
    let at = Utc::now() + chrono::Duration::hours(1);

    let created = svc
        .create_scheduled_change(Request::new(one_shot_req(env_id, entity_id, at.timestamp_millis())))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(created.entity_type, pb::ScheduleEntityType::Flag as i32);
    assert_eq!(created.schedule_kind, pb::ScheduleKind::OneShot as i32);
    assert_eq!(created.scheduled_at_ms, at.timestamp_millis());
    assert!(created.runs.is_empty());

    let id: Uuid = created.id.parse().unwrap();
    // Append a run so Get hydrates run history.
    repo.append_run(id, RunOutcome::Applied, Some("ok")).await.unwrap();

    let fetched = svc
        .get_scheduled_change(Request::new(GetScheduledChangeRequest { id: created.id.clone() }))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(fetched.runs.len(), 1);
    assert_eq!(fetched.runs[0].outcome, pb::ScheduleRunOutcome::Applied as i32);
    assert_eq!(fetched.runs[0].detail, "ok");
}

#[sqlx::test(migrations = "../stitchd-db/migrations")]
async fn create_recurring_computes_next_run(pool: PgPool) {
    let svc = ScheduleServiceImpl::new(ScheduledChangeRepository::new(pool));
    let req = CreateScheduledChangeRequest {
        entity_type: pb::ScheduleEntityType::Flag as i32,
        entity_id: Uuid::new_v4().to_string(),
        env_id: Uuid::new_v4().to_string(),
        mutation_payload_json: "{}".to_string(),
        schedule_kind: pb::ScheduleKind::Recurring as i32,
        scheduled_at_ms: 0,
        rrule: "DTSTART;TZID=America/New_York:20260101T090000\nRRULE:FREQ=DAILY".to_string(),
        tz: "America/New_York".to_string(),
    };
    let created = svc
        .create_scheduled_change(Request::new(req))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(created.schedule_kind, pb::ScheduleKind::Recurring as i32);
    assert_eq!(created.tz, "America/New_York");
    assert!(created.next_run_at_ms > 0, "recurring change must have a computed next_run_at");
}

#[sqlx::test(migrations = "../stitchd-db/migrations")]
async fn list_by_entity_and_by_env(pool: PgPool) {
    let repo = ScheduledChangeRepository::new(pool);
    let svc = ScheduleServiceImpl::new(repo);
    let env_id = Uuid::new_v4();
    let entity_id = Uuid::new_v4();
    let at = Utc::now() + chrono::Duration::hours(1);
    svc.create_scheduled_change(Request::new(one_shot_req(env_id, entity_id, at.timestamp_millis())))
        .await
        .unwrap();

    // Entity-scoped (entity_type specified).
    let by_entity = svc
        .list_scheduled_changes(Request::new(ListScheduledChangesRequest {
            env_id: env_id.to_string(),
            entity_type: pb::ScheduleEntityType::Flag as i32,
            entity_id: entity_id.to_string(),
            status: pb::ScheduleStatus::Unspecified as i32,
        }))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(by_entity.changes.len(), 1);

    // Env-scoped (entity_type unspecified → falls through to list_by_env).
    let by_env = svc
        .list_scheduled_changes(Request::new(ListScheduledChangesRequest {
            env_id: env_id.to_string(),
            entity_type: pb::ScheduleEntityType::Unspecified as i32,
            entity_id: String::new(),
            status: pb::ScheduleStatus::Unspecified as i32,
        }))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(by_env.changes.len(), 1);
}

#[sqlx::test(migrations = "../stitchd-db/migrations")]
async fn pause_resume_transitions_on_recurring(pool: PgPool) {
    let svc = ScheduleServiceImpl::new(ScheduledChangeRepository::new(pool));
    // Recurring changes are created `active`, so they support pause → resume.
    let req = CreateScheduledChangeRequest {
        entity_type: pb::ScheduleEntityType::Flag as i32,
        entity_id: Uuid::new_v4().to_string(),
        env_id: Uuid::new_v4().to_string(),
        mutation_payload_json: "{}".to_string(),
        schedule_kind: pb::ScheduleKind::Recurring as i32,
        scheduled_at_ms: 0,
        rrule: "DTSTART;TZID=UTC:20260101T000000\nRRULE:FREQ=DAILY".to_string(),
        tz: "UTC".to_string(),
    };
    let created = svc
        .create_scheduled_change(Request::new(req))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(created.status, pb::ScheduleStatus::Active as i32);
    let id = created.id.clone();

    let paused = svc
        .pause_scheduled_change(Request::new(PauseScheduledChangeRequest {
            id: id.clone(),
            version: created.version,
        }))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(paused.status, pb::ScheduleStatus::Paused as i32);

    let resumed = svc
        .resume_scheduled_change(Request::new(ResumeScheduledChangeRequest {
            id,
            version: paused.version,
        }))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(resumed.status, pb::ScheduleStatus::Active as i32);
}

#[sqlx::test(migrations = "../stitchd-db/migrations")]
async fn cancel_transitions_pending_one_shot(pool: PgPool) {
    let svc = ScheduleServiceImpl::new(ScheduledChangeRepository::new(pool));
    let at = Utc::now() + chrono::Duration::hours(1);
    let created = svc
        .create_scheduled_change(Request::new(one_shot_req(
            Uuid::new_v4(),
            Uuid::new_v4(),
            at.timestamp_millis(),
        )))
        .await
        .unwrap()
        .into_inner();
    let cancelled = svc
        .cancel_scheduled_change(Request::new(CancelScheduledChangeRequest {
            id: created.id,
            version: created.version,
        }))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(cancelled.status, pb::ScheduleStatus::Cancelled as i32);
}

#[sqlx::test(migrations = "../stitchd-db/migrations")]
async fn list_due_changes_peeks_without_mutating(pool: PgPool) {
    let repo = ScheduledChangeRepository::new(pool);
    let svc = ScheduleServiceImpl::new(repo.clone());
    let env_id = Uuid::new_v4();
    let entity_id = Uuid::new_v4();
    // Due in the past so it's claimable now.
    let past = Utc::now() - chrono::Duration::minutes(5);
    let created = svc
        .create_scheduled_change(Request::new(one_shot_req(env_id, entity_id, past.timestamp_millis())))
        .await
        .unwrap()
        .into_inner();
    let id: Uuid = created.id.parse().unwrap();

    let due = svc
        .list_due_changes(Request::new(ListDueChangesRequest {
            as_of_ms: Utc::now().timestamp_millis(),
            limit: 0, // exercises the default-limit branch
        }))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(due.changes.len(), 1);

    // The peek rolled back: the row is untouched and still claimable.
    let after = repo.get(id).await.unwrap();
    assert_eq!(after.status, stitchd_db::ScheduleStatus::Pending);
}

#[sqlx::test(migrations = "../stitchd-db/migrations")]
async fn invalid_arguments_are_rejected(pool: PgPool) {
    let svc = ScheduleServiceImpl::new(ScheduledChangeRepository::new(pool));
    let env_id = Uuid::new_v4();

    // Unspecified entity_type.
    let mut bad = one_shot_req(env_id, Uuid::new_v4(), Utc::now().timestamp_millis());
    bad.entity_type = pb::ScheduleEntityType::Unspecified as i32;
    let err = svc.create_scheduled_change(Request::new(bad)).await.unwrap_err();
    assert_eq!(err.code(), Code::InvalidArgument);

    // Bad entity_id UUID.
    let mut bad = one_shot_req(env_id, Uuid::new_v4(), Utc::now().timestamp_millis());
    bad.entity_id = "not-a-uuid".to_string();
    let err = svc.create_scheduled_change(Request::new(bad)).await.unwrap_err();
    assert_eq!(err.code(), Code::InvalidArgument);

    // Invalid mutation payload JSON.
    let mut bad = one_shot_req(env_id, Uuid::new_v4(), Utc::now().timestamp_millis());
    bad.mutation_payload_json = "{not json".to_string();
    let err = svc.create_scheduled_change(Request::new(bad)).await.unwrap_err();
    assert_eq!(err.code(), Code::InvalidArgument);

    // One-shot missing scheduled_at_ms.
    let mut bad = one_shot_req(env_id, Uuid::new_v4(), 0);
    bad.scheduled_at_ms = 0;
    let err = svc.create_scheduled_change(Request::new(bad)).await.unwrap_err();
    assert_eq!(err.code(), Code::InvalidArgument);

    // Recurring missing rrule.
    let bad = CreateScheduledChangeRequest {
        entity_type: pb::ScheduleEntityType::Flag as i32,
        entity_id: Uuid::new_v4().to_string(),
        env_id: env_id.to_string(),
        mutation_payload_json: "{}".to_string(),
        schedule_kind: pb::ScheduleKind::Recurring as i32,
        scheduled_at_ms: 0,
        rrule: String::new(),
        tz: String::new(),
    };
    let err = svc.create_scheduled_change(Request::new(bad)).await.unwrap_err();
    assert_eq!(err.code(), Code::InvalidArgument);

    // Bad UUID on get.
    let err = svc
        .get_scheduled_change(Request::new(GetScheduledChangeRequest { id: "nope".to_string() }))
        .await
        .unwrap_err();
    assert_eq!(err.code(), Code::InvalidArgument);
}
