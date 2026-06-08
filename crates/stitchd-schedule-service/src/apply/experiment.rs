//! Experiment apply path (Phase 5 Task 1).
//!
//! A due **experiment** scheduled change is dispatched to experimentation-service's
//! canonical `TransitionExperiment` RPC, so the lifecycle transition flows through
//! the same path a human start/pause/resume/stop does: it bumps the experiment's
//! optimistic-concurrency version and (on the start path) enforces the experiment's
//! start-time prerequisites server-side. The scheduler client carries no end-user
//! identity, so the transition is attributed to the system/scheduler actor.
//!
//! ## Transition validation at fire time (spec A9, A3)
//! The stored payload names a target lifecycle status (start/pause/resume/stop/
//! archive). The transition is validated **at fire time** by experimentation-service
//! (`validate_transition` in `stitchd-core`), not when the schedule was authored —
//! the experiment may have moved since. An invalid transition (e.g. starting an
//! already-running experiment, or another experiment already running on the same
//! flag, or an unmet start-time prerequisite) comes back as `FAILED_PRECONDITION` /
//! `ALREADY_EXISTS`; we map it to [`ApplyOutcome::Skipped`] with the reason so:
//!   * the run is recorded `skipped` (never `failed` — it is recoverable),
//!   * the scheduler loop does NOT error, and
//!   * a **recurring** schedule still advances to its next window.
//!
//! Any other RPC error (transport, internal) maps to [`ApplyOutcome::Failed`].
//!
//! ## Mutation payload
//! The stored `mutation_payload` is a small serde mirror of the
//! `TransitionExperiment` inputs the scheduler supports: a target status plus an
//! optional human-readable reason. prost messages are not serde-serializable, so
//! this mirror is rebuilt into a `TransitionExperimentRequest`. The target
//! experiment + environment come from the scheduled-change row's
//! `entity_id` / `env_id`.

use async_trait::async_trait;
use serde::Deserialize;
use tonic::Status;

use stitchd_db::ScheduledChangeRow;
use stitchd_proto::experiments::v1::{ExperimentStatus, TransitionExperimentRequest};

use crate::apply::{Applier, ApplyOutcome};

/// Abstraction over the experimentation-service `TransitionExperiment` RPC so the
/// apply path is unit-testable with a stub (no live experimentation-service).
#[async_trait]
pub trait ExperimentTransitioner: Send + Sync {
    /// Invoke `TransitionExperiment`. Returns the gRPC status on failure (the
    /// apply path inspects its code to classify the outcome).
    async fn transition_experiment(&self, req: TransitionExperimentRequest) -> Result<(), Status>;
}

/// Production [`ExperimentTransitioner`] backed by a tonic experimentation-service
/// client. The scheduler carries no end-user identity on the request, so
/// experimentation-service attributes the transition to the system/scheduler
/// actor in the audit log and enforces start-time prerequisites itself.
pub struct GrpcExperimentTransitioner {
    client: std::sync::Arc<
        tokio::sync::Mutex<
            stitchd_proto::experiments::v1::experimentation_service_client::ExperimentationServiceClient<
                tonic::transport::Channel,
            >,
        >,
    >,
}

impl GrpcExperimentTransitioner {
    /// Construct a transitioner over a shared experimentation-service client.
    #[must_use]
    pub const fn new(
        client: std::sync::Arc<
            tokio::sync::Mutex<
                stitchd_proto::experiments::v1::experimentation_service_client::ExperimentationServiceClient<
                    tonic::transport::Channel,
                >,
            >,
        >,
    ) -> Self {
        Self { client }
    }
}

#[async_trait]
impl ExperimentTransitioner for GrpcExperimentTransitioner {
    async fn transition_experiment(&self, req: TransitionExperimentRequest) -> Result<(), Status> {
        let mut client = self.client.lock().await;
        client
            .transition_experiment(tonic::Request::new(req))
            .await?;
        Ok(())
    }
}

/// Applies due experiment changes via an [`ExperimentTransitioner`].
pub struct ExperimentApplier<T: ExperimentTransitioner> {
    transitioner: T,
}

impl<T: ExperimentTransitioner> ExperimentApplier<T> {
    /// Construct an experiment applier over `transitioner`.
    pub const fn new(transitioner: T) -> Self {
        Self { transitioner }
    }
}

#[async_trait]
impl<T: ExperimentTransitioner> Applier for ExperimentApplier<T> {
    async fn apply(&self, change: &ScheduledChangeRow) -> anyhow::Result<ApplyOutcome> {
        // Deserialize the stored payload. A malformed payload is a permanent
        // (non-retryable) failure for this change — record it as failed.
        let payload: ExperimentTransitionPayload =
            match serde_json::from_value(change.mutation_payload.clone()) {
                Ok(p) => p,
                Err(e) => {
                    return Ok(ApplyOutcome::Failed(format!(
                        "invalid experiment transition payload: {e}"
                    )));
                }
            };

        let req = TransitionExperimentRequest {
            environment_id: change.env_id.to_string(),
            experiment_id: change.entity_id.to_string(),
            new_status: payload.transition.to_proto() as i32,
            reason: payload.reason.unwrap_or_default(),
        };

        match self.transitioner.transition_experiment(req).await {
            Ok(()) => Ok(ApplyOutcome::Applied),
            Err(status) => Ok(classify_status(&status)),
        }
    }
}

/// Map a `TransitionExperiment` error status to an [`ApplyOutcome`].
///
/// An **invalid transition at fire time** — including an already-running
/// experiment (`FAILED_PRECONDITION` from `validate_transition`), a conflicting
/// experiment already running on the same flag (`ALREADY_EXISTS` from the
/// uniqueness guard), and an **unmet start-time prerequisite**
/// (`FAILED_PRECONDITION` from the start-prereq gate) — is a recoverable
/// [`ApplyOutcome::Skipped`]: the run is recorded `skipped` with the reason and a
/// recurring schedule advances to its next window. Everything else (transport /
/// internal) is a non-recoverable [`ApplyOutcome::Failed`].
fn classify_status(status: &Status) -> ApplyOutcome {
    match status.code() {
        tonic::Code::FailedPrecondition | tonic::Code::AlreadyExists => {
            ApplyOutcome::Skipped(format!("{}: {}", status.code(), status.message()))
        }
        code => ApplyOutcome::Failed(format!("{}: {}", code, status.message())),
    }
}

/// JSON shape stored in `scheduled_changes.mutation_payload` for an experiment
/// change.
#[derive(Debug, Clone, Deserialize)]
pub struct ExperimentTransitionPayload {
    /// The lifecycle transition to apply.
    pub transition: ExperimentTransition,
    /// Optional human-readable reason recorded on the transition.
    #[serde(default)]
    pub reason: Option<String>,
}

/// The lifecycle transitions the scheduler can dispatch to an experiment.
///
/// Maps onto the proto `ExperimentStatus` target. `start` and `resume` both move
/// the experiment to `ACTIVE` (the start-prereq gate fires on the underlying
/// transition into running); `stop` and `archive` both move it to `CONCLUDED`
/// (the experiment model has no separate archived state — a concluded experiment
/// is the terminal/archived state).
#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ExperimentTransition {
    /// Start a draft (or restart a stopped) experiment → `ACTIVE`.
    Start,
    /// Pause a running experiment → `PAUSED`.
    Pause,
    /// Resume a paused experiment → `ACTIVE`.
    Resume,
    /// Stop a running/paused experiment → `CONCLUDED`.
    Stop,
    /// Archive an experiment → `CONCLUDED` (terminal state).
    Archive,
}

impl ExperimentTransition {
    const fn to_proto(self) -> ExperimentStatus {
        match self {
            Self::Start | Self::Resume => ExperimentStatus::Active,
            Self::Pause => ExperimentStatus::Paused,
            Self::Stop | Self::Archive => ExperimentStatus::Concluded,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use uuid::Uuid;

    fn experiment_change(payload: serde_json::Value) -> ScheduledChangeRow {
        ScheduledChangeRow {
            id: Uuid::new_v4(),
            entity_type: "experiment".to_string(),
            entity_id: Uuid::new_v4(),
            env_id: Uuid::new_v4(),
            mutation_payload: payload,
            schedule_kind: "one_shot".to_string(),
            scheduled_at: Some(chrono::Utc::now()),
            rrule: None,
            tz: None,
            next_run_at: Some(chrono::Utc::now()),
            last_run_at: None,
            status: stitchd_db::ScheduleStatus::Pending,
            version: 1,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            created_by: None,
        }
    }

    /// Stub that records the request it received and returns a scripted result.
    struct StubTransitioner {
        result: Mutex<Option<Status>>,
        seen: Mutex<Option<TransitionExperimentRequest>>,
    }
    impl StubTransitioner {
        fn ok() -> Self {
            Self {
                result: Mutex::new(None),
                seen: Mutex::new(None),
            }
        }
        fn err(status: Status) -> Self {
            Self {
                result: Mutex::new(Some(status)),
                seen: Mutex::new(None),
            }
        }
    }
    #[async_trait]
    impl ExperimentTransitioner for StubTransitioner {
        async fn transition_experiment(
            &self,
            req: TransitionExperimentRequest,
        ) -> Result<(), Status> {
            *self.seen.lock().unwrap() = Some(req);
            match self.result.lock().unwrap().clone() {
                Some(s) => Err(s),
                None => Ok(()),
            }
        }
    }

    #[tokio::test]
    async fn start_one_shot_applies_and_sends_active_status() {
        let payload = serde_json::json!({ "transition": "start", "reason": "scheduled launch" });
        let change = experiment_change(payload);
        let applier = ExperimentApplier::new(StubTransitioner::ok());
        let outcome = applier.apply(&change).await.unwrap();
        assert_eq!(outcome, ApplyOutcome::Applied);

        let seen = applier.transitioner.seen.lock().unwrap().clone().unwrap();
        assert_eq!(seen.new_status, ExperimentStatus::Active as i32);
        assert_eq!(seen.experiment_id, change.entity_id.to_string());
        assert_eq!(seen.environment_id, change.env_id.to_string());
        assert_eq!(seen.reason, "scheduled launch");
    }

    #[tokio::test]
    async fn stop_maps_to_concluded() {
        let payload = serde_json::json!({ "transition": "stop" });
        let change = experiment_change(payload);
        let applier = ExperimentApplier::new(StubTransitioner::ok());
        let outcome = applier.apply(&change).await.unwrap();
        assert_eq!(outcome, ApplyOutcome::Applied);
        let seen = applier.transitioner.seen.lock().unwrap().clone().unwrap();
        assert_eq!(seen.new_status, ExperimentStatus::Concluded as i32);
        // Absent reason defaults to empty string.
        assert_eq!(seen.reason, "");
    }

    #[tokio::test]
    async fn pause_and_resume_and_archive_map_correctly() {
        for (t, expected) in [
            ("pause", ExperimentStatus::Paused),
            ("resume", ExperimentStatus::Active),
            ("archive", ExperimentStatus::Concluded),
        ] {
            let change = experiment_change(serde_json::json!({ "transition": t }));
            let applier = ExperimentApplier::new(StubTransitioner::ok());
            assert_eq!(applier.apply(&change).await.unwrap(), ApplyOutcome::Applied);
            let seen = applier.transitioner.seen.lock().unwrap().clone().unwrap();
            assert_eq!(seen.new_status, expected as i32, "transition {t}");
        }
    }

    #[tokio::test]
    async fn invalid_transition_is_skipped_not_failed() {
        // Spec A9 / A3: starting an already-running experiment is an invalid
        // transition at fire time — experimentation-service returns
        // FAILED_PRECONDITION. It must be SKIPPED (recoverable; recurring advances).
        let status =
            Status::failed_precondition("Invalid status transition from Running to Running");
        let change = experiment_change(serde_json::json!({ "transition": "start" }));
        let applier = ExperimentApplier::new(StubTransitioner::err(status));
        let outcome = applier.apply(&change).await.unwrap();

        match outcome {
            ApplyOutcome::Skipped(reason) => {
                assert!(
                    reason.contains("Invalid status transition"),
                    "reason: {reason}"
                );
            }
            other => panic!("expected Skipped, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn unmet_start_prerequisite_is_skipped() {
        // Spec C4: a scheduled start with an unmet start-time prerequisite is
        // rejected by experimentation-service with FAILED_PRECONDITION → Skipped.
        let status = Status::failed_precondition(
            "experiment start prerequisite unmet: flag 'kill-switch' must serve variant 'on'",
        );
        let change = experiment_change(serde_json::json!({ "transition": "start" }));
        let applier = ExperimentApplier::new(StubTransitioner::err(status));
        match applier.apply(&change).await.unwrap() {
            ApplyOutcome::Skipped(reason) => {
                assert!(reason.contains("prerequisite unmet"), "reason: {reason}");
            }
            other => panic!("expected Skipped, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn conflicting_running_experiment_is_skipped() {
        // The uniqueness guard (another experiment already running on the same
        // flag) surfaces as ALREADY_EXISTS → Skipped (recoverable).
        let status = Status::already_exists("unique violation on: flag_id");
        let change = experiment_change(serde_json::json!({ "transition": "start" }));
        let applier = ExperimentApplier::new(StubTransitioner::err(status));
        assert!(matches!(
            applier.apply(&change).await.unwrap(),
            ApplyOutcome::Skipped(_)
        ));
    }

    #[tokio::test]
    async fn transport_error_is_failed() {
        let status = Status::unavailable("experimentation-service down");
        let change = experiment_change(serde_json::json!({ "transition": "start" }));
        let applier = ExperimentApplier::new(StubTransitioner::err(status));
        assert!(matches!(
            applier.apply(&change).await.unwrap(),
            ApplyOutcome::Failed(_)
        ));
    }

    #[tokio::test]
    async fn not_found_experiment_is_failed() {
        let status = Status::not_found("not found: <uuid>");
        let change = experiment_change(serde_json::json!({ "transition": "stop" }));
        let applier = ExperimentApplier::new(StubTransitioner::err(status));
        assert!(matches!(
            applier.apply(&change).await.unwrap(),
            ApplyOutcome::Failed(_)
        ));
    }

    #[tokio::test]
    async fn malformed_payload_is_failed_not_panic() {
        let change = experiment_change(serde_json::json!({ "not": "a transition" }));
        let applier = ExperimentApplier::new(StubTransitioner::ok());
        assert!(matches!(
            applier.apply(&change).await.unwrap(),
            ApplyOutcome::Failed(_)
        ));
    }

    #[tokio::test]
    async fn unknown_transition_kind_is_failed() {
        let change = experiment_change(serde_json::json!({ "transition": "teleport" }));
        let applier = ExperimentApplier::new(StubTransitioner::ok());
        assert!(matches!(
            applier.apply(&change).await.unwrap(),
            ApplyOutcome::Failed(_)
        ));
    }
}
