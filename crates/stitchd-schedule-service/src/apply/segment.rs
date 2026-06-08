//! Segment apply path (Phase 5 Task 3).
//!
//! A due **segment** scheduled change is dispatched to segmentation-service's
//! canonical `UpdateAdminSegment` RPC, so the definition update flows through the
//! same admin path a human edit does: it bumps the segment's
//! optimistic-concurrency version and writes the new definition. The target
//! segment comes from the scheduled-change row's `entity_id`.
//!
//! ## Scope: rule-expression (definition) update (spec A4)
//! The supported scheduled mutation is a **definition update**: a new rule
//! condition-expression (plus optional name/description/tags/context_type/exclude
//! keys) swapped in via `UpdateAdminSegment`. `condition_expr` is the JSON-encoded
//! `ConditionExpr` (bytes on the wire), carried in the stored payload as a JSON
//! string the scheduler passes through verbatim.
//!
//! ## List-generation activation (spec A4)
//! For **list-based** segments, a scheduled change may instead "activate a prepared
//! generation": the payload's `kind` is `list_generation` and it carries the full
//! prepared `include`/`exclude` member set. The scheduler dispatches it to the
//! segmentation-service `ActivateListGeneration` RPC, which atomically full-replaces
//! the member set via the ScyllaDB generation-swap (fresh generation + CAS pointer
//! flip) — so the scheduled swap actually fires (flag_lifecycle_20260604 Phase 10).
//!
//! ## Outcome classification
//! Mirrors the flag/experiment apply paths: a stale-version conflict
//! (`ABORTED` / `FAILED_PRECONDITION`) is a recoverable [`ApplyOutcome::Skipped`]
//! (a recurring schedule advances to its next window); a `NOT_FOUND` /
//! `INVALID_ARGUMENT` / transport error is a non-recoverable
//! [`ApplyOutcome::Failed`].

use async_trait::async_trait;
use serde::Deserialize;
use tonic::Status;

use stitchd_db::ScheduledChangeRow;
use stitchd_proto::segments::v1::{ActivateListGenerationRequest, UpdateAdminSegmentRequest};

use crate::apply::{Applier, ApplyOutcome};

/// Abstraction over the segmentation-service RPCs used by the apply path so it is
/// unit-testable with a stub (no live segmentation-service).
#[async_trait]
pub trait SegmentUpdater: Send + Sync {
    /// Invoke `UpdateAdminSegment` (definition update). Returns the gRPC status on
    /// failure (the apply path inspects its code to classify the outcome).
    async fn update_segment(&self, req: UpdateAdminSegmentRequest) -> Result<(), Status>;

    /// Invoke `ActivateListGeneration` (list-segment generation swap). Returns the
    /// gRPC status on failure.
    async fn activate_list_generation(
        &self,
        req: ActivateListGenerationRequest,
    ) -> Result<(), Status>;
}

/// Production [`SegmentUpdater`] backed by a tonic segmentation-service client.
pub struct GrpcSegmentUpdater {
    client: std::sync::Arc<
        tokio::sync::Mutex<
            stitchd_proto::segments::v1::segmentation_service_client::SegmentationServiceClient<
                tonic::transport::Channel,
            >,
        >,
    >,
}

impl GrpcSegmentUpdater {
    /// Construct an updater over a shared segmentation-service client.
    #[must_use]
    pub const fn new(
        client: std::sync::Arc<
            tokio::sync::Mutex<
                stitchd_proto::segments::v1::segmentation_service_client::SegmentationServiceClient<
                    tonic::transport::Channel,
                >,
            >,
        >,
    ) -> Self {
        Self { client }
    }
}

#[async_trait]
impl SegmentUpdater for GrpcSegmentUpdater {
    async fn update_segment(&self, req: UpdateAdminSegmentRequest) -> Result<(), Status> {
        let mut client = self.client.lock().await;
        client
            .update_admin_segment(tonic::Request::new(req))
            .await?;
        Ok(())
    }

    async fn activate_list_generation(
        &self,
        req: ActivateListGenerationRequest,
    ) -> Result<(), Status> {
        let mut client = self.client.lock().await;
        client
            .activate_list_generation(tonic::Request::new(req))
            .await?;
        Ok(())
    }
}

/// Applies due segment changes via a [`SegmentUpdater`].
pub struct SegmentApplier<U: SegmentUpdater> {
    updater: U,
}

impl<U: SegmentUpdater> SegmentApplier<U> {
    /// Construct a segment applier over `updater`.
    pub const fn new(updater: U) -> Self {
        Self { updater }
    }
}

#[async_trait]
impl<U: SegmentUpdater> Applier for SegmentApplier<U> {
    async fn apply(&self, change: &ScheduledChangeRow) -> anyhow::Result<ApplyOutcome> {
        let payload: SegmentMutationPayload =
            match serde_json::from_value(change.mutation_payload.clone()) {
                Ok(p) => p,
                Err(e) => {
                    return Ok(ApplyOutcome::Failed(format!(
                        "invalid segment mutation payload: {e}"
                    )));
                }
            };

        let result = match payload.kind {
            SegmentMutationKind::DefinitionUpdate => {
                self.updater
                    .update_segment(payload.into_update_request(change))
                    .await
            }
            SegmentMutationKind::ListGeneration => {
                self.updater
                    .activate_list_generation(payload.into_activation_request(change))
                    .await
            }
        };

        match result {
            Ok(()) => Ok(ApplyOutcome::Applied),
            Err(status) => Ok(classify_status(&status)),
        }
    }
}

/// Map an RPC error status to an [`ApplyOutcome`]. A stale-version
/// conflict is recoverable (`Skipped` — a recurring schedule advances); everything
/// else is `Failed`.
fn classify_status(status: &Status) -> ApplyOutcome {
    match status.code() {
        tonic::Code::Aborted | tonic::Code::FailedPrecondition => {
            ApplyOutcome::Skipped(format!("{}: {}", status.code(), status.message()))
        }
        code => ApplyOutcome::Failed(format!("{}: {}", code, status.message())),
    }
}

/// JSON shape stored in `scheduled_changes.mutation_payload` for a segment change.
#[derive(Debug, Clone, Deserialize)]
pub struct SegmentMutationPayload {
    /// Mutation kind. `definition_update` dispatches `UpdateAdminSegment`;
    /// `list_generation` dispatches `ActivateListGeneration` (the prepared
    /// member-set swap).
    #[serde(default = "default_segment_kind")]
    pub kind: SegmentMutationKind,
    /// Optimistic-concurrency version expected by segmentation-service.
    #[serde(default)]
    pub version: u64,
    /// New display name (empty preserves nothing — UpdateAdminSegment replaces).
    #[serde(default)]
    pub name: String,
    /// New description.
    #[serde(default)]
    pub description: String,
    /// Tags.
    #[serde(default)]
    pub tags: Vec<String>,
    /// JSON-encoded `ConditionExpr` (rule-expression). Passed through to the
    /// `condition_expr` bytes field verbatim.
    #[serde(default)]
    pub condition_expr: String,
    /// Context kind this list targets; defaults to "user" server-side when empty.
    #[serde(default)]
    pub context_type: String,
    /// Keys to explicitly exclude (list-based definition update only).
    #[serde(default)]
    pub excluded_keys: Vec<String>,
    /// `list_generation` only: the full prepared include-list to activate.
    #[serde(default)]
    pub include: Vec<String>,
    /// `list_generation` only: the full prepared exclude-list to activate.
    #[serde(default)]
    pub exclude: Vec<String>,
}

const fn default_segment_kind() -> SegmentMutationKind {
    SegmentMutationKind::DefinitionUpdate
}

/// The segment mutation kinds the scheduler recognizes.
#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SegmentMutationKind {
    /// Swap in a new rule-expression / definition via `UpdateAdminSegment`.
    DefinitionUpdate,
    /// Activate a prepared list generation via `ActivateListGeneration`.
    ListGeneration,
}

impl SegmentMutationPayload {
    /// Build the `UpdateAdminSegmentRequest` for a `definition_update` change.
    fn into_update_request(self, change: &ScheduledChangeRow) -> UpdateAdminSegmentRequest {
        UpdateAdminSegmentRequest {
            segment_id: change.entity_id.to_string(),
            name: self.name,
            description: self.description,
            tags: self.tags,
            condition_expr: self.condition_expr.into_bytes(),
            user_list: Vec::new(),
            version: self.version,
            context_type: self.context_type,
            excluded_keys: self.excluded_keys,
        }
    }

    /// Build the `ActivateListGenerationRequest` for a `list_generation` change.
    fn into_activation_request(self, change: &ScheduledChangeRow) -> ActivateListGenerationRequest {
        ActivateListGenerationRequest {
            segment_id: change.entity_id.to_string(),
            context_type: self.context_type,
            include: self.include,
            exclude: self.exclude,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use uuid::Uuid;

    fn segment_change(payload: serde_json::Value) -> ScheduledChangeRow {
        ScheduledChangeRow {
            id: Uuid::new_v4(),
            entity_type: "segment".to_string(),
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

    struct StubUpdater {
        result: Mutex<Option<Status>>,
        seen: Mutex<Option<UpdateAdminSegmentRequest>>,
        seen_activation: Mutex<Option<ActivateListGenerationRequest>>,
    }
    impl StubUpdater {
        fn ok() -> Self {
            Self {
                result: Mutex::new(None),
                seen: Mutex::new(None),
                seen_activation: Mutex::new(None),
            }
        }
        fn err(status: Status) -> Self {
            Self {
                result: Mutex::new(Some(status)),
                seen: Mutex::new(None),
                seen_activation: Mutex::new(None),
            }
        }
    }
    #[async_trait]
    impl SegmentUpdater for StubUpdater {
        async fn update_segment(&self, req: UpdateAdminSegmentRequest) -> Result<(), Status> {
            *self.seen.lock().unwrap() = Some(req);
            match self.result.lock().unwrap().clone() {
                Some(s) => Err(s),
                None => Ok(()),
            }
        }
        async fn activate_list_generation(
            &self,
            req: ActivateListGenerationRequest,
        ) -> Result<(), Status> {
            *self.seen_activation.lock().unwrap() = Some(req);
            match self.result.lock().unwrap().clone() {
                Some(s) => Err(s),
                None => Ok(()),
            }
        }
    }

    #[tokio::test]
    async fn definition_update_applies_and_passes_condition_expr() {
        let payload = serde_json::json!({
            "kind": "definition_update",
            "version": 4,
            "name": "EU users",
            "condition_expr": "{\"op\":\"eq\",\"field\":\"country\",\"value\":\"DE\"}",
            "context_type": "user",
        });
        let change = segment_change(payload);
        let applier = SegmentApplier::new(StubUpdater::ok());
        assert_eq!(applier.apply(&change).await.unwrap(), ApplyOutcome::Applied);

        let seen = applier.updater.seen.lock().unwrap().clone().unwrap();
        assert_eq!(seen.segment_id, change.entity_id.to_string());
        assert_eq!(seen.version, 4);
        assert_eq!(seen.name, "EU users");
        assert_eq!(
            String::from_utf8(seen.condition_expr).unwrap(),
            "{\"op\":\"eq\",\"field\":\"country\",\"value\":\"DE\"}"
        );
    }

    #[tokio::test]
    async fn kind_defaults_to_definition_update() {
        // Omitting `kind` defaults to a definition update.
        let payload = serde_json::json!({ "version": 1, "condition_expr": "{}" });
        let change = segment_change(payload);
        let applier = SegmentApplier::new(StubUpdater::ok());
        assert_eq!(applier.apply(&change).await.unwrap(), ApplyOutcome::Applied);
    }

    #[tokio::test]
    async fn list_generation_kind_activates_prepared_generation() {
        // Phase 10.3: a list_generation payload now fires ActivateListGeneration
        // with the prepared include/exclude member set (was rejected before).
        let payload = serde_json::json!({
            "kind": "list_generation",
            "context_type": "user",
            "include": ["alice", "bob"],
            "exclude": ["mallory"],
        });
        let change = segment_change(payload);
        let applier = SegmentApplier::new(StubUpdater::ok());
        assert_eq!(applier.apply(&change).await.unwrap(), ApplyOutcome::Applied);

        let seen = applier
            .updater
            .seen_activation
            .lock()
            .unwrap()
            .clone()
            .expect("activation dispatched");
        assert_eq!(seen.segment_id, change.entity_id.to_string());
        assert_eq!(seen.context_type, "user");
        assert_eq!(seen.include, vec!["alice", "bob"]);
        assert_eq!(seen.exclude, vec!["mallory"]);
        // No definition-update RPC was sent.
        assert!(applier.updater.seen.lock().unwrap().is_none());
    }

    #[tokio::test]
    async fn list_generation_activation_failure_is_classified() {
        // A NOT_FOUND from the activation RPC is non-recoverable (Failed).
        let payload = serde_json::json!({
            "kind": "list_generation",
            "include": ["x"],
        });
        let change = segment_change(payload);
        let applier = SegmentApplier::new(StubUpdater::err(Status::not_found("segment gone")));
        assert!(matches!(
            applier.apply(&change).await.unwrap(),
            ApplyOutcome::Failed(_)
        ));
    }

    #[tokio::test]
    async fn stale_version_is_skipped() {
        // A version conflict is recoverable — Skipped (recurring advances).
        let status = Status::aborted("version conflict: expected 4, actual 5");
        let change = segment_change(serde_json::json!({ "version": 4, "condition_expr": "{}" }));
        let applier = SegmentApplier::new(StubUpdater::err(status));
        assert!(matches!(
            applier.apply(&change).await.unwrap(),
            ApplyOutcome::Skipped(_)
        ));
    }

    #[tokio::test]
    async fn not_found_is_failed() {
        let status = Status::not_found("segment gone");
        let change = segment_change(serde_json::json!({ "version": 1, "condition_expr": "{}" }));
        let applier = SegmentApplier::new(StubUpdater::err(status));
        assert!(matches!(
            applier.apply(&change).await.unwrap(),
            ApplyOutcome::Failed(_)
        ));
    }

    #[tokio::test]
    async fn malformed_payload_is_failed_not_panic() {
        let change = segment_change(serde_json::json!({ "kind": "bogus" }));
        let applier = SegmentApplier::new(StubUpdater::ok());
        assert!(matches!(
            applier.apply(&change).await.unwrap(),
            ApplyOutcome::Failed(_)
        ));
    }
}
