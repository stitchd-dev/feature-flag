//! gRPC `ScheduleService` implementation backed by [`ScheduledChangeRepository`].

use tonic::{Request, Response, Status};
use uuid::Uuid;

use stitchd_core::schedule::RecurrenceSpec;
use stitchd_db::{NewScheduledChange, RepositoryError, ScheduleKind, ScheduledChangeRepository};
use stitchd_proto::schedule::v1::{
    self as pb, CancelScheduledChangeRequest, CreateScheduledChangeRequest,
    GetScheduledChangeRequest, ListDueChangesRequest, ListDueChangesResponse,
    ListScheduledChangesRequest, ListScheduledChangesResponse, PauseScheduledChangeRequest,
    ResumeScheduledChangeRequest, ScheduledChange, schedule_service_server::ScheduleService,
};

use crate::mapping;

/// gRPC server for scheduled-change lifecycle operations.
pub struct ScheduleServiceImpl {
    repo: ScheduledChangeRepository,
}

impl ScheduleServiceImpl {
    /// Construct a new service bound to `repo`.
    #[must_use]
    pub const fn new(repo: ScheduledChangeRepository) -> Self {
        Self { repo }
    }

    /// Fetch a change and hydrate it with run history.
    async fn change_with_runs(&self, id: Uuid) -> Result<ScheduledChange, Status> {
        let row = self.repo.get(id).await.map_err(repo_status)?;
        let runs = self
            .repo
            .list_runs(id)
            .await
            .map_err(repo_status)?
            .iter()
            .map(mapping::run_to_proto)
            .collect();
        Ok(mapping::change_to_proto(&row, runs))
    }
}

fn parse_uuid(s: &str, field: &str) -> Result<Uuid, Status> {
    s.parse::<Uuid>()
        .map_err(|_| Status::invalid_argument(format!("invalid {field} UUID")))
}

fn repo_status(e: RepositoryError) -> Status {
    Status::from(e)
}

#[tonic::async_trait]
impl ScheduleService for ScheduleServiceImpl {
    async fn create_scheduled_change(
        &self,
        request: Request<CreateScheduledChangeRequest>,
    ) -> Result<Response<ScheduledChange>, Status> {
        let req = request.into_inner();

        let entity_type = mapping::entity_type_from_proto(req.entity_type)
            .ok_or_else(|| Status::invalid_argument("entity_type must be specified"))?;
        let entity_id = parse_uuid(&req.entity_id, "entity_id")?;
        let env_id = parse_uuid(&req.env_id, "env_id")?;

        let mutation_payload: serde_json::Value = serde_json::from_str(&req.mutation_payload_json)
            .map_err(|e| {
                Status::invalid_argument(format!("mutation_payload_json is not valid JSON: {e}"))
            })?;

        let kind = pb::ScheduleKind::try_from(req.schedule_kind)
            .map_err(|_| Status::invalid_argument("unknown schedule_kind"))?;

        let (schedule_kind, scheduled_at, rrule, tz, next_run_at) = match kind {
            pb::ScheduleKind::OneShot => {
                let at = mapping::from_ms(req.scheduled_at_ms)
                    .ok_or_else(|| Status::invalid_argument("one-shot requires scheduled_at_ms"))?;
                (ScheduleKind::OneShot, Some(at), None, None, Some(at))
            }
            pb::ScheduleKind::Recurring => {
                if req.rrule.is_empty() {
                    return Err(Status::invalid_argument("recurring requires rrule"));
                }
                let tz = if req.tz.is_empty() {
                    "UTC".to_string()
                } else {
                    req.tz.clone()
                };
                // Compute the first occurrence strictly after "now" so the
                // scheduler picks it up on the appropriate tick.
                let spec = RecurrenceSpec {
                    rrule: req.rrule.clone(),
                    tz: tz.clone(),
                };
                let next = spec
                    .next_occurrence(chrono::Utc::now())
                    .map_err(|e| Status::invalid_argument(format!("invalid rrule: {e}")))?;
                (
                    ScheduleKind::Recurring,
                    None,
                    Some(req.rrule.clone()),
                    Some(tz),
                    next,
                )
            }
            pb::ScheduleKind::Unspecified => {
                return Err(Status::invalid_argument("schedule_kind must be specified"));
            }
        };

        let input = NewScheduledChange {
            entity_type: entity_type.to_string(),
            entity_id,
            env_id,
            mutation_payload,
            schedule_kind,
            scheduled_at,
            rrule,
            tz,
            next_run_at,
            created_by: None,
        };
        let row = self.repo.create(&input).await.map_err(repo_status)?;
        Ok(Response::new(mapping::change_to_proto(&row, Vec::new())))
    }

    async fn list_scheduled_changes(
        &self,
        request: Request<ListScheduledChangesRequest>,
    ) -> Result<Response<ListScheduledChangesResponse>, Status> {
        let req = request.into_inner();
        let env_id = parse_uuid(&req.env_id, "env_id")?;

        let rows = if let Some(entity_type) = mapping::entity_type_from_proto(req.entity_type) {
            // Entity-scoped listing.
            let entity_id = parse_uuid(&req.entity_id, "entity_id")?;
            self.repo
                .list_by_entity(entity_type, entity_id)
                .await
                .map_err(repo_status)?
        } else {
            let status = mapping::status_from_proto(req.status);
            self.repo
                .list_by_env(env_id, status)
                .await
                .map_err(repo_status)?
        };

        let changes = rows
            .iter()
            .map(|r| mapping::change_to_proto(r, Vec::new()))
            .collect();
        Ok(Response::new(ListScheduledChangesResponse { changes }))
    }

    async fn get_scheduled_change(
        &self,
        request: Request<GetScheduledChangeRequest>,
    ) -> Result<Response<ScheduledChange>, Status> {
        let id = parse_uuid(&request.into_inner().id, "id")?;
        Ok(Response::new(self.change_with_runs(id).await?))
    }

    async fn cancel_scheduled_change(
        &self,
        request: Request<CancelScheduledChangeRequest>,
    ) -> Result<Response<ScheduledChange>, Status> {
        let req = request.into_inner();
        let id = parse_uuid(&req.id, "id")?;
        let version = i64::try_from(req.version)
            .map_err(|_| Status::invalid_argument("version out of range"))?;
        let row = self.repo.cancel(id, version).await.map_err(repo_status)?;
        Ok(Response::new(mapping::change_to_proto(&row, Vec::new())))
    }

    async fn pause_scheduled_change(
        &self,
        request: Request<PauseScheduledChangeRequest>,
    ) -> Result<Response<ScheduledChange>, Status> {
        let req = request.into_inner();
        let id = parse_uuid(&req.id, "id")?;
        let version = i64::try_from(req.version)
            .map_err(|_| Status::invalid_argument("version out of range"))?;
        let row = self.repo.pause(id, version).await.map_err(repo_status)?;
        Ok(Response::new(mapping::change_to_proto(&row, Vec::new())))
    }

    async fn resume_scheduled_change(
        &self,
        request: Request<ResumeScheduledChangeRequest>,
    ) -> Result<Response<ScheduledChange>, Status> {
        let req = request.into_inner();
        let id = parse_uuid(&req.id, "id")?;
        let version = i64::try_from(req.version)
            .map_err(|_| Status::invalid_argument("version out of range"))?;
        let row = self.repo.resume(id, version).await.map_err(repo_status)?;
        Ok(Response::new(mapping::change_to_proto(&row, Vec::new())))
    }

    async fn list_due_changes(
        &self,
        request: Request<ListDueChangesRequest>,
    ) -> Result<Response<ListDueChangesResponse>, Status> {
        let req = request.into_inner();
        let as_of = mapping::from_ms(req.as_of_ms).unwrap_or_else(chrono::Utc::now);
        let limit = if req.limit == 0 {
            100
        } else {
            i64::from(req.limit)
        };
        // Read-only peek at due rows (no claim/lock): the internal RPC just
        // surfaces what is due; the in-process scheduler loop does the
        // locking claim directly against the repo.
        let mut tx = self.repo.begin().await.map_err(repo_status)?;
        let rows = self
            .repo
            .claim_due(&mut tx, as_of, limit)
            .await
            .map_err(repo_status)?;
        // Roll back so the peek does not hold locks or mutate state.
        tx.rollback()
            .await
            .map_err(|e| Status::internal(format!("rollback failed: {e}")))?;

        let changes = rows
            .iter()
            .map(|r| mapping::change_to_proto(r, Vec::new()))
            .collect();
        Ok(Response::new(ListDueChangesResponse { changes }))
    }
}
