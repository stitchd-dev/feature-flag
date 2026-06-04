//! Flag apply path (Phase 3 Task 3).
//!
//! A due **flag** scheduled change is dispatched to flag-service's canonical
//! `MutateFlag` RPC, so the mutation flows through the same path a human edit
//! does: it bumps the flag's optimistic-concurrency version and writes an
//! audit-log entry. The scheduler client carries no end-user identity, so the
//! mutation is attributed to the system/scheduler actor in the audit trail.
//!
//! ## Experiment-lock handling (spec A9)
//! If the target flag is locked by a running/paused experiment, `MutateFlag`
//! returns `FAILED_PRECONDITION` carrying the
//! `flag_locked_by_experiment:<uuid>` sentinel (see
//! `stitchd_flag_service::error::FLAG_LOCKED_STATUS_PREFIX` and the gateway
//! decode in `stitchd_gateway::error`). We map that to
//! [`ApplyOutcome::Skipped`] with the sentinel as the reason — the run is
//! recorded `skipped`, the loop does NOT fail, and a recurring schedule proceeds
//! to its next window. Any other RPC error maps to [`ApplyOutcome::Failed`].
//!
//! ## Mutation payload
//! The stored `mutation_payload` is a JSON object the scheduler deserializes
//! into a [`FlagMutationPayload`] and rebuilds into a `MutateFlagRequest`. prost
//! messages are not serde-serializable, so this is a small explicit mirror of
//! the `MutateFlag` inputs the scheduler supports (enable/disable via
//! `enabled_override`, and a flag body for variant/metadata replacement). The
//! richer rule-replacement payloads are a clean future extension on this mirror.

use async_trait::async_trait;
use serde::Deserialize;
use tonic::Status;

use stitchd_db::ScheduledChangeRow;
use stitchd_proto::flags::v1::{
    FeatureFlag, FlagValueType, MutateFlagRequest, MutationKind, Variant, VariantValue,
    variant_value,
};

use crate::apply::{Applier, ApplyOutcome};

/// Sentinel prefix flag-service stamps onto a `FAILED_PRECONDITION` status when
/// the target flag is locked by a running/paused experiment. Mirrors
/// `stitchd_flag_service::error::FLAG_LOCKED_STATUS_PREFIX` and the gateway
/// decode convention.
pub const FLAG_LOCKED_STATUS_PREFIX: &str = "flag_locked_by_experiment:";

/// Abstraction over the flag-service `MutateFlag` RPC so the apply path is
/// unit-testable with a stub (no live flag-service).
#[async_trait]
pub trait FlagMutator: Send + Sync {
    /// Invoke `MutateFlag`. Returns the gRPC status on failure (the apply path
    /// inspects it for the experiment-lock sentinel).
    async fn mutate_flag(&self, req: MutateFlagRequest) -> Result<(), Status>;
}

/// Production [`FlagMutator`] backed by a tonic flag-service client. The
/// scheduler carries no end-user identity on the request, so flag-service
/// attributes the mutation to the system/scheduler actor in the audit log.
pub struct GrpcFlagMutator {
    client: std::sync::Arc<
        tokio::sync::Mutex<
            stitchd_proto::flags::v1::flag_service_client::FlagServiceClient<
                tonic::transport::Channel,
            >,
        >,
    >,
}

impl GrpcFlagMutator {
    /// Construct a mutator over a shared flag-service client.
    #[must_use]
    pub const fn new(
        client: std::sync::Arc<
            tokio::sync::Mutex<
                stitchd_proto::flags::v1::flag_service_client::FlagServiceClient<
                    tonic::transport::Channel,
                >,
            >,
        >,
    ) -> Self {
        Self { client }
    }
}

#[async_trait]
impl FlagMutator for GrpcFlagMutator {
    async fn mutate_flag(&self, req: MutateFlagRequest) -> Result<(), Status> {
        let mut client = self.client.lock().await;
        client.mutate_flag(tonic::Request::new(req)).await?;
        Ok(())
    }
}

/// Applies due flag changes via a [`FlagMutator`].
pub struct FlagApplier<M: FlagMutator> {
    mutator: M,
}

impl<M: FlagMutator> FlagApplier<M> {
    /// Construct a flag applier over `mutator`.
    pub const fn new(mutator: M) -> Self {
        Self { mutator }
    }
}

#[async_trait]
impl<M: FlagMutator> Applier for FlagApplier<M> {
    async fn apply(&self, change: &ScheduledChangeRow) -> anyhow::Result<ApplyOutcome> {
        // Deserialize the stored payload. A malformed payload is a permanent
        // (non-retryable) failure for this change — record it as failed.
        let payload: FlagMutationPayload =
            match serde_json::from_value(change.mutation_payload.clone()) {
                Ok(p) => p,
                Err(e) => {
                    return Ok(ApplyOutcome::Failed(format!(
                        "invalid flag mutation payload: {e}"
                    )));
                }
            };

        let req = match payload.into_request(change) {
            Ok(r) => r,
            Err(e) => return Ok(ApplyOutcome::Failed(e)),
        };

        match self.mutator.mutate_flag(req).await {
            Ok(()) => Ok(ApplyOutcome::Applied),
            Err(status) => Ok(classify_status(&status)),
        }
    }
}

/// Map a `MutateFlag` error status to an [`ApplyOutcome`]. The experiment-lock
/// sentinel becomes a recoverable [`ApplyOutcome::Skipped`] (carrying the
/// sentinel so the run history / UI can surface the locking experiment);
/// everything else is a [`ApplyOutcome::Failed`].
fn classify_status(status: &Status) -> ApplyOutcome {
    if status.code() == tonic::Code::FailedPrecondition
        && status.message().starts_with(FLAG_LOCKED_STATUS_PREFIX)
    {
        // Preserve the full sentinel (`flag_locked_by_experiment:<uuid>`) as the
        // skip reason so it round-trips into the run-history `detail`.
        ApplyOutcome::Skipped(status.message().to_string())
    } else {
        ApplyOutcome::Failed(format!("{}: {}", status.code(), status.message()))
    }
}

/// JSON shape stored in `scheduled_changes.mutation_payload` for a flag change.
#[derive(Debug, Clone, Deserialize)]
pub struct FlagMutationPayload {
    /// Mutation kind (e.g. `update`, `archive`, `replace_variants`).
    pub kind: PayloadMutationKind,
    /// Optimistic-concurrency version expected by flag-service for
    /// UPDATE/DELETE/ARCHIVE. 0 for CREATE.
    #[serde(default)]
    pub version: u64,
    /// Project-scoped admin operations set this (mutually exclusive with
    /// `environment_id`).
    #[serde(default)]
    pub project_id: Option<String>,
    /// Environment-scoped operations set this.
    #[serde(default)]
    pub environment_id: Option<String>,
    /// Override the flag's enabled state (enable/disable). When absent, the
    /// current state is preserved.
    #[serde(default)]
    pub enabled_override: Option<bool>,
    /// Optional flag body for variant/metadata replacement.
    #[serde(default)]
    pub flag: Option<FlagBody>,
}

/// The mutation kinds the scheduler can dispatch.
#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PayloadMutationKind {
    /// Update flag fields.
    Update,
    /// Archive (soft-delete) the flag.
    Archive,
    /// Restore an archived flag.
    Restore,
    /// Replace the flag's variants list.
    ReplaceVariants,
    /// Replace the flag's rules list.
    ReplaceRules,
}

impl PayloadMutationKind {
    const fn to_proto(self) -> MutationKind {
        match self {
            Self::Update => MutationKind::Update,
            Self::Archive => MutationKind::Archive,
            Self::Restore => MutationKind::Restore,
            Self::ReplaceVariants => MutationKind::ReplaceVariants,
            Self::ReplaceRules => MutationKind::ReplaceRules,
        }
    }
}

/// A serde mirror of the proto `FeatureFlag` fields the scheduler supports.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct FlagBody {
    /// Flag key (or `flag_id` for an admin update path).
    #[serde(default)]
    pub key: String,
    /// Flag UUID (admin update path).
    #[serde(default)]
    pub flag_id: String,
    /// Display name.
    #[serde(default)]
    pub name: String,
    /// Description.
    #[serde(default)]
    pub description: String,
    /// Enabled state.
    #[serde(default)]
    pub enabled: bool,
    /// Value type (`bool` | `int` | `double` | `string` | `json`).
    #[serde(default)]
    pub value_type: Option<String>,
    /// Variants list (for `replace_variants`).
    #[serde(default)]
    pub variants: Vec<VariantBody>,
    /// Default-variant key.
    #[serde(default)]
    pub default_variant_key: String,
}

/// A serde mirror of the proto `Variant` + `VariantValue`.
#[derive(Debug, Clone, Deserialize)]
pub struct VariantBody {
    /// Variant key.
    pub key: String,
    /// Variant value (one of the typed forms).
    pub value: VariantValueBody,
}

/// A serde mirror of the proto `VariantValue` oneof.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VariantValueBody {
    /// Boolean value.
    Bool(bool),
    /// Integer value.
    Int(i64),
    /// Double value.
    Double(f64),
    /// String value.
    Str(String),
    /// JSON value, serialized as a string.
    Json(String),
}

impl VariantValueBody {
    fn to_proto(&self) -> VariantValue {
        let value = match self {
            Self::Bool(b) => variant_value::Value::BoolValue(*b),
            Self::Int(i) => variant_value::Value::IntValue(*i),
            Self::Double(d) => variant_value::Value::DoubleValue(*d),
            Self::Str(s) => variant_value::Value::StringValue(s.clone()),
            Self::Json(j) => variant_value::Value::JsonValue(j.clone()),
        };
        VariantValue { value: Some(value) }
    }
}

fn value_type_to_proto(s: Option<&str>) -> FlagValueType {
    match s {
        Some("bool") => FlagValueType::Bool,
        Some("int") => FlagValueType::Int,
        Some("double") => FlagValueType::Double,
        Some("string") => FlagValueType::String,
        Some("json") => FlagValueType::Json,
        _ => FlagValueType::Unspecified,
    }
}

impl FlagMutationPayload {
    /// Build the `MutateFlagRequest` for `change`. Requires exactly one of
    /// `project_id` / `environment_id`.
    fn into_request(self, change: &ScheduledChangeRow) -> Result<MutateFlagRequest, String> {
        if self.project_id.is_none() && self.environment_id.is_none() {
            return Err("flag mutation payload needs project_id or environment_id".to_string());
        }

        let flag = self.flag.map(|b| FeatureFlag {
            key: if b.key.is_empty() {
                // Fall back to the entity_id so update/archive targeting works
                // even when the payload omits the key.
                change.entity_id.to_string()
            } else {
                b.key
            },
            flag_id: b.flag_id,
            name: b.name,
            description: b.description,
            enabled: b.enabled,
            value_type: value_type_to_proto(b.value_type.as_deref()) as i32,
            variants: b
                .variants
                .into_iter()
                .map(|v| Variant {
                    key: v.key,
                    value: Some(v.value.to_proto()),
                })
                .collect(),
            default_variant_key: b.default_variant_key,
            ..Default::default()
        });

        Ok(MutateFlagRequest {
            environment_id: self.environment_id.unwrap_or_default(),
            project_id: self.project_id.unwrap_or_default(),
            kind: self.kind.to_proto() as i32,
            flag,
            version: self.version,
            enabled_override: self.enabled_override,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use uuid::Uuid;

    fn flag_change(payload: serde_json::Value) -> ScheduledChangeRow {
        ScheduledChangeRow {
            id: Uuid::new_v4(),
            entity_type: "flag".to_string(),
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
    struct StubMutator {
        result: Mutex<Option<Status>>,
        seen: Mutex<Option<MutateFlagRequest>>,
    }
    impl StubMutator {
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
    impl FlagMutator for StubMutator {
        async fn mutate_flag(&self, req: MutateFlagRequest) -> Result<(), Status> {
            *self.seen.lock().unwrap() = Some(req);
            match self.result.lock().unwrap().clone() {
                Some(s) => Err(s),
                None => Ok(()),
            }
        }
    }

    #[tokio::test]
    async fn disable_one_shot_applies_and_sends_enabled_override() {
        let payload = serde_json::json!({
            "kind": "update",
            "version": 3,
            "environment_id": "11111111-1111-1111-1111-111111111111",
            "enabled_override": false,
        });
        let change = flag_change(payload);
        let applier = FlagApplier::new(StubMutator::ok());
        let outcome = applier.apply(&change).await.unwrap();
        assert_eq!(outcome, ApplyOutcome::Applied);

        let seen = applier.mutator.seen.lock().unwrap().clone().unwrap();
        assert_eq!(seen.enabled_override, Some(false));
        assert_eq!(seen.version, 3);
        assert_eq!(seen.kind, MutationKind::Update as i32);
    }

    #[tokio::test]
    async fn locked_flag_is_skipped_with_sentinel_reason() {
        // Spec A9: a flag locked by an experiment must be SKIPPED, not failed.
        let exp_id = "22222222-2222-2222-2222-222222222222";
        let sentinel = format!("{FLAG_LOCKED_STATUS_PREFIX}{exp_id}");
        let status = Status::failed_precondition(sentinel.clone());

        let payload = serde_json::json!({
            "kind": "update",
            "version": 1,
            "environment_id": "11111111-1111-1111-1111-111111111111",
            "enabled_override": true,
        });
        let change = flag_change(payload);
        let applier = FlagApplier::new(StubMutator::err(status));
        let outcome = applier.apply(&change).await.unwrap();

        match outcome {
            ApplyOutcome::Skipped(reason) => {
                assert_eq!(reason, sentinel, "skip reason carries the lock sentinel");
                assert!(reason.contains(exp_id));
            }
            other => panic!("expected Skipped, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn other_grpc_error_is_failed() {
        let status = Status::not_found("flag gone");
        let payload = serde_json::json!({
            "kind": "update",
            "version": 1,
            "project_id": "11111111-1111-1111-1111-111111111111",
        });
        let change = flag_change(payload);
        let applier = FlagApplier::new(StubMutator::err(status));
        let outcome = applier.apply(&change).await.unwrap();
        assert!(matches!(outcome, ApplyOutcome::Failed(_)));
    }

    #[tokio::test]
    async fn malformed_payload_is_failed_not_panic() {
        let change = flag_change(serde_json::json!({"not": "a valid payload"}));
        let applier = FlagApplier::new(StubMutator::ok());
        let outcome = applier.apply(&change).await.unwrap();
        assert!(matches!(outcome, ApplyOutcome::Failed(_)));
    }

    #[tokio::test]
    async fn replace_variants_builds_flag_body() {
        let payload = serde_json::json!({
            "kind": "replace_variants",
            "version": 2,
            "project_id": "11111111-1111-1111-1111-111111111111",
            "flag": {
                "key": "my-flag",
                "value_type": "string",
                "variants": [
                    { "key": "a", "value": { "str": "alpha" } },
                    { "key": "b", "value": { "bool": true } }
                ]
            }
        });
        let change = flag_change(payload);
        let applier = FlagApplier::new(StubMutator::ok());
        let outcome = applier.apply(&change).await.unwrap();
        assert_eq!(outcome, ApplyOutcome::Applied);

        let seen = applier.mutator.seen.lock().unwrap().clone().unwrap();
        assert_eq!(seen.kind, MutationKind::ReplaceVariants as i32);
        let flag = seen.flag.unwrap();
        assert_eq!(flag.key, "my-flag");
        assert_eq!(flag.variants.len(), 2);
        assert_eq!(flag.variants[0].key, "a");
    }

    #[tokio::test]
    async fn missing_scope_is_failed() {
        let payload = serde_json::json!({ "kind": "update", "version": 1 });
        let change = flag_change(payload);
        let applier = FlagApplier::new(StubMutator::ok());
        let outcome = applier.apply(&change).await.unwrap();
        assert!(matches!(outcome, ApplyOutcome::Failed(_)));
    }
}
