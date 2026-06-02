//! gRPC service implementation for `ExperimentationService`.

use std::sync::Arc;

use async_trait::async_trait;
use chrono::Utc;
use tonic::{Request, Response, Status};
use tracing::instrument;

use stitchd_core::{
    experimentation::{Experiment, ExclusionGroup, ExperimentStatus},
    id::{EnvironmentId, ExclusionGroupId, ExperimentId, ExperimentIterationId, RuleId},
    rule_engine::types::ExclusionGate,
};
use stitchd_db::{
    ExperimentRepository, RepositoryError, StatsScheduleRepository,
    repository::pg::ExclusionGroupRepository,
};
use stitchd_proto::experiments::v1::{
    BoundTarget, ContextTypeResults, CreateExperimentRequest, DeleteExperimentRequest,
    ExperimentIteration as ProtoIteration, ExperimentResults, GetExperimentIterationRequest,
    GetExperimentRequest, GetResultsRequest, ListExperimentsRequest, ListExperimentsResponse,
    ListIterationsRequest, ListIterationsResponse, ListRunningExperimentsRequest,
    RunningExperiment, TransitionExperimentRequest, UpdateExperimentRequest,
    UpdateIterationLastComputedRequest, UpdateIterationLastComputedResponse, VariantResult,
    experimentation_service_server::ExperimentationService,
};

use crate::analytics_client::AnalyticsResultsPort;
use crate::dict_refresh::{DictionaryRefresher, spawn_refresh};
use crate::flag_client::FlagClient;

// ---------------------------------------------------------------------------
// Experiment binding validation (GL-08)
// ---------------------------------------------------------------------------

/// Inputs for the experiment binding invariant checks.
struct BindingInputs<'a> {
    environment_id: &'a str,
    flag_key: Option<&'a str>,
    flag_rule_id: Option<&'a str>,
    targets_default_rule: bool,
    unit_context_types: &'a [String],
}

/// Validate the four binding invariants and return the resolved `flag_id`
/// string on success.
///
/// Returns `Status::invalid_argument` on any violation — the gateway maps
/// that to HTTP 400.
///
/// When `flag_client` is `None` the flag checks are skipped (matches the
/// existing behaviour where the service is started without flag-service
/// connectivity).
#[allow(clippy::result_large_err)]
async fn validate_experiment_binding(
    inputs: &BindingInputs<'_>,
    analytics_client: &dyn AnalyticsResultsPort,
    flag_client: Option<&FlagClient>,
) -> Result<String, Status> {
    // ── XOR — exactly one of flag_rule_id / targets_default_rule must be set.
    let has_rule_id = inputs.flag_rule_id.is_some();
    if has_rule_id && inputs.targets_default_rule {
        return Err(Status::invalid_argument(
            "set exactly one of flag_rule_id or targets_default_rule, not both",
        ));
    }
    if !has_rule_id && !inputs.targets_default_rule {
        return Err(Status::invalid_argument(
            "experiment must bind to either a flag_rule_id (percentage rollout) or the flag's default rule (targets_default_rule=true)",
        ));
    }

    // ── Unit context types — at least one entry, all known to the env's registry.
    if inputs.unit_context_types.is_empty() {
        return Err(Status::invalid_argument(
            "unit_context_types must contain at least one entry",
        ));
    }

    let registered = analytics_client
        .list_context_types(inputs.environment_id)
        .await?;
    let registered_set: std::collections::HashSet<_> = registered.into_iter().collect();
    for ct in inputs.unit_context_types {
        if !registered_set.contains(ct) {
            return Err(Status::invalid_argument(format!(
                "context type '{ct}' is not registered for this environment"
            )));
        }
    }

    // ── Flag lookup — fetch the flag to obtain its UUID (flag_id) and
    // validate the rule kind when `flag_rule_id` is set.
    let flag_key = inputs
        .flag_key
        .ok_or_else(|| Status::invalid_argument("flag_key is required"))?;

    let flag_id = if let Some(fc) = flag_client {
        let flag = fc.get_flag(inputs.environment_id, flag_key).await?;

        // ── Rule kind — when bound to a flag rule, the rule's output must be a
        // percentage rollout (Allocation).
        if inputs.flag_rule_id.is_some() {
            let any_percentage_rule = flag.rules.iter().any(|r| {
                matches!(
                    r.output,
                    Some(stitchd_proto::flags::v1::flag_rule::Output::Allocation(_))
                )
            });
            if !any_percentage_rule {
                return Err(Status::invalid_argument(
                    "the bound rule must produce a percentage rollout (allocation); specific-variant rules are not eligible",
                ));
            }
        }

        flag.flag_id
    } else {
        // No flag client — skip flag-side validation, use empty flag_id.
        String::new()
    };

    Ok(flag_id)
}

// ---------------------------------------------------------------------------
// Status mapping helpers
// ---------------------------------------------------------------------------

/// Map proto `ExperimentStatus` integer to core [`ExperimentStatus`].
#[allow(clippy::result_large_err)]
fn proto_status_to_core(status: i32) -> Result<ExperimentStatus, Status> {
    use stitchd_proto::experiments::v1::ExperimentStatus as ProtoStatus;
    match ProtoStatus::try_from(status).unwrap_or(ProtoStatus::Unspecified) {
        ProtoStatus::Unspecified | ProtoStatus::Draft => Ok(ExperimentStatus::Draft),
        ProtoStatus::Active => Ok(ExperimentStatus::Running),
        ProtoStatus::Paused => Ok(ExperimentStatus::Paused),
        ProtoStatus::Concluded => Ok(ExperimentStatus::Stopped),
    }
}

/// Map core [`ExperimentStatus`] to proto integer.
fn core_status_to_proto(status: ExperimentStatus) -> i32 {
    use stitchd_proto::experiments::v1::ExperimentStatus as ProtoStatus;
    match status {
        ExperimentStatus::Draft => ProtoStatus::Draft as i32,
        ExperimentStatus::Running => ProtoStatus::Active as i32,
        ExperimentStatus::Paused => ProtoStatus::Paused as i32,
        ExperimentStatus::Stopped => ProtoStatus::Concluded as i32,
    }
}

/// Map a core [`stitchd_core::experimentation::ExperimentIteration`] to proto.
fn iteration_to_proto(i: &stitchd_core::experimentation::ExperimentIteration) -> ProtoIteration {
    ProtoIteration {
        id: i.id.to_string(),
        experiment_id: i.experiment_id.to_string(),
        iteration_number: i.iteration_number,
        started_at_ms: i.started_at.timestamp_millis(),
        ended_at_ms: i.ended_at.map_or(0, |t| t.timestamp_millis()),
        metric_ids: i.metric_ids.iter().map(ToString::to_string).collect(),
        traffic_allocation: i.traffic_allocation,
        unit_context_types: i.unit_context_types.clone(),
        exclusion_group_id: i.exclusion_group_id.map(|g| g.to_string()),
        group_bucket_lo: i.group_bucket_lo.map(u32::from),
        group_bucket_hi: i.group_bucket_hi.map(u32::from),
    }
}

/// Map an SRM JSON payload (as written by stats-service) into the proto
/// [`stitchd_proto::experiments::v1::SrmResult`] message.
///
/// Expected JSON shape (matches `stitchd_core::experimentation::stats::srm::SrmResult`):
/// ```json
/// {
///   "per_variant": [
///     { "variant_key": "control", "observed": 100, "expected": 100.0, "chi_sq_contribution": 0.0 }
///   ],
///   "overall_chi_sq": 0.0,
///   "overall_chi_sq_p": 1.0,
///   "health": "green" | "yellow" | "red"
/// }
/// ```
fn srm_json_to_proto(val: &serde_json::Value) -> Option<stitchd_proto::experiments::v1::SrmResult> {
    use stitchd_proto::experiments::v1::{SrmPerVariant, SrmResult};
    let obj = val.as_object()?;
    let per_variant = obj
        .get("per_variant")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .map(|row| SrmPerVariant {
                    variant_key: row
                        .get("variant_key")
                        .and_then(|s| s.as_str())
                        .unwrap_or_default()
                        .to_string(),
                    observed: row
                        .get("observed")
                        .and_then(serde_json::Value::as_u64)
                        .unwrap_or(0),
                    expected: row
                        .get("expected")
                        .and_then(serde_json::Value::as_f64)
                        .unwrap_or(0.0),
                    chi_sq_contribution: row
                        .get("chi_sq_contribution")
                        .and_then(serde_json::Value::as_f64)
                        .unwrap_or(0.0),
                })
                .collect()
        })
        .unwrap_or_default();
    Some(SrmResult {
        per_variant,
        overall_chi_sq: obj
            .get("overall_chi_sq")
            .and_then(serde_json::Value::as_f64)
            .unwrap_or(0.0),
        overall_chi_sq_p: obj
            .get("overall_chi_sq_p")
            .and_then(serde_json::Value::as_f64)
            .unwrap_or(1.0),
        health: obj
            .get("health")
            .and_then(|s| s.as_str())
            .unwrap_or("green")
            .to_lowercase(),
    })
}

/// Map a core [`Experiment`] to the proto [`stitchd_proto::experiments::v1::Experiment`] message.
fn core_to_proto(e: &Experiment) -> stitchd_proto::experiments::v1::Experiment {
    stitchd_proto::experiments::v1::Experiment {
        id: e.id.to_string(),
        environment_id: e.environment_id.to_string(),
        name: e.name.clone(),
        description: e.description.clone().unwrap_or_default(),
        flag_key: e.flag_key.clone().unwrap_or_default(),
        status: core_status_to_proto(e.status),
        variant_keys: e.variant_keys.clone(),
        created_at_ms: e.created_at.timestamp_millis(),
        updated_at_ms: e.updated_at.timestamp_millis(),
        version: u64::try_from(e.version).unwrap_or(1),
        flag_id: e.flag_id.to_string(),
        flag_rule_id: e
            .flag_rule_id
            .as_ref()
            .map(|id| id.to_string())
            .unwrap_or_default(),
        targets_default_rule: e.targets_default_rule,
        unit_context_types: e.unit_context_types.clone(),
        guardrail_metric_ids: e
            .guardrail_metric_ids
            .iter()
            .map(|id| id.to_string())
            .collect(),
        pre_period_days: e.pre_period_days,
        metric_ids: e.metric_ids.iter().map(|id| id.to_string()).collect(),
        exclusion_group_id: e.exclusion_group_id.map(|g| g.to_string()),
        group_bucket_lo: e.group_bucket_lo.map(u32::from),
        group_bucket_hi: e.group_bucket_hi.map(u32::from),
    }
}

/// Map a core [`ExclusionGroup`] to the proto message.
fn group_to_proto(g: &ExclusionGroup) -> stitchd_proto::experiments::v1::ExclusionGroup {
    stitchd_proto::experiments::v1::ExclusionGroup {
        id: g.id.to_string(),
        env_id: g.environment_id.to_string(),
        name: g.name.clone(),
        description: g.description.clone().unwrap_or_default(),
        allocated_bp: g.allocated_bp,
        free_bp: g.free_bp,
        version: g.version,
        unit_context_type: g.unit_context_type.clone(),
    }
}

/// Convert a traffic-allocation percentage (e.g. `25.0`) to basis points
/// (`2500`), clamped to `[0, 10000]`.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn pct_to_bp(pct: f64) -> u32 {
    let bp = (pct * 100.0).round();
    if bp <= 0.0 {
        0
    } else if bp >= 10_000.0 {
        10_000
    } else {
        bp as u32
    }
}

/// Sentinel prefix used to encode the locking experiment ID into a
/// `failed_precondition` status so the gateway rebuilds the structured 409
/// (`flag_locked_by_experiment`) body. Must match
/// `stitchd_flag_service::error::FLAG_LOCKED_STATUS_PREFIX`.
const FLAG_LOCKED_STATUS_PREFIX: &str = "flag_locked_by_experiment:";

/// Reject group-membership mutations while the experiment's flag is locked by a
/// running/paused experiment. The experiment itself being running/paused means
/// its flag is locked (whole-flag freeze), so we key off the experiment's own
/// status. Returns the flag-lock sentinel `Status` (→ HTTP 409) when locked.
#[allow(clippy::result_large_err)]
fn reject_if_flag_locked(experiment: &Experiment) -> Result<(), Status> {
    if matches!(
        experiment.status,
        ExperimentStatus::Running | ExperimentStatus::Paused
    ) {
        return Err(Status::failed_precondition(format!(
            "{FLAG_LOCKED_STATUS_PREFIX}{}",
            experiment.id
        )));
    }
    Ok(())
}

#[allow(clippy::result_large_err)]
fn parse_env_id(s: &str) -> Result<EnvironmentId, Status> {
    uuid::Uuid::parse_str(s)
        .map(EnvironmentId::from_uuid)
        .map_err(|_| Status::invalid_argument("invalid env_id UUID"))
}

#[allow(clippy::result_large_err)]
fn parse_group_id(s: &str) -> Result<ExclusionGroupId, Status> {
    uuid::Uuid::parse_str(s)
        .map(ExclusionGroupId::from_uuid)
        .map_err(|_| Status::invalid_argument("invalid group_id UUID"))
}

#[allow(clippy::result_large_err)]
fn parse_experiment_id(s: &str) -> Result<ExperimentId, Status> {
    uuid::Uuid::parse_str(s)
        .map(ExperimentId::from_uuid)
        .map_err(|_| Status::invalid_argument("invalid experiment_id UUID"))
}

// ---------------------------------------------------------------------------
// Rule-gate writer port
// ---------------------------------------------------------------------------

/// Port for writing/clearing a rule's exclusion gate on its stored `rule_def`.
///
/// Defined here (rather than in the DB trait layer) so the experimentation
/// service can depend on the narrow capability it needs and tests can mock it.
/// Implemented for `stitchd_db::repository::pg::PgFlagRepository` below.
#[async_trait]
pub trait RuleGateWriter: Send + Sync {
    /// Set (`Some`) or clear (`None`) the exclusion gate on `flag_rule_id`.
    async fn set_rule_exclusion_gate(
        &self,
        flag_rule_id: RuleId,
        gate: Option<ExclusionGate>,
    ) -> Result<(), RepositoryError>;
}

#[async_trait]
impl RuleGateWriter for stitchd_db::repository::pg::PgFlagRepository {
    async fn set_rule_exclusion_gate(
        &self,
        flag_rule_id: RuleId,
        gate: Option<ExclusionGate>,
    ) -> Result<(), RepositoryError> {
        // Delegates to the inherent method added on the PG flag repo (P3.T1).
        Self::set_rule_exclusion_gate(self, flag_rule_id, gate).await
    }
}

// ---------------------------------------------------------------------------
// Service struct
// ---------------------------------------------------------------------------

/// The gRPC `ExperimentationService` implementation.
/// Staleness threshold: results are stale if last_computed_at is older than this.
const STALE_AFTER_SECS: i64 = 3600; // 60 minutes

pub struct ExperimentationServiceImpl {
    experiment_repo: Arc<dyn ExperimentRepository>,
    analytics_client: Arc<dyn AnalyticsResultsPort>,
    schedule_repo: Arc<dyn StatsScheduleRepository>,
    /// Optional Flag Service client. When `None`, flag verification is skipped.
    flag_client: Option<FlagClient>,
    /// Optional CH dictionary refresher. When `Some`, every successful
    /// transition fires `SYSTEM RELOAD DICTIONARY experiment_iterations_active`
    /// so the attribution MV picks up the iteration change immediately.
    /// `None` skips the refresh (the dictionary's LIFETIME caps staleness at
    /// 60s in that case).
    dictionary_refresher: Option<Arc<dyn DictionaryRefresher>>,
    /// Optional ClickHouse-backed reader for paginated `experiment_assignments`
    /// reads (`ListExposures` RPC). `None` makes the RPC return `Unimplemented`.
    exposure_reader: Option<Arc<dyn crate::exposure_reader::ExposureReader>>,
    /// Optional ClickHouse-backed reader for `experiment_interactions`
    /// (`GetExperimentInteractions` RPC). `None` makes the RPC return
    /// `Unimplemented`.
    interactions_reader: Option<Arc<dyn crate::interactions_reader::InteractionsReader>>,
    /// Optional exclusion-group repository (PG). `None` makes the exclusion-group
    /// RPCs return `Unimplemented`.
    exclusion_group_repo: Option<Arc<dyn ExclusionGroupRepository>>,
    /// Optional writer for rule exclusion gates (PG flag repo). Required for
    /// Assign/Unassign to push the gate onto the rule's `rule_def` so the
    /// flag snapshot picks it up. `None` makes those RPCs return `Unimplemented`.
    rule_gate_writer: Option<Arc<dyn RuleGateWriter>>,
}

impl ExperimentationServiceImpl {
    /// Construct a new service instance.
    #[must_use]
    pub fn new(
        experiment_repo: Arc<dyn ExperimentRepository>,
        analytics_client: Arc<dyn AnalyticsResultsPort>,
        schedule_repo: Arc<dyn StatsScheduleRepository>,
        flag_client: Option<FlagClient>,
    ) -> Self {
        Self {
            experiment_repo,
            analytics_client,
            schedule_repo,
            flag_client,
            dictionary_refresher: None,
            exposure_reader: None,
            interactions_reader: None,
            exclusion_group_repo: None,
            rule_gate_writer: None,
        }
    }

    /// Attach a ClickHouse-backed reader for `experiment_interactions`. Without
    /// it, `GetExperimentInteractions` returns `Status::unimplemented`.
    #[must_use]
    pub fn with_interactions_reader(
        mut self,
        reader: Arc<dyn crate::interactions_reader::InteractionsReader>,
    ) -> Self {
        self.interactions_reader = Some(reader);
        self
    }

    /// Attach the exclusion-group repository and rule-gate writer. Required for
    /// the exclusion-group RPCs (Create/List/Update/Delete/Assign/Unassign) to
    /// function; without it those calls return `Status::unimplemented`.
    #[must_use]
    pub fn with_exclusion_groups(
        mut self,
        repo: Arc<dyn ExclusionGroupRepository>,
        gate_writer: Arc<dyn RuleGateWriter>,
    ) -> Self {
        self.exclusion_group_repo = Some(repo);
        self.rule_gate_writer = Some(gate_writer);
        self
    }

    /// Borrow the exclusion-group repo or fail with `Unimplemented` when the
    /// service was constructed without one.
    #[allow(clippy::result_large_err)]
    fn exclusion_group_repo(&self) -> Result<&Arc<dyn ExclusionGroupRepository>, Status> {
        self.exclusion_group_repo.as_ref().ok_or_else(|| {
            Status::unimplemented("exclusion_group_repo not configured on this service instance")
        })
    }

    /// Borrow the rule-gate writer or fail with `Unimplemented`.
    #[allow(clippy::result_large_err)]
    fn rule_gate_writer(&self) -> Result<&Arc<dyn RuleGateWriter>, Status> {
        self.rule_gate_writer.as_ref().ok_or_else(|| {
            Status::unimplemented("rule_gate_writer not configured on this service instance")
        })
    }

    /// Release an experiment's exclusion-group assignment: free its bucket
    /// range, clear the group columns, and clear the rule's exclusion gate.
    ///
    /// Shared by `unassign_experiment` (RPC) and the `stopped` lifecycle
    /// transition. Idempotent — safe to call on an already-ungrouped experiment.
    async fn release_experiment_group(
        &self,
        repo: &dyn ExclusionGroupRepository,
        gate_writer: &dyn RuleGateWriter,
        experiment: &Experiment,
    ) -> Result<(), Status> {
        repo.free_range(experiment.id).await.map_err(Status::from)?;
        if let Some(flag_rule_id) = experiment.flag_rule_id {
            gate_writer
                .set_rule_exclusion_gate(flag_rule_id, None)
                .await
                .map_err(Status::from)?;
        }
        Ok(())
    }

    /// Attach a ClickHouse dictionary refresher. Every successful
    /// `transition_experiment` invocation will fire-and-forget a reload of
    /// the `experiment_iterations_active` dictionary after the PG transition
    /// lands.
    #[must_use]
    pub fn with_dictionary_refresher(mut self, refresher: Arc<dyn DictionaryRefresher>) -> Self {
        self.dictionary_refresher = Some(refresher);
        self
    }

    /// Attach an [`crate::exposure_reader::ExposureReader`] for the
    /// `ListExposures` RPC. Without one, calls to `ListExposures` return
    /// `Status::unimplemented`.
    #[must_use]
    pub fn with_exposure_reader(
        mut self,
        reader: Arc<dyn crate::exposure_reader::ExposureReader>,
    ) -> Self {
        self.exposure_reader = Some(reader);
        self
    }
}

// ---------------------------------------------------------------------------
// gRPC trait impl
// ---------------------------------------------------------------------------

#[async_trait]
impl ExperimentationService for ExperimentationServiceImpl {
    /// Create a new experiment.
    ///
    /// Validates the experiment binding invariants (GL-08) server-side before
    /// persisting. If the request specifies `ACTIVE` status, also verifies the
    /// flag exists via the Flag Service.
    #[instrument(skip(self))]
    async fn create_experiment(
        &self,
        request: Request<CreateExperimentRequest>,
    ) -> Result<Response<stitchd_proto::experiments::v1::Experiment>, Status> {
        let req = request.into_inner();
        let proto_exp = req
            .experiment
            .ok_or_else(|| Status::invalid_argument("experiment field is required"))?;

        let target_status = proto_status_to_core(proto_exp.status)?;

        // ── Binding validation (GL-08) — validates XOR invariant, context types,
        // and flag rule kind. Returns the resolved flag_id from the flag service.
        let flag_key_opt = if proto_exp.flag_key.is_empty() {
            None
        } else {
            Some(proto_exp.flag_key.as_str())
        };
        let flag_rule_id_opt = if proto_exp.flag_rule_id.is_empty() {
            None
        } else {
            Some(proto_exp.flag_rule_id.as_str())
        };
        let binding_inputs = BindingInputs {
            environment_id: &proto_exp.environment_id,
            flag_key: flag_key_opt,
            flag_rule_id: flag_rule_id_opt,
            targets_default_rule: proto_exp.targets_default_rule,
            unit_context_types: &proto_exp.unit_context_types,
        };
        let resolved_flag_id_str = validate_experiment_binding(
            &binding_inputs,
            self.analytics_client.as_ref(),
            self.flag_client.as_ref(),
        )
        .await?;

        // Flag-lock: when activating (ACTIVE/Running), verify the flag exists.
        // (This check is retained in addition to binding validation because it
        // uses a different code path: failed_precondition vs. invalid_argument.)
        if target_status == ExperimentStatus::Running
            && !proto_exp.flag_key.is_empty()
            && let Some(fc) = &self.flag_client
        {
            fc.verify_flag_exists(&proto_exp.environment_id, &proto_exp.flag_key)
                .await
                .map_err(|status| {
                    if status.code() == tonic::Code::NotFound {
                        Status::failed_precondition(format!(
                            "flag '{}' not found in Flag Service",
                            proto_exp.flag_key
                        ))
                    } else {
                        status
                    }
                })?;
        }

        // Parse IDs.
        let env_uuid = uuid::Uuid::parse_str(&proto_exp.environment_id)
            .map_err(|_| Status::invalid_argument("invalid environment_id UUID"))?;
        let env_id = EnvironmentId::from_uuid(env_uuid);

        // Use the flag_id resolved by the binding validator when available;
        // fall back to whatever the client sent (e.g. when flag_client is None).
        // When neither provides a valid UUID (e.g. tests with no flag_client),
        // generate a new flag_id so the experiment can still be created.
        let flag_id = if !resolved_flag_id_str.is_empty() {
            uuid::Uuid::parse_str(&resolved_flag_id_str)
                .map(stitchd_core::id::FlagId::from_uuid)
                .map_err(|_| Status::invalid_argument("invalid resolved flag_id UUID"))?
        } else if !proto_exp.flag_id.is_empty() {
            uuid::Uuid::parse_str(&proto_exp.flag_id)
                .map(stitchd_core::id::FlagId::from_uuid)
                .map_err(|_| Status::invalid_argument("invalid flag_id UUID"))?
        } else {
            // No flag_client present — generate a placeholder flag_id.
            stitchd_core::id::FlagId::new()
        };

        let flag_rule_id = if proto_exp.flag_rule_id.is_empty() {
            None
        } else {
            Some(
                uuid::Uuid::parse_str(&proto_exp.flag_rule_id)
                    .map(stitchd_core::id::RuleId::from_uuid)
                    .map_err(|_| Status::invalid_argument("invalid flag_rule_id UUID"))?,
            )
        };

        let guardrail_metric_ids = proto_exp
            .guardrail_metric_ids
            .iter()
            .map(|s| {
                uuid::Uuid::parse_str(s)
                    .map(stitchd_core::id::MetricId::from_uuid)
                    .map_err(|_| Status::invalid_argument("invalid guardrail_metric_id UUID"))
            })
            .collect::<Result<Vec<_>, _>>()?;

        let now = Utc::now();
        let experiment = Experiment {
            id: ExperimentId::new(),
            environment_id: env_id,
            flag_id,
            flag_key: None,
            variant_keys: vec![],
            flag_rule_id,
            targets_default_rule: proto_exp.targets_default_rule,
            name: proto_exp.name.clone(),
            description: if proto_exp.description.is_empty() {
                None
            } else {
                Some(proto_exp.description.clone())
            },
            hypothesis: None,
            metric_ids: vec![],
            guardrail_metric_ids,
            traffic_allocation: 100.0,
            min_sample_size: None,
            pre_period_days: proto_exp.pre_period_days,
            unit_context_types: if proto_exp.unit_context_types.is_empty() {
                vec!["user".to_string()]
            } else {
                proto_exp.unit_context_types.clone()
            },
            scheduled_start_at: None,
            scheduled_end_at: None,
            status: ExperimentStatus::Draft,
            created_at: now,
            updated_at: now,
            deleted_at: None,
            version: 1,
            // Group assignment happens via AssignExperimentToGroup (Phase 2/3),
            // not at create time; ungrouped on creation.
            exclusion_group_id: None,
            group_bucket_lo: None,
            group_bucket_hi: None,
        };

        self.experiment_repo
            .create(&experiment)
            .await
            .map_err(Status::from)?;

        metrics::counter!("experimentation_service.create_experiment.ok").increment(1);

        let mut proto = core_to_proto(&experiment);
        proto.flag_key = proto_exp.flag_key;
        Ok(Response::new(proto))
    }

    /// Fetch a single experiment by ID.
    #[instrument(skip(self))]
    async fn get_experiment(
        &self,
        request: Request<GetExperimentRequest>,
    ) -> Result<Response<stitchd_proto::experiments::v1::Experiment>, Status> {
        let req = request.into_inner();
        let exp_uuid = uuid::Uuid::parse_str(&req.experiment_id)
            .map_err(|_| Status::invalid_argument("invalid experiment_id UUID"))?;
        let exp_id = ExperimentId::from_uuid(exp_uuid);

        let experiment = self
            .experiment_repo
            .find_by_id(exp_id)
            .await
            .map_err(Status::from)?;

        metrics::counter!("experimentation_service.get_experiment.ok").increment(1);
        Ok(Response::new(core_to_proto(&experiment)))
    }

    /// List all experiments for an environment.
    #[instrument(skip(self))]
    async fn list_experiments(
        &self,
        request: Request<ListExperimentsRequest>,
    ) -> Result<Response<ListExperimentsResponse>, Status> {
        let req = request.into_inner();
        let env_uuid = uuid::Uuid::parse_str(&req.environment_id)
            .map_err(|_| Status::invalid_argument("invalid environment_id UUID"))?;
        let env_id = EnvironmentId::from_uuid(env_uuid);

        let page = if req.page == 0 { 1u64 } else { req.page as u64 };
        let per_page = if req.per_page == 0 {
            50u64
        } else {
            (req.per_page as u64).min(200)
        };
        let offset = (page - 1) * per_page;

        let (experiments, total) = self
            .experiment_repo
            .list_by_environment_paginated(env_id, offset, per_page)
            .await
            .map_err(Status::from)?;

        let protos: Vec<_> = experiments.iter().map(core_to_proto).collect();
        metrics::counter!("experimentation_service.list_experiments.ok").increment(1);
        Ok(Response::new(ListExperimentsResponse {
            experiments: protos,
            total,
        }))
    }

    /// Update an existing experiment (name, description, variant keys, and
    /// optionally binding fields).
    ///
    /// When any of the binding fields (`flag_key`, `flag_rule_id`,
    /// `targets_default_rule`, `unit_context_types`) are supplied in the
    /// request, the binding invariants are validated server-side (GL-08)
    /// before the update is persisted.
    #[instrument(skip(self))]
    async fn update_experiment(
        &self,
        request: Request<UpdateExperimentRequest>,
    ) -> Result<Response<stitchd_proto::experiments::v1::Experiment>, Status> {
        let req = request.into_inner();
        let proto_exp = req
            .experiment
            .ok_or_else(|| Status::invalid_argument("experiment field is required"))?;

        let exp_uuid = uuid::Uuid::parse_str(&proto_exp.id)
            .map_err(|_| Status::invalid_argument("invalid experiment id UUID"))?;
        let exp_id = stitchd_core::id::ExperimentId::from_uuid(exp_uuid);

        let mut experiment = self
            .experiment_repo
            .find_by_id(exp_id)
            .await
            .map_err(Status::from)?;

        if !proto_exp.name.is_empty() {
            experiment.name = proto_exp.name.clone();
        }
        if !proto_exp.description.is_empty() {
            experiment.description = Some(proto_exp.description.clone());
        }
        if proto_exp.version > 0 {
            experiment.version = i64::try_from(proto_exp.version).unwrap_or(experiment.version);
        }

        // ── Binding fields update + validation (GL-08) ───────────────────────
        // Run the binding validator only when the caller supplied any of the
        // binding fields — matching the gateway's `touches_binding` heuristic.
        let touches_binding = !proto_exp.flag_rule_id.is_empty()
            || proto_exp.targets_default_rule
            || !proto_exp.unit_context_types.is_empty()
            || !proto_exp.flag_key.is_empty();

        if touches_binding {
            let flag_key_opt = if proto_exp.flag_key.is_empty() {
                None
            } else {
                Some(proto_exp.flag_key.as_str())
            };
            let flag_rule_id_opt = if proto_exp.flag_rule_id.is_empty() {
                None
            } else {
                Some(proto_exp.flag_rule_id.as_str())
            };
            let binding_inputs = BindingInputs {
                environment_id: &proto_exp.environment_id,
                flag_key: flag_key_opt,
                flag_rule_id: flag_rule_id_opt,
                targets_default_rule: proto_exp.targets_default_rule,
                unit_context_types: &proto_exp.unit_context_types,
            };
            validate_experiment_binding(
                &binding_inputs,
                self.analytics_client.as_ref(),
                self.flag_client.as_ref(),
            )
            .await?;

            // Apply binding field updates to the experiment row.
            if !proto_exp.flag_key.is_empty() {
                experiment.flag_key = Some(proto_exp.flag_key.clone());
            }
            if !proto_exp.flag_rule_id.is_empty() {
                let rule_uuid = uuid::Uuid::parse_str(&proto_exp.flag_rule_id)
                    .map_err(|_| Status::invalid_argument("invalid flag_rule_id UUID"))?;
                experiment.flag_rule_id = Some(stitchd_core::id::RuleId::from_uuid(rule_uuid));
            }
            experiment.targets_default_rule = proto_exp.targets_default_rule;
            if !proto_exp.unit_context_types.is_empty() {
                experiment.unit_context_types = proto_exp.unit_context_types.clone();
            }
        }
        // ─────────────────────────────────────────────────────────────────────

        experiment.updated_at = chrono::Utc::now();

        let updated = self
            .experiment_repo
            .update(&experiment)
            .await
            .map_err(Status::from)?;

        metrics::counter!("experimentation_service.update_experiment.ok").increment(1);

        let mut proto = core_to_proto(&updated);
        proto.flag_key = proto_exp.flag_key;
        Ok(Response::new(proto))
    }

    /// Soft-delete an experiment.
    #[instrument(skip(self))]
    async fn delete_experiment(
        &self,
        request: Request<DeleteExperimentRequest>,
    ) -> Result<Response<stitchd_proto::experiments::v1::Experiment>, Status> {
        let req = request.into_inner();
        let exp_uuid = uuid::Uuid::parse_str(&req.experiment_id)
            .map_err(|_| Status::invalid_argument("invalid experiment_id UUID"))?;
        let exp_id = stitchd_core::id::ExperimentId::from_uuid(exp_uuid);

        let experiment = self
            .experiment_repo
            .find_by_id(exp_id)
            .await
            .map_err(Status::from)?;

        self.experiment_repo
            .soft_delete(exp_id)
            .await
            .map_err(Status::from)?;

        metrics::counter!("experimentation_service.delete_experiment.ok").increment(1);
        Ok(Response::new(core_to_proto(&experiment)))
    }

    /// Apply a lifecycle status transition to an experiment.
    #[instrument(skip(self))]
    async fn transition_experiment(
        &self,
        request: Request<TransitionExperimentRequest>,
    ) -> Result<Response<stitchd_proto::experiments::v1::Experiment>, Status> {
        let req = request.into_inner();
        let exp_uuid = uuid::Uuid::parse_str(&req.experiment_id)
            .map_err(|_| Status::invalid_argument("invalid experiment_id UUID"))?;
        let exp_id = stitchd_core::id::ExperimentId::from_uuid(exp_uuid);
        let target_status = proto_status_to_core(req.new_status)?;

        let updated = self
            .experiment_repo
            .apply_transition(exp_id, target_status, None)
            .await
            .map_err(Status::from)?;

        // Lifecycle (P3.T3): on transition to `stopped`, release any
        // exclusion-group assignment — free the bucket range and clear the
        // rule's exclusion gate — so the bucket space becomes reusable. Only
        // runs when the service is wired with exclusion-group support; the
        // `free_range` call is a no-op when the experiment holds no range.
        if target_status == ExperimentStatus::Stopped
            && let (Some(repo), Some(gate_writer)) =
                (self.exclusion_group_repo.as_ref(), self.rule_gate_writer.as_ref())
        {
            self.release_experiment_group(repo.as_ref(), gate_writer.as_ref(), &updated)
                .await?;
        }

        // Note: the flag-service holds an in-process `FlagLockCache` keyed on
        // `flag_id` that derives lockedness from PG. Because flag-service and
        // experimentation-service run as separate binaries in production we
        // cannot invalidate that cache directly from here; instead the cache's
        // 30s TTL caps the staleness window. A future proto RPC
        // (`InvalidateFlagLockCache`) would let us push invalidations across
        // the wire — out of scope for the Phase 3 lock-enforcement work.
        // See `crates/stitchd-flag-service/src/flag_lock.rs` for the cache.

        // Phase 4 attribution pipeline: ping CH to reload the
        // `experiment_iterations_active` dictionary. Fire-and-forget — the
        // dictionary's LIFETIME(MIN 30 MAX 60) refresh is the fallback if
        // this call fails. Logging happens inside the spawned task.
        if let Some(refresher) = self.dictionary_refresher.as_ref() {
            spawn_refresh(refresher.clone());
        }

        metrics::counter!("experimentation_service.transition_experiment.ok").increment(1);
        Ok(Response::new(core_to_proto(&updated)))
    }

    /// List iterations for an experiment, optionally paginated.
    ///
    /// `limit == 0` returns the full list (used by stats-service which doesn't
    /// paginate); any non-zero limit clamps to `[offset, offset + limit)`.
    /// `total` is always populated with the unpaginated count so callers can
    /// drive UI pagination.
    #[instrument(skip(self))]
    async fn list_iterations(
        &self,
        request: Request<ListIterationsRequest>,
    ) -> Result<Response<ListIterationsResponse>, Status> {
        let req = request.into_inner();
        let exp_uuid = uuid::Uuid::parse_str(&req.experiment_id)
            .map_err(|_| Status::invalid_argument("invalid experiment_id UUID"))?;
        let exp_id = stitchd_core::id::ExperimentId::from_uuid(exp_uuid);

        let iterations = self
            .experiment_repo
            .list_iterations(exp_id)
            .await
            .map_err(Status::from)?;

        let total = iterations.len() as u64;
        let window: Vec<_> = if req.limit == 0 {
            iterations.iter().map(iteration_to_proto).collect()
        } else {
            iterations
                .iter()
                .skip(usize::try_from(req.offset).unwrap_or(usize::MAX))
                .take(usize::try_from(req.limit).unwrap_or(usize::MAX))
                .map(iteration_to_proto)
                .collect()
        };

        metrics::counter!("experimentation_service.list_iterations.ok").increment(1);
        Ok(Response::new(ListIterationsResponse {
            iterations: window,
            total,
        }))
    }

    /// Fetch pre-computed statistical results for an experiment.
    ///
    /// Delegates to analytics-service via `ListExperimentResults` gRPC
    /// (ClickHouse-backed). The PG `experiment_results` table is no longer
    /// consulted here.
    #[instrument(skip(self))]
    async fn get_results(
        &self,
        request: Request<GetResultsRequest>,
    ) -> Result<Response<ExperimentResults>, Status> {
        let req = request.into_inner();
        let exp_id_uuid = req
            .experiment_id
            .parse::<uuid::Uuid>()
            .map_err(|_| Status::invalid_argument("invalid experiment_id UUID"))?;

        // Call analytics-service to get results from ClickHouse.
        let results = self
            .analytics_client
            .list_experiment_results(
                &req.environment_id,
                &req.experiment_id,
                None, // latest iteration
            )
            .await
            .map_err(|e| Status::internal(format!("analytics-service error: {e}")))?;

        // Aggregate streamed ExperimentResult protos into VariantResult messages
        // bucketed BY CONTEXT TYPE (Phase 7). `variant_stats` is a JSON string
        // either `{"<variant_key>": <count_or_obj>, ...}` (back-compat) or
        // `{"<variant_key>": { "count": N, "lift": L, ... }, ...}` (Phase 6).
        //
        // The legacy `variant_results` field is also populated (flat, across
        // context types) for back-compat with callers that haven't switched to
        // `results_by_context_type` yet.
        //
        // Guardrail rows are identified via the metric_id membership in the
        // experiment's `guardrail_metric_ids`. They are written by stats-service
        // with the same shape as primaries; we split them into the
        // `guardrails` bucket here.
        use std::collections::HashMap;
        // Map<(context_type, variant_key), VariantResult> for primaries.
        let mut by_ctx_variant: HashMap<(String, String), VariantResult> = HashMap::new();
        // Map<(context_type, variant_key), VariantResult> for guardrails.
        let mut guardrails_by_ctx_variant: HashMap<(String, String), VariantResult> =
            HashMap::new();
        // SRM JSON snapshots per context_type (any metric row may carry it; we
        // overwrite — they should agree across rows for a given context type).
        let mut srm_by_ctx: HashMap<String, serde_json::Value> = HashMap::new();
        // Track which metric_ids are guardrails so we route rows accordingly.
        // Fetched once below from the experiment row.
        let mut latest_computed_at_ms: i64 = 0;

        // Fetch experiment to determine guardrail metric ids + bound_target + pre_period.
        let experiment_record = self
            .experiment_repo
            .find_by_id(stitchd_core::id::ExperimentId::from_uuid(exp_id_uuid))
            .await
            .ok();

        let guardrail_metric_ids: std::collections::HashSet<String> = experiment_record
            .as_ref()
            .map(|e| {
                e.guardrail_metric_ids
                    .iter()
                    .map(ToString::to_string)
                    .collect()
            })
            .unwrap_or_default();

        for result in &results {
            // Parse computed_at RFC 3339 → milliseconds.
            let computed_ms = result
                .computed_at
                .parse::<chrono::DateTime<chrono::Utc>>()
                .map(|dt| dt.timestamp_millis())
                .unwrap_or(0);
            if computed_ms > latest_computed_at_ms {
                latest_computed_at_ms = computed_ms;
            }

            let context_type = if result.context_type.is_empty() {
                "user".to_string()
            } else {
                result.context_type.clone()
            };

            let is_guardrail = guardrail_metric_ids.contains(&result.metric_key);
            let target = if is_guardrail {
                &mut guardrails_by_ctx_variant
            } else {
                &mut by_ctx_variant
            };

            // variant_stats is a JSON object string.
            let variant_stats: serde_json::Value =
                serde_json::from_str(&result.variant_stats).unwrap_or(serde_json::Value::Null);

            // SRM payload — stats-service may attach it under top-level "srm"
            // of the variant_stats JSON (Phase 6) or in a `srm_result` key.
            if let Some(srm_val) = variant_stats.get("srm") {
                srm_by_ctx
                    .entry(context_type.clone())
                    .or_insert_with(|| srm_val.clone());
            }

            if let Some(obj) = variant_stats.as_object() {
                for (variant_key, val) in obj {
                    if variant_key == "srm" {
                        continue;
                    }
                    let (participant_count, lift) = match val {
                        serde_json::Value::Number(n) => (n.as_u64().unwrap_or(0), 0.0_f64),
                        serde_json::Value::Object(map) => {
                            let count = map
                                .get("count")
                                .or_else(|| map.get("participant_count"))
                                .and_then(serde_json::Value::as_u64)
                                .unwrap_or(0);
                            let lift = map
                                .get("lift")
                                .and_then(serde_json::Value::as_f64)
                                .unwrap_or(0.0);
                            (count, lift)
                        }
                        _ => (0, 0.0),
                    };

                    let key = (context_type.clone(), variant_key.clone());
                    let entry = target.entry(key).or_insert_with(|| VariantResult {
                        variant_key: variant_key.clone(),
                        participant_count,
                        metric_values: HashMap::new(),
                        p_value: 0.0,
                        p_value_present: false,
                        p_value_corrected: None,
                        context_type: context_type.clone(),
                        direction_violation: false,
                        lift,
                    });

                    // Pull p_value (+ corrected) from frequentist_result JSON.
                    if let Some(freq_str) = &result.frequentist_result
                        && let Ok(freq_json) = serde_json::from_str::<serde_json::Value>(freq_str)
                    {
                        if let Some(p_val) = freq_json.get("p_value").and_then(|v| v.as_f64()) {
                            entry.p_value = p_val;
                            entry.p_value_present = true;
                        }
                        if let Some(p_corr) =
                            freq_json.get("p_value_corrected").and_then(|v| v.as_f64())
                        {
                            entry.p_value_corrected = Some(p_corr);
                        }
                    }

                    entry
                        .metric_values
                        .insert(result.metric_key.clone(), participant_count as f64);
                }
            }
        }

        // Flat back-compat list (across context types + primaries only).
        let variant_results: Vec<VariantResult> = by_ctx_variant.values().cloned().collect();

        // Build per-context-type buckets.
        let mut ctx_groups: HashMap<String, (Vec<VariantResult>, Vec<VariantResult>)> =
            HashMap::new();
        for ((ct, _), vr) in by_ctx_variant {
            ctx_groups.entry(ct).or_default().0.push(vr);
        }
        for ((ct, _), vr) in guardrails_by_ctx_variant {
            ctx_groups.entry(ct).or_default().1.push(vr);
        }

        let mut results_by_context_type: Vec<ContextTypeResults> = ctx_groups
            .into_iter()
            .map(|(context_type, (variants, guardrails))| {
                let srm = srm_by_ctx.get(&context_type).and_then(srm_json_to_proto);
                ContextTypeResults {
                    context_type,
                    variants,
                    srm,
                    guardrails,
                }
            })
            .collect();
        // Deterministic ordering for stable test snapshots.
        results_by_context_type.sort_by(|a, b| a.context_type.cmp(&b.context_type));

        // Bound-target + pre_period_days from the experiment row.
        let (bound_target, pre_period_days) = match &experiment_record {
            Some(e) => {
                let kind = if e.targets_default_rule {
                    "default_rule"
                } else {
                    "rule"
                };
                let rule_id = e
                    .flag_rule_id
                    .as_ref()
                    .map(ToString::to_string)
                    .unwrap_or_default();
                let label = if e.targets_default_rule {
                    "Default rule (fallthrough)".to_string()
                } else {
                    // Rule names live on the flag service; the gateway enriches
                    // when needed. Default to the rule_id string here.
                    rule_id.clone()
                };
                (
                    Some(BoundTarget {
                        kind: kind.to_string(),
                        rule_id,
                        label,
                    }),
                    e.pre_period_days,
                )
            }
            None => (None, 0u32),
        };

        // Fetch schedule for staleness metadata.
        let schedule = self
            .schedule_repo
            .get_schedule_for_experiment(exp_id_uuid)
            .await
            .map_err(|e| Status::internal(format!("schedule database error: {e}")))?;

        let (is_stale, next_run_at_ms, computation_status) = match &schedule {
            None => (true, 0i64, String::new()),
            Some(s) => {
                let stale = s.last_computed_at.is_none_or(|t| {
                    Utc::now().signed_duration_since(t).num_seconds() > STALE_AFTER_SECS
                });
                let next_run = s.next_run_at.map_or(0, |t| t.timestamp_millis());
                let status = match s.computation_status {
                    stitchd_db::ComputationStatus::Ready => "ready",
                    stitchd_db::ComputationStatus::Computing => "computing",
                    stitchd_db::ComputationStatus::NeverComputed => "never_computed",
                }
                .to_string();
                (stale, next_run, status)
            }
        };

        metrics::counter!("experimentation_service.get_results.ok").increment(1);
        Ok(Response::new(ExperimentResults {
            experiment_id: req.experiment_id,
            variant_results,
            computed_at_ms: latest_computed_at_ms,
            is_stale,
            next_run_at_ms,
            computation_status,
            results_by_context_type,
            bound_target,
            pre_period_days,
        }))
    }

    // ── Stats-service–facing RPCs ────────────────────────────────────────────

    /// Server-streaming RPC: emit all currently running experiments.
    ///
    /// Loads the full list from PG and streams each as a [`RunningExperiment`]
    /// message. The stream is short-lived (no long-poll); stats-service should
    /// call this on a cadence of its choosing.
    type ListRunningExperimentsStream = std::pin::Pin<
        Box<dyn futures_core::Stream<Item = Result<RunningExperiment, Status>> + Send + 'static>,
    >;

    #[instrument(skip(self))]
    async fn list_running_experiments(
        &self,
        _request: Request<ListRunningExperimentsRequest>,
    ) -> Result<Response<Self::ListRunningExperimentsStream>, Status> {
        let experiments = self
            .experiment_repo
            .list_all_running()
            .await
            .map_err(Status::from)?;

        // For each running experiment, fetch the active (un-ended) iteration.
        let mut items: Vec<Result<RunningExperiment, Status>> =
            Vec::with_capacity(experiments.len());
        for exp in experiments {
            let iterations = self
                .experiment_repo
                .list_iterations(exp.id)
                .await
                .map_err(Status::from)?;

            // Active iteration = most recent one with no ended_at.
            let active = iterations.into_iter().rfind(|i| i.ended_at.is_none());

            if let Some(iter) = active {
                items.push(Ok(RunningExperiment {
                    experiment_id: exp.id.to_string(),
                    environment_id: exp.environment_id.to_string(),
                    iteration_id: iter.id.to_string(),
                    variant_keys: vec![], // variants live on the flag; not denormalised here
                    metric_ids: iter.metric_ids.iter().map(ToString::to_string).collect(),
                    started_at_ms: iter.started_at.timestamp_millis(),
                    status: "running".to_string(),
                }));
            }
        }

        metrics::counter!("experimentation_service.list_running_experiments.ok").increment(1);
        let stream = tokio_stream::iter(items);
        Ok(Response::new(Box::pin(stream)))
    }

    /// Fetch a single iteration row by ID (for stats-service to read metric
    /// configuration without touching PG directly).
    #[instrument(skip(self))]
    async fn get_experiment_iteration(
        &self,
        request: Request<GetExperimentIterationRequest>,
    ) -> Result<Response<ProtoIteration>, Status> {
        let req = request.into_inner();
        let iter_uuid = uuid::Uuid::parse_str(&req.iteration_id)
            .map_err(|_| Status::invalid_argument("invalid iteration_id UUID"))?;
        let iter_id = ExperimentIterationId::from_uuid(iter_uuid);

        let iter = self
            .experiment_repo
            .find_iteration_by_id(iter_id)
            .await
            .map_err(Status::from)?;

        metrics::counter!("experimentation_service.get_experiment_iteration.ok").increment(1);
        Ok(Response::new(iteration_to_proto(&iter)))
    }

    /// Update the `last_computed_at` timestamp for a given iteration's
    /// stats_schedule row.
    #[instrument(skip(self))]
    async fn update_iteration_last_computed(
        &self,
        request: Request<UpdateIterationLastComputedRequest>,
    ) -> Result<Response<UpdateIterationLastComputedResponse>, Status> {
        let req = request.into_inner();
        let iter_uuid = uuid::Uuid::parse_str(&req.iteration_id)
            .map_err(|_| Status::invalid_argument("invalid iteration_id UUID"))?;

        self.schedule_repo
            .update_last_computed(iter_uuid, req.last_computed_at_ms)
            .await
            .map_err(|e| Status::internal(format!("schedule database error: {e}")))?;

        metrics::counter!("experimentation_service.update_iteration_last_computed.ok").increment(1);
        Ok(Response::new(UpdateIterationLastComputedResponse {}))
    }

    // ── Phase 7 — admin reads ────────────────────────────────────────────────

    /// Paginated read against the ClickHouse `experiment_assignments` table.
    ///
    /// Returns one page of (context_type, context_key, variant, assigned_at,
    /// matched_rule_id) rows for the given `(experiment_id, context_type)`.
    /// Returns `INVALID_ARGUMENT` for malformed UUIDs or empty `context_type`,
    /// `UNIMPLEMENTED` when no `exposure_reader` is attached.
    #[instrument(skip(self))]
    async fn list_exposures(
        &self,
        request: Request<stitchd_proto::experiments::v1::ListExposuresRequest>,
    ) -> Result<Response<stitchd_proto::experiments::v1::ListExposuresResponse>, Status> {
        let req = request.into_inner();
        let exp_uuid = uuid::Uuid::parse_str(&req.experiment_id)
            .map_err(|_| Status::invalid_argument("invalid experiment_id UUID"))?;
        if req.context_type.is_empty() {
            return Err(Status::invalid_argument(
                "context_type is required and must be non-empty",
            ));
        }

        let reader = self.exposure_reader.as_ref().ok_or_else(|| {
            Status::unimplemented("exposure_reader not configured on this service instance")
        })?;

        let (rows, total) = reader
            .list_exposures(exp_uuid, &req.context_type, req.offset, req.limit)
            .await
            .map_err(|e| Status::internal(format!("clickhouse error: {e}")))?;

        let exposures = rows
            .into_iter()
            .map(|r| stitchd_proto::experiments::v1::ExposureRow {
                context_type: r.context_type,
                context_key: r.context_key,
                variant_key: r.variant_key,
                assigned_at: r.assigned_at.to_rfc3339(),
                matched_rule_id: r.matched_rule_id.map(|u| u.to_string()).unwrap_or_default(),
            })
            .collect();

        metrics::counter!("experimentation_service.list_exposures.ok").increment(1);
        Ok(Response::new(
            stitchd_proto::experiments::v1::ListExposuresResponse { exposures, total },
        ))
    }

    // ── Exclusion groups + interactions ─────────────────────────────────────
    //
    // Proto surface landed via the parallel P1.T5 merge; the handlers are wired
    // in Phase 2/3 (reads + persistence). Until then these return
    // `Unimplemented` so the trait is satisfied and the workspace stays green.

    #[instrument(skip(self))]
    async fn create_exclusion_group(
        &self,
        request: Request<stitchd_proto::experiments::v1::CreateExclusionGroupRequest>,
    ) -> Result<Response<stitchd_proto::experiments::v1::ExclusionGroup>, Status> {
        let repo = self.exclusion_group_repo()?;
        let proto = request
            .into_inner()
            .group
            .ok_or_else(|| Status::invalid_argument("group field is required"))?;

        let env_id = parse_env_id(&proto.env_id)?;
        if proto.name.trim().is_empty() {
            return Err(Status::invalid_argument("group name must not be empty"));
        }
        let description = if proto.description.is_empty() {
            None
        } else {
            Some(proto.description.as_str())
        };

        // The group's diversion unit; defaults to "user" when unset. Validate it
        // against the env's registered context types (mirrors experiment
        // creation) so the group can only randomize on a real unit.
        let unit_context_type = if proto.unit_context_type.trim().is_empty() {
            "user".to_string()
        } else {
            proto.unit_context_type.clone()
        };
        let registered = self
            .analytics_client
            .list_context_types(&proto.env_id)
            .await?;
        if !registered.iter().any(|ct| ct == &unit_context_type) {
            return Err(Status::invalid_argument(format!(
                "context type '{unit_context_type}' is not registered for this environment"
            )));
        }

        let group = repo
            .create(env_id, &proto.name, description, &unit_context_type)
            .await
            .map_err(Status::from)?;

        metrics::counter!("experimentation_service.create_exclusion_group.ok").increment(1);
        Ok(Response::new(group_to_proto(&group)))
    }

    #[instrument(skip(self))]
    async fn get_exclusion_group(
        &self,
        request: Request<stitchd_proto::experiments::v1::GetExclusionGroupRequest>,
    ) -> Result<Response<stitchd_proto::experiments::v1::ExclusionGroup>, Status> {
        let repo = self.exclusion_group_repo()?;
        let req = request.into_inner();
        let group_id = parse_group_id(&req.group_id)?;

        let group = repo.find_by_id(group_id).await.map_err(Status::from)?;

        metrics::counter!("experimentation_service.get_exclusion_group.ok").increment(1);
        Ok(Response::new(group_to_proto(&group)))
    }

    #[instrument(skip(self))]
    async fn list_exclusion_groups(
        &self,
        request: Request<stitchd_proto::experiments::v1::ListExclusionGroupsRequest>,
    ) -> Result<Response<stitchd_proto::experiments::v1::ListExclusionGroupsResponse>, Status> {
        let repo = self.exclusion_group_repo()?;
        let req = request.into_inner();
        let env_id = parse_env_id(&req.env_id)?;

        let groups = repo
            .list_by_environment(env_id)
            .await
            .map_err(Status::from)?;
        let total = groups.len() as u64;
        let proto_groups = groups.iter().map(group_to_proto).collect();

        metrics::counter!("experimentation_service.list_exclusion_groups.ok").increment(1);
        Ok(Response::new(
            stitchd_proto::experiments::v1::ListExclusionGroupsResponse {
                groups: proto_groups,
                total,
            },
        ))
    }

    #[instrument(skip(self))]
    async fn update_exclusion_group(
        &self,
        request: Request<stitchd_proto::experiments::v1::UpdateExclusionGroupRequest>,
    ) -> Result<Response<stitchd_proto::experiments::v1::ExclusionGroup>, Status> {
        let repo = self.exclusion_group_repo()?;
        let proto = request
            .into_inner()
            .group
            .ok_or_else(|| Status::invalid_argument("group field is required"))?;

        let group_id = parse_group_id(&proto.id)?;
        if proto.name.trim().is_empty() {
            return Err(Status::invalid_argument("group name must not be empty"));
        }
        let description = if proto.description.is_empty() {
            None
        } else {
            Some(proto.description.as_str())
        };
        let expected_version = proto.version;

        let group = repo
            .update(group_id, &proto.name, description, expected_version)
            .await
            .map_err(Status::from)?;

        metrics::counter!("experimentation_service.update_exclusion_group.ok").increment(1);
        Ok(Response::new(group_to_proto(&group)))
    }

    #[instrument(skip(self))]
    async fn delete_exclusion_group(
        &self,
        request: Request<stitchd_proto::experiments::v1::DeleteExclusionGroupRequest>,
    ) -> Result<Response<stitchd_proto::experiments::v1::DeleteExclusionGroupResponse>, Status> {
        let repo = self.exclusion_group_repo()?;
        let req = request.into_inner();
        let group_id = parse_group_id(&req.group_id)?;

        repo.soft_delete(group_id).await.map_err(Status::from)?;

        metrics::counter!("experimentation_service.delete_exclusion_group.ok").increment(1);
        Ok(Response::new(
            stitchd_proto::experiments::v1::DeleteExclusionGroupResponse {},
        ))
    }

    #[instrument(skip(self))]
    async fn assign_experiment_to_group(
        &self,
        request: Request<stitchd_proto::experiments::v1::AssignExperimentToGroupRequest>,
    ) -> Result<Response<stitchd_proto::experiments::v1::AssignExperimentToGroupResponse>, Status>
    {
        let repo = self.exclusion_group_repo()?;
        let gate_writer = self.rule_gate_writer()?;
        let req = request.into_inner();

        let group_id = parse_group_id(&req.group_id)?;
        let exp_id = parse_experiment_id(&req.experiment_id)?;

        // Load the experiment so we can size the allocation and build the gate.
        let experiment = self
            .experiment_repo
            .find_by_id(exp_id)
            .await
            .map_err(Status::from)?;

        // Membership mutation is rejected while the bound flag is locked
        // (experiment running/paused) — surfaces as the whole-flag-lock 409.
        reject_if_flag_locked(&experiment)?;

        // Size the carve-out by the experiment's traffic allocation, unless the
        // caller explicitly overrides via `requested_bp`.
        let requested_bp = if req.requested_bp > 0 {
            req.requested_bp
        } else {
            pct_to_bp(experiment.traffic_allocation)
        };

        // Load the group to obtain its salt and diversion unit for the gate.
        let group = repo.find_by_id(group_id).await.map_err(Status::from)?;

        // Mutual exclusion only holds if EVERY member randomizes on the same
        // unit — the group's `unit_context_type`. Reject any experiment that
        // does not declare that unit; otherwise its context would bucket on a
        // different key and silently break exclusion.
        if !experiment
            .unit_context_types
            .iter()
            .any(|ct| ct == &group.unit_context_type)
        {
            return Err(Status::failed_precondition(format!(
                "experiment cannot join exclusion group: the group randomizes on unit '{}', \
                 but the experiment's unit_context_types ({}) do not include it; all members \
                 must share the group's diversion unit for mutual exclusion to hold",
                group.unit_context_type,
                experiment.unit_context_types.join(", ")
            )));
        }

        // Allocate a disjoint range; capacity overflow → FAILED_PRECONDITION.
        let range = repo
            .allocate_range(group_id, exp_id, requested_bp)
            .await
            .map_err(Status::from)?;

        // When the experiment is rule-bound, push the gate onto the rule so the
        // flag snapshot enforces exclusion. The randomization unit is the
        // GROUP's diversion unit (shared by all members), not the experiment's.
        if let Some(flag_rule_id) = experiment.flag_rule_id {
            let gate = ExclusionGate {
                group_salt: group.salt.clone(),
                context_type: group.unit_context_type.clone(),
                bucket_lo: range.lo,
                bucket_hi: range.hi,
            };
            // Best-effort consistency: if the gate write fails, release the
            // range we just allocated so we don't leak bucket space.
            if let Err(e) = gate_writer
                .set_rule_exclusion_gate(flag_rule_id, Some(gate))
                .await
            {
                let _ = repo.free_range(exp_id).await;
                return Err(Status::from(e));
            }
        }

        // Re-read the group for the post-allocation allocated/free split.
        let updated_group = repo.find_by_id(group_id).await.map_err(Status::from)?;

        // Echo the experiment with its new group assignment populated.
        let mut proto_exp = core_to_proto(&experiment);
        proto_exp.exclusion_group_id = Some(group_id.to_string());
        proto_exp.group_bucket_lo = Some(u32::from(range.lo));
        proto_exp.group_bucket_hi = Some(u32::from(range.hi));

        metrics::counter!("experimentation_service.assign_experiment_to_group.ok").increment(1);
        Ok(Response::new(
            stitchd_proto::experiments::v1::AssignExperimentToGroupResponse {
                experiment: Some(proto_exp),
                group: Some(group_to_proto(&updated_group)),
            },
        ))
    }

    #[instrument(skip(self))]
    async fn unassign_experiment(
        &self,
        request: Request<stitchd_proto::experiments::v1::UnassignExperimentRequest>,
    ) -> Result<Response<stitchd_proto::experiments::v1::UnassignExperimentResponse>, Status> {
        let repo = self.exclusion_group_repo()?;
        let gate_writer = self.rule_gate_writer()?;
        let req = request.into_inner();
        let exp_id = parse_experiment_id(&req.experiment_id)?;

        let experiment = self
            .experiment_repo
            .find_by_id(exp_id)
            .await
            .map_err(Status::from)?;

        // Reject while the bound flag is locked (running/paused) → 409.
        reject_if_flag_locked(&experiment)?;

        let group_id = experiment.exclusion_group_id;

        // Release the range, clear group columns, and clear the rule gate.
        self.release_experiment_group(repo.as_ref(), gate_writer.as_ref(), &experiment)
            .await?;

        // Echo the now-ungrouped experiment.
        let mut proto_exp = core_to_proto(&experiment);
        proto_exp.exclusion_group_id = None;
        proto_exp.group_bucket_lo = None;
        proto_exp.group_bucket_hi = None;

        // Return the group (post-release) when the experiment had one.
        let group_proto = if let Some(gid) = group_id {
            repo.find_by_id(gid)
                .await
                .ok()
                .map(|g| group_to_proto(&g))
        } else {
            None
        };

        metrics::counter!("experimentation_service.unassign_experiment.ok").increment(1);
        Ok(Response::new(
            stitchd_proto::experiments::v1::UnassignExperimentResponse {
                experiment: Some(proto_exp),
                group: group_proto,
            },
        ))
    }

    #[instrument(skip(self))]
    async fn get_experiment_interactions(
        &self,
        request: Request<stitchd_proto::experiments::v1::GetExperimentInteractionsRequest>,
    ) -> Result<Response<stitchd_proto::experiments::v1::GetExperimentInteractionsResponse>, Status>
    {
        let req = request.into_inner();
        let env_id = uuid::Uuid::parse_str(&req.env_id)
            .map_err(|_| Status::invalid_argument("invalid env_id UUID"))?;
        let experiment_id = uuid::Uuid::parse_str(&req.experiment_id)
            .map_err(|_| Status::invalid_argument("invalid experiment_id UUID"))?;

        let reader = self.interactions_reader.as_ref().ok_or_else(|| {
            Status::unimplemented("interactions_reader not configured on this service instance")
        })?;

        let rows = reader
            .list_interactions(env_id, experiment_id)
            .await
            .map_err(|e| Status::internal(format!("clickhouse error: {e}")))?;

        // Resolve the "other" experiment's name per row, caching lookups so a
        // pair appearing across many (context_type, metric_key) rows costs one
        // experiments-table read.
        let mut name_cache: std::collections::HashMap<uuid::Uuid, String> =
            std::collections::HashMap::new();
        let mut interactions = Vec::with_capacity(rows.len());
        for r in rows {
            let other_id = if r.experiment_id_a == experiment_id {
                r.experiment_id_b
            } else {
                r.experiment_id_a
            };
            let other_name = match name_cache.get(&other_id) {
                Some(n) => n.clone(),
                None => {
                    let name = match self
                        .experiment_repo
                        .find_by_id(ExperimentId::from_uuid(other_id))
                        .await
                    {
                        Ok(exp) => exp.name,
                        // A deleted / missing counterpart should not fail the
                        // whole read — surface an empty name for that row.
                        Err(_) => String::new(),
                    };
                    name_cache.insert(other_id, name.clone());
                    name
                }
            };
            interactions.push(stitchd_proto::experiments::v1::ExperimentInteraction {
                experiment_id_a: r.experiment_id_a.to_string(),
                experiment_id_b: r.experiment_id_b.to_string(),
                other_experiment_name: other_name,
                context_type: r.context_type,
                metric_key: r.metric_key,
                shared_count: r.shared_count,
                interaction_estimate: r.interaction_estimate,
                p_value: r.p_value,
                significant: r.significant,
                insufficient_data: r.insufficient_data,
            });
        }

        Ok(Response::new(
            stitchd_proto::experiments::v1::GetExperimentInteractionsResponse { interactions },
        ))
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use chrono::Utc;
    use std::sync::Arc;
    use stitchd_core::{
        experimentation::{Experiment, ExperimentIteration, ExperimentStatus},
        id::{EnvironmentId, ExperimentId, MetricId, RuleId},
    };
    use stitchd_db::stats_schedule::{StatsScheduleRow, UpsertStatsSchedule};
    use stitchd_db::{ComputationStatus, RepositoryError, StatsScheduleRepository};
    use stitchd_proto::analytics::v1::ExperimentResult as ProtoExperimentResult;
    use uuid::Uuid;

    // -----------------------------------------------------------------------
    // Mock analytics client
    // -----------------------------------------------------------------------

    /// Returns an empty stream — simulates an experiment with no results yet.
    struct EmptyAnalyticsMock;

    #[async_trait]
    impl crate::analytics_client::AnalyticsResultsPort for EmptyAnalyticsMock {
        async fn list_experiment_results(
            &self,
            _env_id: &str,
            _experiment_id: &str,
            _iteration_id: Option<&str>,
        ) -> Result<Vec<ProtoExperimentResult>, tonic::Status> {
            Ok(vec![])
        }

        async fn list_context_types(
            &self,
            _environment_id: &str,
        ) -> Result<Vec<String>, tonic::Status> {
            Ok(vec!["user".to_string()])
        }
    }

    /// Returns a fixed list of `ExperimentResult` protos — simulates analytics
    /// service ClickHouse data.
    struct AnalyticsMockWithData {
        results: Vec<ProtoExperimentResult>,
    }

    #[async_trait]
    impl crate::analytics_client::AnalyticsResultsPort for AnalyticsMockWithData {
        async fn list_experiment_results(
            &self,
            _env_id: &str,
            _experiment_id: &str,
            _iteration_id: Option<&str>,
        ) -> Result<Vec<ProtoExperimentResult>, tonic::Status> {
            Ok(self.results.clone())
        }

        async fn list_context_types(
            &self,
            _environment_id: &str,
        ) -> Result<Vec<String>, tonic::Status> {
            Ok(vec!["user".to_string()])
        }
    }

    /// Always returns an Internal gRPC error — simulates analytics-service failure.
    struct AnalyticsErrorMock;

    #[async_trait]
    impl crate::analytics_client::AnalyticsResultsPort for AnalyticsErrorMock {
        async fn list_experiment_results(
            &self,
            _env_id: &str,
            _experiment_id: &str,
            _iteration_id: Option<&str>,
        ) -> Result<Vec<ProtoExperimentResult>, tonic::Status> {
            Err(tonic::Status::internal("analytics-service unavailable"))
        }

        async fn list_context_types(
            &self,
            _environment_id: &str,
        ) -> Result<Vec<String>, tonic::Status> {
            Err(tonic::Status::internal("analytics-service unavailable"))
        }
    }

    /// Build a minimal `ExperimentResult` proto for use in tests.
    fn make_analytics_result(
        env_id: &str,
        experiment_id: &str,
        variant_key: &str,
        metric_key: &str,
        count: u64,
    ) -> ProtoExperimentResult {
        ProtoExperimentResult {
            env_id: env_id.to_string(),
            experiment_id: experiment_id.to_string(),
            iteration_id: Uuid::new_v4().to_string(),
            variant_key: variant_key.to_string(),
            metric_key: metric_key.to_string(),
            metric_type: "count".to_string(),
            variant_stats: serde_json::json!({ variant_key: count }).to_string(),
            frequentist_result: Some(serde_json::json!({ "p_value": 0.04 }).to_string()),
            bayesian_result: None,
            recommendation: "ship_treatment".to_string(),
            computed_at: "2026-05-01T00:00:00Z".to_string(),
            created_at: "2026-05-01T00:00:00Z".to_string(),
            context_type: "user".to_string(),
        }
    }

    // -----------------------------------------------------------------------
    // Stub repositories
    // -----------------------------------------------------------------------

    fn make_experiment(env_id: EnvironmentId) -> Experiment {
        let now = Utc::now();
        Experiment {
            id: ExperimentId::new(),
            environment_id: env_id,
            flag_id: stitchd_core::id::FlagId::new(),
            flag_key: None,
            variant_keys: vec![],
            flag_rule_id: Some(RuleId::new()),
            targets_default_rule: false,
            name: "Test Experiment".to_string(),
            description: Some("A description".to_string()),
            hypothesis: None,
            metric_ids: vec![MetricId::new()],
            guardrail_metric_ids: vec![],
            traffic_allocation: 100.0,
            min_sample_size: None,
            pre_period_days: 0,
            unit_context_types: vec!["user".to_string()],
            scheduled_start_at: None,
            scheduled_end_at: None,
            status: ExperimentStatus::Draft,
            created_at: now,
            updated_at: now,
            deleted_at: None,
            version: 1,
            exclusion_group_id: None,
            group_bucket_lo: None,
            group_bucket_hi: None,
        }
    }

    struct AlwaysSucceedRepo {
        env_id: EnvironmentId,
    }

    #[async_trait]
    impl ExperimentRepository for AlwaysSucceedRepo {
        async fn find_by_id(&self, id: ExperimentId) -> Result<Experiment, RepositoryError> {
            let mut exp = make_experiment(self.env_id);
            exp.id = id;
            Ok(exp)
        }

        async fn list_by_environment(
            &self,
            _env_id: EnvironmentId,
            _status_filter: Option<ExperimentStatus>,
        ) -> Result<Vec<Experiment>, RepositoryError> {
            Ok(vec![make_experiment(self.env_id)])
        }

        async fn list_by_environment_paginated(
            &self,
            _env_id: EnvironmentId,
            _offset: u64,
            _limit: u64,
        ) -> Result<(Vec<Experiment>, u64), RepositoryError> {
            let exp = make_experiment(self.env_id);
            Ok((vec![exp], 1))
        }

        async fn create(&self, _experiment: &Experiment) -> Result<(), RepositoryError> {
            Ok(())
        }

        async fn update(&self, experiment: &Experiment) -> Result<Experiment, RepositoryError> {
            Ok(experiment.clone())
        }

        async fn soft_delete(&self, _id: ExperimentId) -> Result<(), RepositoryError> {
            Ok(())
        }

        async fn list_iterations(
            &self,
            _experiment_id: ExperimentId,
        ) -> Result<Vec<ExperimentIteration>, RepositoryError> {
            Ok(vec![])
        }

        async fn apply_transition(
            &self,
            id: ExperimentId,
            to: ExperimentStatus,
            _actor_id: Option<stitchd_core::id::UserId>,
        ) -> Result<Experiment, RepositoryError> {
            let mut exp = make_experiment(self.env_id);
            exp.id = id;
            exp.status = to;
            Ok(exp)
        }

        async fn list_all_running(&self) -> Result<Vec<Experiment>, RepositoryError> {
            let mut exp = make_experiment(self.env_id);
            exp.status = ExperimentStatus::Running;
            Ok(vec![exp])
        }

        async fn find_active_experiment_for_flag(
            &self,
            _flag_id: stitchd_core::id::FlagId,
        ) -> Result<Option<ExperimentId>, RepositoryError> {
            Ok(None)
        }

        async fn find_iteration_by_id(
            &self,
            iteration_id: stitchd_core::id::ExperimentIterationId,
        ) -> Result<ExperimentIteration, RepositoryError> {
            Ok(ExperimentIteration {
                id: iteration_id,
                experiment_id: ExperimentId::new(),
                flag_id: stitchd_core::id::FlagId::new(),
                iteration_number: 1,
                started_at: Utc::now(),
                ended_at: None,
                metric_ids: vec![MetricId::new()],
                guardrail_metric_ids: vec![],
                traffic_allocation: 100.0,
                min_sample_size: None,
                targets_default_rule: false,
                pre_period_days: 0,
                unit_context_types: vec!["user".to_string()],
                default_rule_distribution: None,
                exclusion_group_id: None,
                group_bucket_lo: None,
                group_bucket_hi: None,
            })
        }
    }

    struct NotFoundRepo;

    #[async_trait]
    impl ExperimentRepository for NotFoundRepo {
        async fn find_by_id(&self, id: ExperimentId) -> Result<Experiment, RepositoryError> {
            Err(RepositoryError::NotFound { id: id.to_string() })
        }

        async fn list_by_environment(
            &self,
            env_id: EnvironmentId,
            _status_filter: Option<ExperimentStatus>,
        ) -> Result<Vec<Experiment>, RepositoryError> {
            Err(RepositoryError::NotFound {
                id: env_id.to_string(),
            })
        }

        async fn list_by_environment_paginated(
            &self,
            env_id: EnvironmentId,
            _offset: u64,
            _limit: u64,
        ) -> Result<(Vec<Experiment>, u64), RepositoryError> {
            Err(RepositoryError::NotFound {
                id: env_id.to_string(),
            })
        }

        async fn create(&self, _experiment: &Experiment) -> Result<(), RepositoryError> {
            Err(RepositoryError::NotFound {
                id: "new".to_string(),
            })
        }

        async fn update(&self, _experiment: &Experiment) -> Result<Experiment, RepositoryError> {
            Err(RepositoryError::NotFound {
                id: "none".to_string(),
            })
        }

        async fn soft_delete(&self, id: ExperimentId) -> Result<(), RepositoryError> {
            Err(RepositoryError::NotFound { id: id.to_string() })
        }

        async fn list_iterations(
            &self,
            _experiment_id: ExperimentId,
        ) -> Result<Vec<ExperimentIteration>, RepositoryError> {
            Ok(vec![])
        }

        async fn apply_transition(
            &self,
            id: ExperimentId,
            _to: ExperimentStatus,
            _actor_id: Option<stitchd_core::id::UserId>,
        ) -> Result<Experiment, RepositoryError> {
            Err(RepositoryError::NotFound { id: id.to_string() })
        }

        async fn list_all_running(&self) -> Result<Vec<Experiment>, RepositoryError> {
            Err(RepositoryError::Database(sqlx::Error::RowNotFound))
        }

        async fn find_active_experiment_for_flag(
            &self,
            _flag_id: stitchd_core::id::FlagId,
        ) -> Result<Option<ExperimentId>, RepositoryError> {
            Ok(None)
        }

        async fn find_iteration_by_id(
            &self,
            iteration_id: stitchd_core::id::ExperimentIterationId,
        ) -> Result<ExperimentIteration, RepositoryError> {
            Err(RepositoryError::NotFound {
                id: iteration_id.to_string(),
            })
        }
    }

    // -----------------------------------------------------------------------
    // Stub schedule repos
    // -----------------------------------------------------------------------

    /// Returns `None` — simulates an experiment that has never been scheduled.
    struct NoScheduleRepo;

    #[async_trait]
    impl StatsScheduleRepository for NoScheduleRepo {
        async fn upsert_schedule(
            &self,
            _input: &UpsertStatsSchedule,
        ) -> Result<StatsScheduleRow, sqlx::Error> {
            Err(sqlx::Error::RowNotFound)
        }

        async fn get_schedule_for_experiment(
            &self,
            _experiment_id: Uuid,
        ) -> Result<Option<StatsScheduleRow>, sqlx::Error> {
            Ok(None)
        }

        async fn update_last_computed(
            &self,
            _iteration_id: Uuid,
            _last_computed_at_ms: i64,
        ) -> Result<(), sqlx::Error> {
            Ok(())
        }
    }

    /// Returns a schedule row with `last_computed_at` set to the given timestamp.
    struct FixedScheduleRepo {
        last_computed_at: Option<chrono::DateTime<Utc>>,
        next_run_at: Option<chrono::DateTime<Utc>>,
        status: ComputationStatus,
    }

    #[async_trait]
    impl StatsScheduleRepository for FixedScheduleRepo {
        async fn upsert_schedule(
            &self,
            _input: &UpsertStatsSchedule,
        ) -> Result<StatsScheduleRow, sqlx::Error> {
            Err(sqlx::Error::RowNotFound)
        }

        async fn get_schedule_for_experiment(
            &self,
            _experiment_id: Uuid,
        ) -> Result<Option<StatsScheduleRow>, sqlx::Error> {
            Ok(Some(StatsScheduleRow {
                experiment_id: Uuid::new_v4(),
                last_computed_at: self.last_computed_at,
                next_run_at: self.next_run_at,
                computation_status: self.status.clone(),
                updated_at: Utc::now(),
            }))
        }

        async fn update_last_computed(
            &self,
            _iteration_id: Uuid,
            _last_computed_at_ms: i64,
        ) -> Result<(), sqlx::Error> {
            Ok(())
        }
    }

    fn make_service(env_id: EnvironmentId) -> ExperimentationServiceImpl {
        ExperimentationServiceImpl::new(
            Arc::new(AlwaysSucceedRepo { env_id }),
            Arc::new(EmptyAnalyticsMock),
            Arc::new(NoScheduleRepo),
            None,
        )
    }

    // -----------------------------------------------------------------------
    // Helper: valid env UUID string
    // -----------------------------------------------------------------------
    fn env_uuid() -> (EnvironmentId, String) {
        let id = EnvironmentId::new();
        let s = id.to_string();
        (id, s)
    }

    // -----------------------------------------------------------------------
    // proto_status_to_core tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_proto_status_draft_maps_to_draft() {
        use stitchd_proto::experiments::v1::ExperimentStatus as PS;
        let result = proto_status_to_core(PS::Draft as i32).unwrap();
        assert_eq!(result, ExperimentStatus::Draft);
    }

    #[test]
    fn test_proto_status_active_maps_to_running() {
        use stitchd_proto::experiments::v1::ExperimentStatus as PS;
        let result = proto_status_to_core(PS::Active as i32).unwrap();
        assert_eq!(result, ExperimentStatus::Running);
    }

    #[test]
    fn test_proto_status_paused_maps_to_paused() {
        use stitchd_proto::experiments::v1::ExperimentStatus as PS;
        let result = proto_status_to_core(PS::Paused as i32).unwrap();
        assert_eq!(result, ExperimentStatus::Paused);
    }

    #[test]
    fn test_proto_status_concluded_maps_to_stopped() {
        use stitchd_proto::experiments::v1::ExperimentStatus as PS;
        let result = proto_status_to_core(PS::Concluded as i32).unwrap();
        assert_eq!(result, ExperimentStatus::Stopped);
    }

    #[test]
    fn test_proto_status_unspecified_maps_to_draft() {
        let result = proto_status_to_core(0).unwrap();
        assert_eq!(result, ExperimentStatus::Draft);
    }

    // -----------------------------------------------------------------------
    // core_status_to_proto tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_core_draft_to_proto() {
        use stitchd_proto::experiments::v1::ExperimentStatus as PS;
        assert_eq!(
            core_status_to_proto(ExperimentStatus::Draft),
            PS::Draft as i32
        );
    }

    #[test]
    fn test_core_running_to_proto() {
        use stitchd_proto::experiments::v1::ExperimentStatus as PS;
        assert_eq!(
            core_status_to_proto(ExperimentStatus::Running),
            PS::Active as i32
        );
    }

    #[test]
    fn test_core_paused_to_proto() {
        use stitchd_proto::experiments::v1::ExperimentStatus as PS;
        assert_eq!(
            core_status_to_proto(ExperimentStatus::Paused),
            PS::Paused as i32
        );
    }

    #[test]
    fn test_core_stopped_to_proto() {
        use stitchd_proto::experiments::v1::ExperimentStatus as PS;
        assert_eq!(
            core_status_to_proto(ExperimentStatus::Stopped),
            PS::Concluded as i32
        );
    }

    // -----------------------------------------------------------------------
    // core_to_proto tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_core_to_proto_maps_fields() {
        let env_id = EnvironmentId::new();
        let exp = make_experiment(env_id);
        let proto = core_to_proto(&exp);
        assert_eq!(proto.id, exp.id.to_string());
        assert_eq!(proto.environment_id, env_id.to_string());
        assert_eq!(proto.name, "Test Experiment");
        assert_eq!(proto.description, "A description");
        assert_eq!(proto.version, 1);
    }

    #[test]
    fn test_core_to_proto_empty_description() {
        let env_id = EnvironmentId::new();
        let mut exp = make_experiment(env_id);
        exp.description = None;
        let proto = core_to_proto(&exp);
        assert_eq!(proto.description, "");
    }

    // -----------------------------------------------------------------------
    // RepositoryError → Status conversion tests (via From impl in stitchd-db)
    // -----------------------------------------------------------------------

    #[test]
    fn test_repo_err_not_found_to_not_found_status() {
        let s = Status::from(RepositoryError::NotFound { id: "abc".into() });
        assert_eq!(s.code(), tonic::Code::NotFound);
        assert!(s.message().contains("abc"));
    }

    #[test]
    fn test_repo_err_version_conflict_to_aborted() {
        let s = Status::from(RepositoryError::VersionConflict {
            expected: 1,
            actual: 2,
        });
        assert_eq!(s.code(), tonic::Code::Aborted);
    }

    #[test]
    fn test_repo_err_unique_violation_to_already_exists() {
        let s = Status::from(RepositoryError::UniqueViolation {
            field: "flag_rule_id".into(),
        });
        assert_eq!(s.code(), tonic::Code::AlreadyExists);
    }

    #[test]
    fn test_repo_err_invalid_state_to_failed_precondition() {
        let s = Status::from(RepositoryError::InvalidState {
            reason: "cannot mutate running experiment".into(),
        });
        assert_eq!(s.code(), tonic::Code::FailedPrecondition);
    }

    #[test]
    fn test_repo_err_database_to_internal() {
        let s = Status::from(RepositoryError::Database(sqlx::Error::RowNotFound));
        assert_eq!(s.code(), tonic::Code::Internal);
    }

    #[test]
    fn test_repo_err_unexpected_to_internal() {
        let s = Status::from(RepositoryError::Unexpected(anyhow::anyhow!(
            "unexpected error"
        )));
        assert_eq!(s.code(), tonic::Code::Internal);
    }

    // -----------------------------------------------------------------------
    // CreateExperiment handler tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_create_experiment_missing_experiment_field_returns_invalid_argument() {
        let (env_id, _) = env_uuid();
        let svc = make_service(env_id);
        let req = tonic::Request::new(CreateExperimentRequest { experiment: None });
        let result = svc.create_experiment(req).await;
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().code(), tonic::Code::InvalidArgument);
    }

    #[tokio::test]
    async fn test_create_experiment_invalid_env_id_returns_invalid_argument() {
        let (env_id, _) = env_uuid();
        let svc = make_service(env_id);
        let proto_exp = stitchd_proto::experiments::v1::Experiment {
            environment_id: "not-a-uuid".to_string(),
            ..Default::default()
        };
        let req = tonic::Request::new(CreateExperimentRequest {
            experiment: Some(proto_exp),
        });
        let result = svc.create_experiment(req).await;
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().code(), tonic::Code::InvalidArgument);
    }

    #[tokio::test]
    async fn test_create_experiment_success_returns_experiment() {
        let (env_id, env_id_str) = env_uuid();
        let svc = make_service(env_id);
        // Provide valid binding fields (GL-08): targets_default_rule=true satisfies
        // the XOR invariant; flag_key + unit_context_types pass analytics validation
        // (EmptyAnalyticsMock returns ["user"]).
        let proto_exp = stitchd_proto::experiments::v1::Experiment {
            environment_id: env_id_str.clone(),
            name: "My Experiment".to_string(),
            description: "Testing".to_string(),
            flag_key: "my-flag".to_string(),
            targets_default_rule: true,
            unit_context_types: vec!["user".to_string()],
            ..Default::default()
        };
        let req = tonic::Request::new(CreateExperimentRequest {
            experiment: Some(proto_exp),
        });
        let result = svc.create_experiment(req).await;
        assert!(result.is_ok(), "create failed: {:?}", result.unwrap_err());
        let resp = result.unwrap().into_inner();
        assert_eq!(resp.environment_id, env_id_str);
        assert_eq!(resp.name, "My Experiment");
    }

    #[tokio::test]
    async fn test_create_experiment_repo_failure_returns_error() {
        let svc = ExperimentationServiceImpl::new(
            Arc::new(NotFoundRepo),
            Arc::new(EmptyAnalyticsMock),
            Arc::new(NoScheduleRepo),
            None,
        );
        let env_id = EnvironmentId::new();
        // Provide valid binding fields so we reach the repo call (GL-08).
        let proto_exp = stitchd_proto::experiments::v1::Experiment {
            environment_id: env_id.to_string(),
            name: "Fail".to_string(),
            flag_key: "my-flag".to_string(),
            targets_default_rule: true,
            unit_context_types: vec!["user".to_string()],
            ..Default::default()
        };
        let req = tonic::Request::new(CreateExperimentRequest {
            experiment: Some(proto_exp),
        });
        let result = svc.create_experiment(req).await;
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().code(), tonic::Code::NotFound);
    }

    // -----------------------------------------------------------------------
    // GetExperiment handler tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_get_experiment_success() {
        let (env_id, _) = env_uuid();
        let svc = make_service(env_id);
        let exp_id = ExperimentId::new();
        let req = tonic::Request::new(GetExperimentRequest {
            environment_id: env_id.to_string(),
            experiment_id: exp_id.to_string(),
        });
        let result = svc.get_experiment(req).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap().into_inner().id, exp_id.to_string());
    }

    #[tokio::test]
    async fn test_get_experiment_invalid_id_returns_invalid_argument() {
        let (env_id, _) = env_uuid();
        let svc = make_service(env_id);
        let req = tonic::Request::new(GetExperimentRequest {
            environment_id: env_id.to_string(),
            experiment_id: "not-a-uuid".to_string(),
        });
        let result = svc.get_experiment(req).await;
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().code(), tonic::Code::InvalidArgument);
    }

    #[tokio::test]
    async fn test_get_experiment_not_found_returns_not_found() {
        let svc = ExperimentationServiceImpl::new(
            Arc::new(NotFoundRepo),
            Arc::new(EmptyAnalyticsMock),
            Arc::new(NoScheduleRepo),
            None,
        );
        let exp_id = ExperimentId::new();
        let req = tonic::Request::new(GetExperimentRequest {
            environment_id: EnvironmentId::new().to_string(),
            experiment_id: exp_id.to_string(),
        });
        let result = svc.get_experiment(req).await;
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().code(), tonic::Code::NotFound);
    }

    // -----------------------------------------------------------------------
    // ListExperiments handler tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_list_experiments_success_returns_list() {
        let (env_id, env_id_str) = env_uuid();
        let svc = make_service(env_id);
        let req = tonic::Request::new(ListExperimentsRequest {
            environment_id: env_id_str,
            ..Default::default()
        });
        let result = svc.list_experiments(req).await;
        assert!(result.is_ok());
        let resp = result.unwrap().into_inner();
        assert_eq!(resp.experiments.len(), 1);
    }

    #[tokio::test]
    async fn test_list_experiments_invalid_env_id_returns_invalid_argument() {
        let (env_id, _) = env_uuid();
        let svc = make_service(env_id);
        let req = tonic::Request::new(ListExperimentsRequest {
            environment_id: "bad-uuid".to_string(),
            ..Default::default()
        });
        let result = svc.list_experiments(req).await;
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().code(), tonic::Code::InvalidArgument);
    }

    #[tokio::test]
    async fn test_list_experiments_repo_failure_returns_error() {
        let svc = ExperimentationServiceImpl::new(
            Arc::new(NotFoundRepo),
            Arc::new(EmptyAnalyticsMock),
            Arc::new(NoScheduleRepo),
            None,
        );
        let req = tonic::Request::new(ListExperimentsRequest {
            environment_id: EnvironmentId::new().to_string(),
            ..Default::default()
        });
        let result = svc.list_experiments(req).await;
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().code(), tonic::Code::NotFound);
    }

    // -----------------------------------------------------------------------
    // GetResults handler tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_get_results_invalid_id_returns_invalid_argument() {
        let (env_id, _) = env_uuid();
        let svc = make_service(env_id);
        let req = tonic::Request::new(GetResultsRequest {
            environment_id: env_id.to_string(),
            experiment_id: "bad-uuid".to_string(),
        });
        let result = svc.get_results(req).await;
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().code(), tonic::Code::InvalidArgument);
    }

    #[tokio::test]
    async fn test_get_results_empty_returns_empty_results() {
        let (env_id, _) = env_uuid();
        let svc = make_service(env_id);
        let exp_id = Uuid::new_v4();
        let req = tonic::Request::new(GetResultsRequest {
            environment_id: env_id.to_string(),
            experiment_id: exp_id.to_string(),
        });
        let result = svc.get_results(req).await;
        assert!(result.is_ok());
        let resp = result.unwrap().into_inner();
        assert_eq!(resp.experiment_id, exp_id.to_string());
        assert!(resp.variant_results.is_empty());
        assert_eq!(resp.computed_at_ms, 0);
    }

    #[tokio::test]
    async fn test_get_results_with_data_returns_variant_results() {
        let (env_id, env_id_str) = env_uuid();
        let exp_id = Uuid::new_v4();
        let results = vec![
            make_analytics_result(&env_id_str, &exp_id.to_string(), "control", "checkout", 100),
            make_analytics_result(
                &env_id_str,
                &exp_id.to_string(),
                "treatment",
                "checkout",
                120,
            ),
        ];
        let svc = ExperimentationServiceImpl::new(
            Arc::new(AlwaysSucceedRepo { env_id }),
            Arc::new(AnalyticsMockWithData { results }),
            Arc::new(NoScheduleRepo),
            None,
        );
        let req = tonic::Request::new(GetResultsRequest {
            environment_id: env_id_str,
            experiment_id: exp_id.to_string(),
        });
        let result = svc.get_results(req).await;
        assert!(result.is_ok());
        let resp = result.unwrap().into_inner();
        assert_eq!(resp.experiment_id, exp_id.to_string());
        assert_eq!(resp.variant_results.len(), 2);
        // Both variants should have p_value_present since we set frequentist_result
        for vr in &resp.variant_results {
            assert!(
                vr.p_value_present,
                "variant {} should have p_value_present",
                vr.variant_key
            );
            assert!((vr.p_value - 0.04).abs() < 1e-9);
        }
    }

    // -----------------------------------------------------------------------
    // Staleness tests (Phase 5)
    // -----------------------------------------------------------------------

    /// Task 1.5: Results now come from analytics-service gRPC (ClickHouse), not PostgreSQL.
    #[tokio::test]
    async fn test_get_results_reads_from_analytics_service_grpc() {
        let (env_id, env_id_str) = env_uuid();
        let exp_id = Uuid::new_v4();
        let results = vec![make_analytics_result(
            &env_id_str,
            &exp_id.to_string(),
            "control",
            "checkout",
            50,
        )];
        let svc = ExperimentationServiceImpl::new(
            Arc::new(AlwaysSucceedRepo { env_id }),
            Arc::new(AnalyticsMockWithData { results }),
            Arc::new(NoScheduleRepo),
            None,
        );
        let req = tonic::Request::new(GetResultsRequest {
            environment_id: env_id_str,
            experiment_id: exp_id.to_string(),
        });
        let resp = svc.get_results(req).await.unwrap().into_inner();
        assert_eq!(resp.variant_results.len(), 1);
        assert_eq!(resp.variant_results[0].variant_key, "control");
    }

    /// Task 1.5 extra: analytics-service error is surfaced as Internal gRPC status.
    #[tokio::test]
    async fn test_get_results_analytics_error_returns_internal() {
        let (env_id, env_id_str) = env_uuid();
        let svc = ExperimentationServiceImpl::new(
            Arc::new(AlwaysSucceedRepo { env_id }),
            Arc::new(AnalyticsErrorMock),
            Arc::new(NoScheduleRepo),
            None,
        );
        let req = tonic::Request::new(GetResultsRequest {
            environment_id: env_id_str,
            experiment_id: Uuid::new_v4().to_string(),
        });
        let err = svc.get_results(req).await.unwrap_err();
        assert_eq!(err.code(), tonic::Code::Internal);
        assert!(err.message().contains("analytics-service error"));
    }

    /// Task 2a: is_stale = true when no schedule row exists (never computed).
    #[tokio::test]
    async fn test_get_results_is_stale_when_never_scheduled() {
        let (env_id, _) = env_uuid();
        let svc = make_service(env_id); // uses NoScheduleRepo
        let req = tonic::Request::new(GetResultsRequest {
            environment_id: env_id.to_string(),
            experiment_id: Uuid::new_v4().to_string(),
        });
        let resp = svc.get_results(req).await.unwrap().into_inner();
        assert!(resp.is_stale);
        assert_eq!(resp.computation_status, "");
    }

    /// Task 2b: is_stale = true when last_computed_at is >60 min ago.
    #[tokio::test]
    async fn test_get_results_is_stale_when_last_computed_over_60_min_ago() {
        let (env_id, _) = env_uuid();
        let old_computed = Utc::now() - chrono::Duration::minutes(61);
        let svc = ExperimentationServiceImpl::new(
            Arc::new(AlwaysSucceedRepo { env_id }),
            Arc::new(EmptyAnalyticsMock),
            Arc::new(FixedScheduleRepo {
                last_computed_at: Some(old_computed),
                next_run_at: None,
                status: ComputationStatus::Ready,
            }),
            None,
        );
        let req = tonic::Request::new(GetResultsRequest {
            environment_id: env_id.to_string(),
            experiment_id: Uuid::new_v4().to_string(),
        });
        let resp = svc.get_results(req).await.unwrap().into_inner();
        assert!(
            resp.is_stale,
            "should be stale when last_computed_at is >60 min ago"
        );
        assert_eq!(resp.computation_status, "ready");
    }

    /// Task 2c: is_stale = false when last_computed_at is recent (<60 min ago).
    #[tokio::test]
    async fn test_get_results_not_stale_when_recently_computed() {
        let (env_id, _) = env_uuid();
        let next_run = Utc::now() + chrono::Duration::minutes(30);
        let svc = ExperimentationServiceImpl::new(
            Arc::new(AlwaysSucceedRepo { env_id }),
            Arc::new(EmptyAnalyticsMock),
            Arc::new(FixedScheduleRepo {
                last_computed_at: Some(Utc::now() - chrono::Duration::minutes(5)),
                next_run_at: Some(next_run),
                status: ComputationStatus::Ready,
            }),
            None,
        );
        let req = tonic::Request::new(GetResultsRequest {
            environment_id: env_id.to_string(),
            experiment_id: Uuid::new_v4().to_string(),
        });
        let resp = svc.get_results(req).await.unwrap().into_inner();
        assert!(!resp.is_stale, "should not be stale when recently computed");
        assert_eq!(resp.next_run_at_ms, next_run.timestamp_millis());
        assert_eq!(resp.computation_status, "ready");
    }

    /// Task 2d: computation_status maps all enum variants correctly.
    #[tokio::test]
    async fn test_get_results_computation_status_maps_correctly() {
        let (env_id, _) = env_uuid();
        for (status, expected_str) in [
            (ComputationStatus::Ready, "ready"),
            (ComputationStatus::Computing, "computing"),
            (ComputationStatus::NeverComputed, "never_computed"),
        ] {
            let svc = ExperimentationServiceImpl::new(
                Arc::new(AlwaysSucceedRepo { env_id }),
                Arc::new(EmptyAnalyticsMock),
                Arc::new(FixedScheduleRepo {
                    last_computed_at: Some(Utc::now()),
                    next_run_at: None,
                    status,
                }),
                None,
            );
            let req = tonic::Request::new(GetResultsRequest {
                environment_id: env_id.to_string(),
                experiment_id: Uuid::new_v4().to_string(),
            });
            let resp = svc.get_results(req).await.unwrap().into_inner();
            assert_eq!(resp.computation_status, expected_str);
        }
    }

    // -----------------------------------------------------------------------
    // Flag-lock integration tests (Tasks 4+5)
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_create_experiment_active_status_without_flag_client_skips_verification() {
        // When flag_client is None and status is ACTIVE, we should not panic and
        // still create the experiment (flag verification skipped for flag-lock check;
        // binding validator also runs with flag_client=None → flag checks skipped).
        let (env_id, env_id_str) = env_uuid();
        let svc = make_service(env_id); // flag_client = None
        let proto_exp = stitchd_proto::experiments::v1::Experiment {
            environment_id: env_id_str.clone(),
            name: "Active Experiment".to_string(),
            flag_key: "my-flag".to_string(),
            // ACTIVE status; binding valid: targets_default_rule=true, unit_context_types provided.
            status: stitchd_proto::experiments::v1::ExperimentStatus::Active as i32,
            targets_default_rule: true,
            unit_context_types: vec!["user".to_string()],
            ..Default::default()
        };
        let req = tonic::Request::new(CreateExperimentRequest {
            experiment: Some(proto_exp),
        });
        let result = svc.create_experiment(req).await;
        // No flag client = skip flag check → create succeeds
        assert!(result.is_ok(), "create failed: {:?}", result.unwrap_err());
    }

    #[tokio::test]
    async fn test_create_experiment_active_status_with_empty_flag_key_skips_flag_check() {
        // When flag_key is empty (no binding at all), binding validation will
        // fail because flag_key is required. This test now asserts that
        // a missing flag_key + no binding config returns InvalidArgument.
        let (env_id, env_id_str) = env_uuid();
        let svc = make_service(env_id);
        let proto_exp = stitchd_proto::experiments::v1::Experiment {
            environment_id: env_id_str.clone(),
            name: "Active No Key".to_string(),
            // No flag_key, no binding → targets_default_rule=false, flag_rule_id="" →
            // the XOR invariant (neither set) fires before flag_key check.
            status: stitchd_proto::experiments::v1::ExperimentStatus::Active as i32,
            unit_context_types: vec!["user".to_string()],
            ..Default::default()
        };
        let req = tonic::Request::new(CreateExperimentRequest {
            experiment: Some(proto_exp),
        });
        let result = svc.create_experiment(req).await;
        // Binding validation fires: neither flag_rule_id nor targets_default_rule set.
        assert!(result.is_err());
        assert_eq!(
            result.unwrap_err().code(),
            tonic::Code::InvalidArgument,
            "expected invalid_argument for missing binding"
        );
    }

    // -----------------------------------------------------------------------
    // ListRunningExperiments tests
    // -----------------------------------------------------------------------

    /// Happy path: the mock returns one running experiment → one stream item.
    #[tokio::test]
    async fn test_list_running_experiments_returns_running_items() {
        use futures::StreamExt as _;
        let (env_id, _) = env_uuid();
        let svc = make_service(env_id);
        let req = tonic::Request::new(ListRunningExperimentsRequest {});
        let result = svc.list_running_experiments(req).await;
        assert!(result.is_ok(), "list_running_experiments should succeed");
        let mut stream = result.unwrap().into_inner();
        // AlwaysSucceedRepo.list_all_running returns one experiment with status=Running.
        // list_iterations returns [] → no active iteration → no stream item.
        // (This is correct behaviour: no iteration = nothing to compute on.)
        let mut count = 0usize;
        while let Some(item) = stream.next().await {
            assert!(item.is_ok());
            let running = item.unwrap();
            assert_eq!(running.status, "running");
            count += 1;
        }
        // No active iteration in stub → count is 0 (filter in handler).
        assert_eq!(count, 0);
    }

    /// Stub repo that returns one running experiment with an active iteration.
    struct RunningWithIterationRepo {
        env_id: EnvironmentId,
    }

    #[async_trait]
    impl ExperimentRepository for RunningWithIterationRepo {
        async fn find_by_id(&self, id: ExperimentId) -> Result<Experiment, RepositoryError> {
            let mut exp = make_experiment(self.env_id);
            exp.id = id;
            exp.status = ExperimentStatus::Running;
            Ok(exp)
        }

        async fn list_by_environment(
            &self,
            _env_id: EnvironmentId,
            _status_filter: Option<ExperimentStatus>,
        ) -> Result<Vec<Experiment>, RepositoryError> {
            let mut exp = make_experiment(self.env_id);
            exp.status = ExperimentStatus::Running;
            Ok(vec![exp])
        }

        async fn list_by_environment_paginated(
            &self,
            _env_id: EnvironmentId,
            _offset: u64,
            _limit: u64,
        ) -> Result<(Vec<Experiment>, u64), RepositoryError> {
            let mut exp = make_experiment(self.env_id);
            exp.status = ExperimentStatus::Running;
            Ok((vec![exp], 1))
        }

        async fn create(&self, _experiment: &Experiment) -> Result<(), RepositoryError> {
            Ok(())
        }

        async fn update(&self, experiment: &Experiment) -> Result<Experiment, RepositoryError> {
            Ok(experiment.clone())
        }

        async fn soft_delete(&self, _id: ExperimentId) -> Result<(), RepositoryError> {
            Ok(())
        }

        async fn list_iterations(
            &self,
            experiment_id: ExperimentId,
        ) -> Result<Vec<ExperimentIteration>, RepositoryError> {
            use stitchd_core::id::ExperimentIterationId;
            Ok(vec![ExperimentIteration {
                id: ExperimentIterationId::new(),
                experiment_id,
                flag_id: stitchd_core::id::FlagId::new(),
                iteration_number: 1,
                started_at: Utc::now(),
                ended_at: None, // still active
                metric_ids: vec![MetricId::new()],
                guardrail_metric_ids: vec![],
                traffic_allocation: 100.0,
                min_sample_size: None,
                targets_default_rule: false,
                pre_period_days: 0,
                unit_context_types: vec!["user".to_string()],
                default_rule_distribution: None,
                exclusion_group_id: None,
                group_bucket_lo: None,
                group_bucket_hi: None,
            }])
        }

        async fn apply_transition(
            &self,
            id: ExperimentId,
            to: ExperimentStatus,
            _actor_id: Option<stitchd_core::id::UserId>,
        ) -> Result<Experiment, RepositoryError> {
            let mut exp = make_experiment(self.env_id);
            exp.id = id;
            exp.status = to;
            Ok(exp)
        }

        async fn list_all_running(&self) -> Result<Vec<Experiment>, RepositoryError> {
            let mut exp = make_experiment(self.env_id);
            exp.status = ExperimentStatus::Running;
            Ok(vec![exp])
        }

        async fn find_active_experiment_for_flag(
            &self,
            _flag_id: stitchd_core::id::FlagId,
        ) -> Result<Option<ExperimentId>, RepositoryError> {
            Ok(None)
        }

        async fn find_iteration_by_id(
            &self,
            iteration_id: stitchd_core::id::ExperimentIterationId,
        ) -> Result<ExperimentIteration, RepositoryError> {
            Ok(ExperimentIteration {
                id: iteration_id,
                experiment_id: ExperimentId::new(),
                flag_id: stitchd_core::id::FlagId::new(),
                iteration_number: 1,
                started_at: Utc::now(),
                ended_at: None,
                metric_ids: vec![MetricId::new()],
                guardrail_metric_ids: vec![],
                traffic_allocation: 100.0,
                min_sample_size: None,
                targets_default_rule: false,
                pre_period_days: 0,
                unit_context_types: vec!["user".to_string()],
                default_rule_distribution: None,
                exclusion_group_id: None,
                group_bucket_lo: None,
                group_bucket_hi: None,
            })
        }
    }

    /// When the experiment has an active iteration, it appears in the stream.
    #[tokio::test]
    async fn test_list_running_experiments_with_active_iteration_yields_item() {
        use futures::StreamExt as _;
        let (env_id, _) = env_uuid();
        let svc = ExperimentationServiceImpl::new(
            Arc::new(RunningWithIterationRepo { env_id }),
            Arc::new(EmptyAnalyticsMock),
            Arc::new(NoScheduleRepo),
            None,
        );
        let req = tonic::Request::new(ListRunningExperimentsRequest {});
        let result = svc.list_running_experiments(req).await;
        assert!(result.is_ok());
        let mut stream = result.unwrap().into_inner();
        let item = stream.next().await;
        assert!(item.is_some(), "expected at least one stream item");
        let running = item.unwrap().unwrap();
        assert_eq!(running.status, "running");
        assert!(!running.experiment_id.is_empty());
        assert!(!running.iteration_id.is_empty());
        assert_eq!(
            running.metric_ids.len(),
            1,
            "iteration snapshot exposes a single metric id"
        );
    }

    // -----------------------------------------------------------------------
    // GetExperimentIteration tests
    // -----------------------------------------------------------------------

    /// Happy path: valid iteration_id returns a populated proto.
    #[tokio::test]
    async fn test_get_experiment_iteration_success() {
        use stitchd_core::id::ExperimentIterationId;
        let (env_id, _) = env_uuid();
        let svc = make_service(env_id);
        let iter_id = ExperimentIterationId::new();
        let req = tonic::Request::new(GetExperimentIterationRequest {
            iteration_id: iter_id.to_string(),
        });
        let result = svc.get_experiment_iteration(req).await;
        assert!(result.is_ok(), "get_experiment_iteration should succeed");
        let proto = result.unwrap().into_inner();
        assert_eq!(proto.id, iter_id.to_string());
        assert_eq!(proto.iteration_number, 1);
        assert_eq!(
            proto.metric_ids.len(),
            1,
            "iteration snapshot exposes a single metric id"
        );
    }

    /// Invalid UUID returns InvalidArgument.
    #[tokio::test]
    async fn test_get_experiment_iteration_invalid_id_returns_invalid_argument() {
        let (env_id, _) = env_uuid();
        let svc = make_service(env_id);
        let req = tonic::Request::new(GetExperimentIterationRequest {
            iteration_id: "not-a-uuid".to_string(),
        });
        let result = svc.get_experiment_iteration(req).await;
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().code(), tonic::Code::InvalidArgument);
    }

    /// Repo returns NotFound → gRPC status NotFound.
    #[tokio::test]
    async fn test_get_experiment_iteration_not_found() {
        use stitchd_core::id::ExperimentIterationId;
        let svc = ExperimentationServiceImpl::new(
            Arc::new(NotFoundRepo),
            Arc::new(EmptyAnalyticsMock),
            Arc::new(NoScheduleRepo),
            None,
        );
        let req = tonic::Request::new(GetExperimentIterationRequest {
            iteration_id: ExperimentIterationId::new().to_string(),
        });
        let result = svc.get_experiment_iteration(req).await;
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().code(), tonic::Code::NotFound);
    }

    // -----------------------------------------------------------------------
    // UpdateIterationLastComputed tests
    // -----------------------------------------------------------------------

    /// Happy path: valid UUID and a timestamp succeeds.
    #[tokio::test]
    async fn test_update_iteration_last_computed_success() {
        use stitchd_core::id::ExperimentIterationId;
        let (env_id, _) = env_uuid();
        let svc = make_service(env_id);
        let req = tonic::Request::new(UpdateIterationLastComputedRequest {
            iteration_id: ExperimentIterationId::new().to_string(),
            last_computed_at_ms: Utc::now().timestamp_millis(),
        });
        let result = svc.update_iteration_last_computed(req).await;
        assert!(
            result.is_ok(),
            "update_iteration_last_computed should succeed"
        );
    }

    /// Invalid UUID returns InvalidArgument.
    #[tokio::test]
    async fn test_update_iteration_last_computed_invalid_id_returns_invalid_argument() {
        let (env_id, _) = env_uuid();
        let svc = make_service(env_id);
        let req = tonic::Request::new(UpdateIterationLastComputedRequest {
            iteration_id: "bad-uuid".to_string(),
            last_computed_at_ms: 0,
        });
        let result = svc.update_iteration_last_computed(req).await;
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().code(), tonic::Code::InvalidArgument);
    }

    /// Zero timestamp is accepted (edge case).
    #[tokio::test]
    async fn test_update_iteration_last_computed_zero_timestamp_accepted() {
        use stitchd_core::id::ExperimentIterationId;
        let (env_id, _) = env_uuid();
        let svc = make_service(env_id);
        let req = tonic::Request::new(UpdateIterationLastComputedRequest {
            iteration_id: ExperimentIterationId::new().to_string(),
            last_computed_at_ms: 0,
        });
        let result = svc.update_iteration_last_computed(req).await;
        assert!(result.is_ok());
    }

    // -----------------------------------------------------------------------
    // transition_experiment dictionary-refresh hook tests (Phase 4)
    // -----------------------------------------------------------------------

    /// Recording mock that counts refresher invocations.
    struct RecordingRefresher {
        count: Arc<std::sync::atomic::AtomicUsize>,
    }

    #[async_trait::async_trait]
    impl crate::dict_refresh::DictionaryRefresher for RecordingRefresher {
        async fn reload_experiment_iterations_active(
            &self,
        ) -> Result<(), clickhouse::error::Error> {
            self.count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(())
        }
    }

    async fn await_count(count: &std::sync::atomic::AtomicUsize, expected: usize) {
        for _ in 0..50 {
            if count.load(std::sync::atomic::Ordering::SeqCst) >= expected {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        panic!(
            "expected refresher count >= {} but observed {}",
            expected,
            count.load(std::sync::atomic::Ordering::SeqCst)
        );
    }

    /// Every successful transition fires `SYSTEM RELOAD DICTIONARY` exactly once.
    #[tokio::test]
    async fn test_transition_experiment_fires_dictionary_refresh_per_call() {
        let (env_id, _env_str) = env_uuid();
        let count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let refresher: Arc<dyn crate::dict_refresh::DictionaryRefresher> =
            Arc::new(RecordingRefresher {
                count: count.clone(),
            });
        let svc = ExperimentationServiceImpl::new(
            Arc::new(AlwaysSucceedRepo { env_id }),
            Arc::new(EmptyAnalyticsMock),
            Arc::new(NoScheduleRepo),
            None,
        )
        .with_dictionary_refresher(refresher);

        // draft → running
        let exp_id = ExperimentId::new();
        let req = tonic::Request::new(TransitionExperimentRequest {
            experiment_id: exp_id.to_string(),
            new_status: stitchd_proto::experiments::v1::ExperimentStatus::Active as i32,
            environment_id: String::new(),
            reason: String::new(),
        });
        svc.transition_experiment(req).await.expect("ok");
        await_count(&count, 1).await;

        // running → paused
        let req = tonic::Request::new(TransitionExperimentRequest {
            experiment_id: exp_id.to_string(),
            new_status: stitchd_proto::experiments::v1::ExperimentStatus::Paused as i32,
            environment_id: String::new(),
            reason: String::new(),
        });
        svc.transition_experiment(req).await.expect("ok");
        await_count(&count, 2).await;

        // paused → stopped
        let req = tonic::Request::new(TransitionExperimentRequest {
            experiment_id: exp_id.to_string(),
            new_status: stitchd_proto::experiments::v1::ExperimentStatus::Concluded as i32,
            environment_id: String::new(),
            reason: String::new(),
        });
        svc.transition_experiment(req).await.expect("ok");
        await_count(&count, 3).await;

        // stopped → running (restart)
        let req = tonic::Request::new(TransitionExperimentRequest {
            experiment_id: exp_id.to_string(),
            new_status: stitchd_proto::experiments::v1::ExperimentStatus::Active as i32,
            environment_id: String::new(),
            reason: String::new(),
        });
        svc.transition_experiment(req).await.expect("ok");
        await_count(&count, 4).await;
    }

    // -----------------------------------------------------------------------
    // ListExposures handler tests (Phase 7 Task 2)
    // -----------------------------------------------------------------------

    use crate::exposure_reader::{ExposureReader, ExposureRow as CoreExposureRow};

    /// Test reader returning canned rows.
    struct CannedExposureReader {
        rows: Vec<CoreExposureRow>,
        total: u64,
    }

    #[async_trait]
    impl ExposureReader for CannedExposureReader {
        async fn list_exposures(
            &self,
            _experiment_id: uuid::Uuid,
            _context_type: &str,
            _offset: u64,
            _limit: u64,
        ) -> Result<(Vec<CoreExposureRow>, u64), clickhouse::error::Error> {
            Ok((self.rows.clone(), self.total))
        }
    }

    #[tokio::test]
    async fn test_list_exposures_returns_unimplemented_without_reader() {
        let (env_id, _) = env_uuid();
        let svc = make_service(env_id);
        let req = tonic::Request::new(stitchd_proto::experiments::v1::ListExposuresRequest {
            experiment_id: uuid::Uuid::new_v4().to_string(),
            context_type: "user".to_string(),
            offset: 0,
            limit: 50,
        });
        let err = svc.list_exposures(req).await.unwrap_err();
        assert_eq!(err.code(), tonic::Code::Unimplemented);
    }

    #[tokio::test]
    async fn test_list_exposures_invalid_uuid_returns_invalid_argument() {
        let (env_id, _) = env_uuid();
        let svc = make_service(env_id).with_exposure_reader(Arc::new(CannedExposureReader {
            rows: vec![],
            total: 0,
        }));
        let req = tonic::Request::new(stitchd_proto::experiments::v1::ListExposuresRequest {
            experiment_id: "not-a-uuid".to_string(),
            context_type: "user".to_string(),
            offset: 0,
            limit: 50,
        });
        let err = svc.list_exposures(req).await.unwrap_err();
        assert_eq!(err.code(), tonic::Code::InvalidArgument);
    }

    #[tokio::test]
    async fn test_list_exposures_empty_context_type_returns_invalid_argument() {
        let (env_id, _) = env_uuid();
        let svc = make_service(env_id).with_exposure_reader(Arc::new(CannedExposureReader {
            rows: vec![],
            total: 0,
        }));
        let req = tonic::Request::new(stitchd_proto::experiments::v1::ListExposuresRequest {
            experiment_id: uuid::Uuid::new_v4().to_string(),
            context_type: String::new(),
            offset: 0,
            limit: 50,
        });
        let err = svc.list_exposures(req).await.unwrap_err();
        assert_eq!(err.code(), tonic::Code::InvalidArgument);
    }

    #[tokio::test]
    async fn test_list_exposures_round_trips_rows_and_total() {
        let (env_id, _) = env_uuid();
        let rule_id = uuid::Uuid::new_v4();
        let now = chrono::Utc::now();
        let row = CoreExposureRow {
            context_type: "user".to_string(),
            context_key: "alice".to_string(),
            variant_key: "treatment".to_string(),
            assigned_at: now,
            matched_rule_id: Some(rule_id),
        };
        let svc = make_service(env_id).with_exposure_reader(Arc::new(CannedExposureReader {
            rows: vec![row],
            total: 7,
        }));
        let req = tonic::Request::new(stitchd_proto::experiments::v1::ListExposuresRequest {
            experiment_id: uuid::Uuid::new_v4().to_string(),
            context_type: "user".to_string(),
            offset: 0,
            limit: 10,
        });
        let resp = svc.list_exposures(req).await.unwrap().into_inner();
        assert_eq!(resp.total, 7);
        assert_eq!(resp.exposures.len(), 1);
        let proto_row = &resp.exposures[0];
        assert_eq!(proto_row.context_key, "alice");
        assert_eq!(proto_row.variant_key, "treatment");
        assert_eq!(proto_row.matched_rule_id, rule_id.to_string());
        assert!(!proto_row.assigned_at.is_empty());
    }

    #[tokio::test]
    async fn test_list_exposures_default_rule_emits_empty_matched_rule_id() {
        let (env_id, _) = env_uuid();
        let row = CoreExposureRow {
            context_type: "user".to_string(),
            context_key: "bob".to_string(),
            variant_key: "control".to_string(),
            assigned_at: chrono::Utc::now(),
            matched_rule_id: None,
        };
        let svc = make_service(env_id).with_exposure_reader(Arc::new(CannedExposureReader {
            rows: vec![row],
            total: 1,
        }));
        let req = tonic::Request::new(stitchd_proto::experiments::v1::ListExposuresRequest {
            experiment_id: uuid::Uuid::new_v4().to_string(),
            context_type: "user".to_string(),
            offset: 0,
            limit: 10,
        });
        let resp = svc.list_exposures(req).await.unwrap().into_inner();
        assert_eq!(resp.exposures.len(), 1);
        assert_eq!(resp.exposures[0].matched_rule_id, "");
    }

    // -----------------------------------------------------------------------
    // GetExperimentInteractions handler tests (Phase 6 Task 2)
    // -----------------------------------------------------------------------

    use crate::interactions_reader::{
        InteractionRow as CoreInteractionRow, InteractionsReader,
    };

    /// Test reader returning canned interaction rows.
    struct CannedInteractionsReader {
        rows: Vec<CoreInteractionRow>,
    }

    #[async_trait]
    impl InteractionsReader for CannedInteractionsReader {
        async fn list_interactions(
            &self,
            _env_id: uuid::Uuid,
            _experiment_id: uuid::Uuid,
        ) -> Result<Vec<CoreInteractionRow>, clickhouse::error::Error> {
            Ok(self.rows.clone())
        }
    }

    #[tokio::test]
    async fn test_get_interactions_returns_unimplemented_without_reader() {
        let (env_id, env_str) = env_uuid();
        let svc = make_service(env_id);
        let req = tonic::Request::new(
            stitchd_proto::experiments::v1::GetExperimentInteractionsRequest {
                env_id: env_str,
                experiment_id: uuid::Uuid::new_v4().to_string(),
            },
        );
        let err = svc.get_experiment_interactions(req).await.unwrap_err();
        assert_eq!(err.code(), tonic::Code::Unimplemented);
    }

    #[tokio::test]
    async fn test_get_interactions_invalid_uuid_returns_invalid_argument() {
        let (env_id, env_str) = env_uuid();
        let svc = make_service(env_id)
            .with_interactions_reader(Arc::new(CannedInteractionsReader { rows: vec![] }));
        let req = tonic::Request::new(
            stitchd_proto::experiments::v1::GetExperimentInteractionsRequest {
                env_id: env_str,
                experiment_id: "not-a-uuid".to_string(),
            },
        );
        let err = svc.get_experiment_interactions(req).await.unwrap_err();
        assert_eq!(err.code(), tonic::Code::InvalidArgument);
    }

    #[tokio::test]
    async fn test_get_interactions_empty_when_no_rows() {
        let (env_id, env_str) = env_uuid();
        let svc = make_service(env_id)
            .with_interactions_reader(Arc::new(CannedInteractionsReader { rows: vec![] }));
        let req = tonic::Request::new(
            stitchd_proto::experiments::v1::GetExperimentInteractionsRequest {
                env_id: env_str,
                experiment_id: uuid::Uuid::new_v4().to_string(),
            },
        );
        let resp = svc.get_experiment_interactions(req).await.unwrap().into_inner();
        assert!(resp.interactions.is_empty());
    }

    #[tokio::test]
    async fn test_get_interactions_maps_rows_and_resolves_other_name() {
        let (env_id, env_str) = env_uuid();
        let this_exp = uuid::Uuid::new_v4();
        let other_exp = uuid::Uuid::new_v4();
        // Row where `this_exp` is side A → "other" is side B.
        let row_a = CoreInteractionRow {
            experiment_id_a: this_exp,
            experiment_id_b: other_exp,
            context_type: "user".into(),
            metric_key: "checkout".into(),
            shared_count: 400,
            interaction_estimate: 0.42,
            p_value: 0.001,
            significant: true,
            insufficient_data: false,
        };
        // Row where `this_exp` is side B → "other" is side A. This pair lacked
        // enough shared exposures: insufficient_data=true, significant=false.
        let third_exp = uuid::Uuid::new_v4();
        let row_b = CoreInteractionRow {
            experiment_id_a: third_exp,
            experiment_id_b: this_exp,
            context_type: "account".into(),
            metric_key: "revenue".into(),
            shared_count: 50,
            interaction_estimate: 0.0,
            p_value: 0.0,
            significant: false,
            insufficient_data: true,
        };
        let svc = make_service(env_id).with_interactions_reader(Arc::new(
            CannedInteractionsReader {
                rows: vec![row_a, row_b],
            },
        ));
        let req = tonic::Request::new(
            stitchd_proto::experiments::v1::GetExperimentInteractionsRequest {
                env_id: env_str,
                experiment_id: this_exp.to_string(),
            },
        );
        let resp = svc.get_experiment_interactions(req).await.unwrap().into_inner();
        assert_eq!(resp.interactions.len(), 2);

        let first = &resp.interactions[0];
        assert_eq!(first.experiment_id_a, this_exp.to_string());
        assert_eq!(first.experiment_id_b, other_exp.to_string());
        // AlwaysSucceedRepo names every experiment "Test Experiment".
        assert_eq!(first.other_experiment_name, "Test Experiment");
        assert_eq!(first.context_type, "user");
        assert_eq!(first.metric_key, "checkout");
        assert_eq!(first.shared_count, 400);
        assert!(first.significant);
        assert!(!first.insufficient_data);

        let second = &resp.interactions[1];
        // this_exp is side B here; the "other" resolved is side A (third_exp).
        assert_eq!(second.experiment_id_a, third_exp.to_string());
        assert_eq!(second.experiment_id_b, this_exp.to_string());
        assert_eq!(second.other_experiment_name, "Test Experiment");
        assert!(!second.significant);
        assert!(second.insufficient_data);
    }

    /// When no refresher is attached, transitions still succeed.
    #[tokio::test]
    async fn test_transition_experiment_without_refresher_is_noop() {
        let (env_id, _) = env_uuid();
        let svc = make_service(env_id);
        let req = tonic::Request::new(TransitionExperimentRequest {
            experiment_id: ExperimentId::new().to_string(),
            new_status: stitchd_proto::experiments::v1::ExperimentStatus::Active as i32,
            environment_id: String::new(),
            reason: String::new(),
        });
        let result = svc.transition_experiment(req).await;
        assert!(result.is_ok());
    }

    // -----------------------------------------------------------------------
    // validate_experiment_binding unit tests (GL-08)
    // -----------------------------------------------------------------------

    /// Analytics mock that returns a configurable list of context types.
    struct ContextTypeMock {
        types: Vec<String>,
    }

    #[async_trait]
    impl crate::analytics_client::AnalyticsResultsPort for ContextTypeMock {
        async fn list_experiment_results(
            &self,
            _env_id: &str,
            _experiment_id: &str,
            _iteration_id: Option<&str>,
        ) -> Result<Vec<ProtoExperimentResult>, tonic::Status> {
            Ok(vec![])
        }

        async fn list_context_types(
            &self,
            _environment_id: &str,
        ) -> Result<Vec<String>, tonic::Status> {
            Ok(self.types.clone())
        }
    }

    #[tokio::test]
    async fn binding_validation_rejects_xor_violation_both_set() {
        let analytics = ContextTypeMock {
            types: vec!["user".to_string()],
        };
        let inputs = BindingInputs {
            environment_id: "env-1",
            flag_key: Some("flag-1"),
            flag_rule_id: Some("rule-1"),
            targets_default_rule: true,
            unit_context_types: &["user".to_string()],
        };
        let err = validate_experiment_binding(&inputs, &analytics, None)
            .await
            .expect_err("expected XOR violation error");
        assert_eq!(err.code(), tonic::Code::InvalidArgument);
        assert!(err.message().contains("exactly one"));
    }

    #[tokio::test]
    async fn binding_validation_rejects_xor_violation_neither_set() {
        let analytics = ContextTypeMock {
            types: vec!["user".to_string()],
        };
        let inputs = BindingInputs {
            environment_id: "env-1",
            flag_key: None,
            flag_rule_id: None,
            targets_default_rule: false,
            unit_context_types: &["user".to_string()],
        };
        let err = validate_experiment_binding(&inputs, &analytics, None)
            .await
            .expect_err("expected XOR violation (neither set)");
        assert_eq!(err.code(), tonic::Code::InvalidArgument);
        assert!(err.message().contains("flag_rule_id"));
    }

    #[tokio::test]
    async fn binding_validation_rejects_empty_unit_context_types() {
        let analytics = ContextTypeMock {
            types: vec!["user".to_string()],
        };
        let inputs = BindingInputs {
            environment_id: "env-1",
            flag_key: Some("flag-1"),
            flag_rule_id: Some("rule-1"),
            targets_default_rule: false,
            unit_context_types: &[],
        };
        let err = validate_experiment_binding(&inputs, &analytics, None)
            .await
            .expect_err("expected empty unit_context_types error");
        assert_eq!(err.code(), tonic::Code::InvalidArgument);
        assert!(err.message().contains("unit_context_types"));
    }

    #[tokio::test]
    async fn binding_validation_rejects_unknown_context_type() {
        let analytics = ContextTypeMock {
            types: vec!["user".to_string(), "account".to_string()],
        };
        let inputs = BindingInputs {
            environment_id: "env-1",
            flag_key: Some("flag-1"),
            flag_rule_id: Some("rule-1"),
            targets_default_rule: false,
            unit_context_types: &["device".to_string()],
        };
        let err = validate_experiment_binding(&inputs, &analytics, None)
            .await
            .expect_err("expected unknown context_type error");
        assert_eq!(err.code(), tonic::Code::InvalidArgument);
        assert!(err.message().contains("device"));
    }

    #[tokio::test]
    async fn binding_validation_rejects_missing_flag_key() {
        let analytics = ContextTypeMock {
            types: vec!["user".to_string()],
        };
        let inputs = BindingInputs {
            environment_id: "env-1",
            flag_key: None,
            flag_rule_id: Some("rule-1"),
            targets_default_rule: false,
            unit_context_types: &["user".to_string()],
        };
        // With flag_rule_id set but flag_key missing, targets_default_rule is false
        // so XOR is satisfied. But flag_key is required.
        let err = validate_experiment_binding(&inputs, &analytics, None)
            .await
            .expect_err("expected missing flag_key error");
        assert_eq!(err.code(), tonic::Code::InvalidArgument);
        assert!(err.message().contains("flag_key"));
    }

    #[tokio::test]
    async fn binding_validation_accepts_default_rule_with_no_flag_client() {
        // When flag_client is None, flag validation is skipped.
        let analytics = ContextTypeMock {
            types: vec!["user".to_string()],
        };
        let inputs = BindingInputs {
            environment_id: "env-1",
            flag_key: Some("flag-1"),
            flag_rule_id: None,
            targets_default_rule: true,
            unit_context_types: &["user".to_string()],
        };
        let flag_id = validate_experiment_binding(&inputs, &analytics, None)
            .await
            .expect("expected valid binding to pass");
        // No flag client → empty flag_id returned.
        assert!(flag_id.is_empty());
    }

    // -----------------------------------------------------------------------
    // Exclusion-group RPC tests (P3.T2/T3)
    // -----------------------------------------------------------------------

    use std::sync::Mutex;
    use stitchd_core::experimentation::ExclusionGroup;
    use stitchd_core::evaluation::exclusion::BucketRange;
    use stitchd_db::repository::pg::ExclusionGroupRepository;

    /// In-memory exclusion-group repo for service tests. Records allocate/free
    /// calls so tests can assert lifecycle behaviour.
    #[derive(Default)]
    struct MockGroupRepo {
        salt: String,
        allocated: Mutex<Vec<ExperimentId>>,
        freed: Mutex<Vec<ExperimentId>>,
    }

    fn sample_group(salt: &str) -> ExclusionGroup {
        ExclusionGroup {
            id: ExclusionGroupId::new(),
            environment_id: EnvironmentId::new(),
            name: "g".to_string(),
            description: None,
            salt: salt.to_string(),
            unit_context_type: "user".to_string(),
            allocated_bp: 2500,
            free_bp: 7500,
            version: 1,
        }
    }

    #[async_trait]
    impl ExclusionGroupRepository for MockGroupRepo {
        async fn find_by_id(
            &self,
            id: ExclusionGroupId,
        ) -> Result<ExclusionGroup, RepositoryError> {
            let mut g = sample_group(&self.salt);
            g.id = id;
            Ok(g)
        }
        async fn list_by_environment(
            &self,
            _env_id: EnvironmentId,
        ) -> Result<Vec<ExclusionGroup>, RepositoryError> {
            Ok(vec![sample_group(&self.salt)])
        }
        async fn create(
            &self,
            env_id: EnvironmentId,
            name: &str,
            description: Option<&str>,
            unit_context_type: &str,
        ) -> Result<ExclusionGroup, RepositoryError> {
            Ok(ExclusionGroup {
                id: ExclusionGroupId::new(),
                environment_id: env_id,
                name: name.to_string(),
                description: description.map(ToString::to_string),
                salt: self.salt.clone(),
                unit_context_type: unit_context_type.to_string(),
                allocated_bp: 0,
                free_bp: 10_000,
                version: 1,
            })
        }
        async fn update(
            &self,
            id: ExclusionGroupId,
            name: &str,
            description: Option<&str>,
            expected_version: i64,
        ) -> Result<ExclusionGroup, RepositoryError> {
            Ok(ExclusionGroup {
                id,
                environment_id: EnvironmentId::new(),
                name: name.to_string(),
                description: description.map(ToString::to_string),
                salt: self.salt.clone(),
                unit_context_type: "user".to_string(),
                allocated_bp: 0,
                free_bp: 10_000,
                version: expected_version + 1,
            })
        }
        async fn soft_delete(&self, _id: ExclusionGroupId) -> Result<(), RepositoryError> {
            Ok(())
        }
        async fn allocate_range(
            &self,
            _group_id: ExclusionGroupId,
            experiment_id: ExperimentId,
            requested_bp: u32,
        ) -> Result<BucketRange, RepositoryError> {
            self.allocated.lock().unwrap().push(experiment_id);
            #[allow(clippy::cast_possible_truncation)]
            Ok(BucketRange {
                lo: 0,
                hi: requested_bp as u16,
            })
        }
        async fn free_range(&self, experiment_id: ExperimentId) -> Result<(), RepositoryError> {
            self.freed.lock().unwrap().push(experiment_id);
            Ok(())
        }
        async fn allocated_free_bp(
            &self,
            _group_id: ExclusionGroupId,
        ) -> Result<(u32, u32), RepositoryError> {
            Ok((2500, 7500))
        }
    }

    /// A group repo whose allocate always rejects with a capacity error.
    struct FullGroupRepo;

    #[async_trait]
    impl ExclusionGroupRepository for FullGroupRepo {
        async fn find_by_id(
            &self,
            id: ExclusionGroupId,
        ) -> Result<ExclusionGroup, RepositoryError> {
            let mut g = sample_group("salt");
            g.id = id;
            Ok(g)
        }
        async fn list_by_environment(
            &self,
            _env_id: EnvironmentId,
        ) -> Result<Vec<ExclusionGroup>, RepositoryError> {
            Ok(vec![])
        }
        async fn create(
            &self,
            _env_id: EnvironmentId,
            _name: &str,
            _description: Option<&str>,
            _unit_context_type: &str,
        ) -> Result<ExclusionGroup, RepositoryError> {
            unreachable!()
        }
        async fn update(
            &self,
            _id: ExclusionGroupId,
            _name: &str,
            _description: Option<&str>,
            _expected_version: i64,
        ) -> Result<ExclusionGroup, RepositoryError> {
            unreachable!()
        }
        async fn soft_delete(&self, _id: ExclusionGroupId) -> Result<(), RepositoryError> {
            Ok(())
        }
        async fn allocate_range(
            &self,
            _group_id: ExclusionGroupId,
            _experiment_id: ExperimentId,
            _requested_bp: u32,
        ) -> Result<BucketRange, RepositoryError> {
            Err(RepositoryError::InvalidState {
                reason: "no contiguous free window".to_string(),
            })
        }
        async fn free_range(&self, _experiment_id: ExperimentId) -> Result<(), RepositoryError> {
            Ok(())
        }
        async fn allocated_free_bp(
            &self,
            _group_id: ExclusionGroupId,
        ) -> Result<(u32, u32), RepositoryError> {
            Ok((10_000, 0))
        }
    }

    /// In-memory rule-gate writer recording the last gate written per rule.
    #[derive(Default)]
    struct MockGateWriter {
        sets: Mutex<Vec<(RuleId, Option<ExclusionGate>)>>,
    }

    #[async_trait]
    impl RuleGateWriter for MockGateWriter {
        async fn set_rule_exclusion_gate(
            &self,
            flag_rule_id: RuleId,
            gate: Option<ExclusionGate>,
        ) -> Result<(), RepositoryError> {
            self.sets.lock().unwrap().push((flag_rule_id, gate));
            Ok(())
        }
    }

    /// Experiment repo returning a single experiment with a fixed status.
    struct StatusRepo {
        env_id: EnvironmentId,
        status: ExperimentStatus,
    }

    #[async_trait]
    impl ExperimentRepository for StatusRepo {
        async fn find_by_id(&self, id: ExperimentId) -> Result<Experiment, RepositoryError> {
            let mut exp = make_experiment(self.env_id);
            exp.id = id;
            exp.status = self.status;
            Ok(exp)
        }
        async fn list_by_environment(
            &self,
            _env_id: EnvironmentId,
            _status_filter: Option<ExperimentStatus>,
        ) -> Result<Vec<Experiment>, RepositoryError> {
            Ok(vec![])
        }
        async fn list_by_environment_paginated(
            &self,
            _env_id: EnvironmentId,
            _offset: u64,
            _limit: u64,
        ) -> Result<(Vec<Experiment>, u64), RepositoryError> {
            Ok((vec![], 0))
        }
        async fn create(&self, _experiment: &Experiment) -> Result<(), RepositoryError> {
            Ok(())
        }
        async fn update(&self, experiment: &Experiment) -> Result<Experiment, RepositoryError> {
            Ok(experiment.clone())
        }
        async fn soft_delete(&self, _id: ExperimentId) -> Result<(), RepositoryError> {
            Ok(())
        }
        async fn list_iterations(
            &self,
            _experiment_id: ExperimentId,
        ) -> Result<Vec<ExperimentIteration>, RepositoryError> {
            Ok(vec![])
        }
        async fn apply_transition(
            &self,
            id: ExperimentId,
            to: ExperimentStatus,
            _actor_id: Option<stitchd_core::id::UserId>,
        ) -> Result<Experiment, RepositoryError> {
            let mut exp = make_experiment(self.env_id);
            exp.id = id;
            exp.status = to;
            Ok(exp)
        }
        async fn list_all_running(&self) -> Result<Vec<Experiment>, RepositoryError> {
            Ok(vec![])
        }
        async fn find_active_experiment_for_flag(
            &self,
            _flag_id: stitchd_core::id::FlagId,
        ) -> Result<Option<ExperimentId>, RepositoryError> {
            Ok(None)
        }
        async fn find_iteration_by_id(
            &self,
            iteration_id: stitchd_core::id::ExperimentIterationId,
        ) -> Result<ExperimentIteration, RepositoryError> {
            Err(RepositoryError::NotFound {
                id: iteration_id.to_string(),
            })
        }
    }

    #[tokio::test]
    async fn test_create_exclusion_group_unimplemented_without_repo() {
        let (env_id, env_str) = env_uuid();
        let svc = make_service(env_id);
        let req = tonic::Request::new(
            stitchd_proto::experiments::v1::CreateExclusionGroupRequest {
                group: Some(stitchd_proto::experiments::v1::ExclusionGroup {
                    id: String::new(),
                    env_id: env_str,
                    name: "g".to_string(),
                    description: String::new(),
                    allocated_bp: 0,
                    free_bp: 0,
                    version: 0,
                    unit_context_type: String::new(),
                }),
            },
        );
        let err = svc.create_exclusion_group(req).await.unwrap_err();
        assert_eq!(err.code(), tonic::Code::Unimplemented);
    }

    #[tokio::test]
    async fn test_create_and_list_exclusion_group() {
        let (env_id, env_str) = env_uuid();
        let svc = make_service(env_id).with_exclusion_groups(
            Arc::new(MockGroupRepo::default()),
            Arc::new(MockGateWriter::default()),
        );

        let req = tonic::Request::new(
            stitchd_proto::experiments::v1::CreateExclusionGroupRequest {
                group: Some(stitchd_proto::experiments::v1::ExclusionGroup {
                    id: String::new(),
                    env_id: env_str.clone(),
                    name: "Checkout".to_string(),
                    description: "desc".to_string(),
                    allocated_bp: 0,
                    free_bp: 0,
                    version: 0,
                    unit_context_type: "user".to_string(),
                }),
            },
        );
        let created = svc.create_exclusion_group(req).await.unwrap().into_inner();
        assert_eq!(created.name, "Checkout");
        assert_eq!(created.free_bp, 10_000);

        let list_req = tonic::Request::new(
            stitchd_proto::experiments::v1::ListExclusionGroupsRequest {
                env_id: env_str,
                page: 0,
                per_page: 0,
            },
        );
        let listed = svc.list_exclusion_groups(list_req).await.unwrap().into_inner();
        assert_eq!(listed.total, 1);
    }

    #[tokio::test]
    async fn test_create_exclusion_group_empty_name_rejected() {
        let (env_id, env_str) = env_uuid();
        let svc = make_service(env_id).with_exclusion_groups(
            Arc::new(MockGroupRepo::default()),
            Arc::new(MockGateWriter::default()),
        );
        let req = tonic::Request::new(
            stitchd_proto::experiments::v1::CreateExclusionGroupRequest {
                group: Some(stitchd_proto::experiments::v1::ExclusionGroup {
                    id: String::new(),
                    env_id: env_str,
                    name: "   ".to_string(),
                    description: String::new(),
                    allocated_bp: 0,
                    free_bp: 0,
                    version: 0,
                    unit_context_type: String::new(),
                }),
            },
        );
        let err = svc.create_exclusion_group(req).await.unwrap_err();
        assert_eq!(err.code(), tonic::Code::InvalidArgument);
    }

    #[tokio::test]
    async fn test_assign_experiment_to_group_sets_gate() {
        let (env_id, env_str) = env_uuid();
        let group_repo = Arc::new(MockGroupRepo {
            salt: "the-salt".to_string(),
            ..Default::default()
        });
        let gate_writer = Arc::new(MockGateWriter::default());
        // Draft experiment (AlwaysSucceedRepo) → flag not locked, has flag_rule_id.
        let svc = make_service(env_id)
            .with_exclusion_groups(group_repo.clone(), gate_writer.clone());

        let req = tonic::Request::new(
            stitchd_proto::experiments::v1::AssignExperimentToGroupRequest {
                env_id: env_str,
                group_id: ExclusionGroupId::new().to_string(),
                experiment_id: ExperimentId::new().to_string(),
                requested_bp: 2500,
            },
        );
        let resp = svc.assign_experiment_to_group(req).await.unwrap().into_inner();
        let exp = resp.experiment.unwrap();
        assert_eq!(exp.group_bucket_lo, Some(0));
        assert_eq!(exp.group_bucket_hi, Some(2500));
        assert!(exp.exclusion_group_id.is_some());

        // A gate was written with the group's salt.
        let sets = gate_writer.sets.lock().unwrap();
        assert_eq!(sets.len(), 1);
        let gate = sets[0].1.as_ref().expect("gate should be Some");
        assert_eq!(gate.group_salt, "the-salt");
        assert_eq!(gate.context_type, "user");
        assert_eq!((gate.bucket_lo, gate.bucket_hi), (0, 2500));
        // Range was allocated, not freed.
        assert_eq!(group_repo.allocated.lock().unwrap().len(), 1);
        assert!(group_repo.freed.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_assign_rejected_when_flag_locked() {
        let (env_id, env_str) = env_uuid();
        let svc = ExperimentationServiceImpl::new(
            Arc::new(StatusRepo {
                env_id,
                status: ExperimentStatus::Running,
            }),
            Arc::new(EmptyAnalyticsMock),
            Arc::new(NoScheduleRepo),
            None,
        )
        .with_exclusion_groups(
            Arc::new(MockGroupRepo::default()),
            Arc::new(MockGateWriter::default()),
        );

        let req = tonic::Request::new(
            stitchd_proto::experiments::v1::AssignExperimentToGroupRequest {
                env_id: env_str,
                group_id: ExclusionGroupId::new().to_string(),
                experiment_id: ExperimentId::new().to_string(),
                requested_bp: 2500,
            },
        );
        let err = svc.assign_experiment_to_group(req).await.unwrap_err();
        assert_eq!(err.code(), tonic::Code::FailedPrecondition);
        assert!(
            err.message().starts_with("flag_locked_by_experiment:"),
            "expected flag-lock sentinel, got {:?}",
            err.message()
        );
    }

    #[tokio::test]
    async fn test_assign_capacity_overflow_failed_precondition() {
        let (env_id, env_str) = env_uuid();
        let svc = make_service(env_id)
            .with_exclusion_groups(Arc::new(FullGroupRepo), Arc::new(MockGateWriter::default()));

        let req = tonic::Request::new(
            stitchd_proto::experiments::v1::AssignExperimentToGroupRequest {
                env_id: env_str,
                group_id: ExclusionGroupId::new().to_string(),
                experiment_id: ExperimentId::new().to_string(),
                requested_bp: 6000,
            },
        );
        let err = svc.assign_experiment_to_group(req).await.unwrap_err();
        assert_eq!(err.code(), tonic::Code::FailedPrecondition);
    }

    /// A group repo whose group randomizes on the `account` unit. Used to verify
    /// the cross-member diversion-unit check and gate construction.
    #[derive(Default)]
    struct AccountUnitGroupRepo {
        salt: String,
    }

    #[async_trait]
    impl ExclusionGroupRepository for AccountUnitGroupRepo {
        async fn find_by_id(
            &self,
            id: ExclusionGroupId,
        ) -> Result<ExclusionGroup, RepositoryError> {
            let mut g = sample_group(&self.salt);
            g.id = id;
            g.unit_context_type = "account".to_string();
            Ok(g)
        }
        async fn list_by_environment(
            &self,
            _env_id: EnvironmentId,
        ) -> Result<Vec<ExclusionGroup>, RepositoryError> {
            Ok(vec![])
        }
        async fn create(
            &self,
            _env_id: EnvironmentId,
            _name: &str,
            _description: Option<&str>,
            _unit_context_type: &str,
        ) -> Result<ExclusionGroup, RepositoryError> {
            unreachable!()
        }
        async fn update(
            &self,
            _id: ExclusionGroupId,
            _name: &str,
            _description: Option<&str>,
            _expected_version: i64,
        ) -> Result<ExclusionGroup, RepositoryError> {
            unreachable!()
        }
        async fn soft_delete(&self, _id: ExclusionGroupId) -> Result<(), RepositoryError> {
            Ok(())
        }
        async fn allocate_range(
            &self,
            _group_id: ExclusionGroupId,
            _experiment_id: ExperimentId,
            requested_bp: u32,
        ) -> Result<BucketRange, RepositoryError> {
            #[allow(clippy::cast_possible_truncation)]
            Ok(BucketRange {
                lo: 0,
                hi: requested_bp as u16,
            })
        }
        async fn free_range(&self, _experiment_id: ExperimentId) -> Result<(), RepositoryError> {
            Ok(())
        }
        async fn allocated_free_bp(
            &self,
            _group_id: ExclusionGroupId,
        ) -> Result<(u32, u32), RepositoryError> {
            Ok((2500, 7500))
        }
    }

    /// HIGH-1: assignment is rejected with FAILED_PRECONDITION when the
    /// experiment does not declare the group's diversion unit, because mutual
    /// exclusion can only hold if all members randomize on the same unit.
    #[tokio::test]
    async fn test_assign_rejected_on_unit_mismatch() {
        let (env_id, env_str) = env_uuid();
        // make_experiment → unit_context_types = ["user"], group unit = "account".
        let svc = make_service(env_id).with_exclusion_groups(
            Arc::new(AccountUnitGroupRepo::default()),
            Arc::new(MockGateWriter::default()),
        );

        let req = tonic::Request::new(
            stitchd_proto::experiments::v1::AssignExperimentToGroupRequest {
                env_id: env_str,
                group_id: ExclusionGroupId::new().to_string(),
                experiment_id: ExperimentId::new().to_string(),
                requested_bp: 2500,
            },
        );
        let err = svc.assign_experiment_to_group(req).await.unwrap_err();
        assert_eq!(err.code(), tonic::Code::FailedPrecondition);
        assert!(
            err.message().contains("account"),
            "expected unit-mismatch message, got {:?}",
            err.message()
        );
    }

    /// HIGH-1: the exclusion gate carries the GROUP's diversion unit, not the
    /// experiment's first unit. Here the group randomizes on `account` and the
    /// experiment declares both `user` and `account`, so the gate must use
    /// `account`.
    #[tokio::test]
    async fn test_assign_gate_uses_group_unit_not_experiment_first() {
        let (env_id, env_str) = env_uuid();
        let group_repo = Arc::new(AccountUnitGroupRepo {
            salt: "acct-salt".to_string(),
        });
        let gate_writer = Arc::new(MockGateWriter::default());
        // Experiment that declares the group's unit (account) in addition to user.
        struct MultiUnitRepo {
            env_id: EnvironmentId,
        }
        #[async_trait]
        impl ExperimentRepository for MultiUnitRepo {
            async fn find_by_id(&self, id: ExperimentId) -> Result<Experiment, RepositoryError> {
                let mut exp = make_experiment(self.env_id);
                exp.id = id;
                exp.unit_context_types = vec!["user".to_string(), "account".to_string()];
                Ok(exp)
            }
            async fn list_by_environment(
                &self,
                _env_id: EnvironmentId,
                _status_filter: Option<ExperimentStatus>,
            ) -> Result<Vec<Experiment>, RepositoryError> {
                Ok(vec![])
            }
            async fn list_by_environment_paginated(
                &self,
                _env_id: EnvironmentId,
                _offset: u64,
                _limit: u64,
            ) -> Result<(Vec<Experiment>, u64), RepositoryError> {
                Ok((vec![], 0))
            }
            async fn create(&self, _experiment: &Experiment) -> Result<(), RepositoryError> {
                Ok(())
            }
            async fn update(
                &self,
                experiment: &Experiment,
            ) -> Result<Experiment, RepositoryError> {
                Ok(experiment.clone())
            }
            async fn soft_delete(&self, _id: ExperimentId) -> Result<(), RepositoryError> {
                Ok(())
            }
            async fn list_iterations(
                &self,
                _experiment_id: ExperimentId,
            ) -> Result<Vec<ExperimentIteration>, RepositoryError> {
                Ok(vec![])
            }
            async fn apply_transition(
                &self,
                id: ExperimentId,
                to: ExperimentStatus,
                _actor_id: Option<stitchd_core::id::UserId>,
            ) -> Result<Experiment, RepositoryError> {
                let mut exp = make_experiment(self.env_id);
                exp.id = id;
                exp.status = to;
                Ok(exp)
            }
            async fn list_all_running(&self) -> Result<Vec<Experiment>, RepositoryError> {
                Ok(vec![])
            }
            async fn find_active_experiment_for_flag(
                &self,
                _flag_id: stitchd_core::id::FlagId,
            ) -> Result<Option<ExperimentId>, RepositoryError> {
                Ok(None)
            }
            async fn find_iteration_by_id(
                &self,
                iteration_id: stitchd_core::id::ExperimentIterationId,
            ) -> Result<ExperimentIteration, RepositoryError> {
                Err(RepositoryError::NotFound {
                    id: iteration_id.to_string(),
                })
            }
        }

        let svc = ExperimentationServiceImpl::new(
            Arc::new(MultiUnitRepo { env_id }),
            Arc::new(EmptyAnalyticsMock),
            Arc::new(NoScheduleRepo),
            None,
        )
        .with_exclusion_groups(group_repo.clone(), gate_writer.clone());

        let req = tonic::Request::new(
            stitchd_proto::experiments::v1::AssignExperimentToGroupRequest {
                env_id: env_str,
                group_id: ExclusionGroupId::new().to_string(),
                experiment_id: ExperimentId::new().to_string(),
                requested_bp: 2500,
            },
        );
        svc.assign_experiment_to_group(req).await.unwrap();

        let sets = gate_writer.sets.lock().unwrap();
        assert_eq!(sets.len(), 1);
        let gate = sets[0].1.as_ref().expect("gate should be Some");
        assert_eq!(gate.context_type, "account");
        assert_eq!(gate.group_salt, "acct-salt");
    }

    /// The new GetExclusionGroup RPC returns the group (with its diversion unit)
    /// and surfaces NOT_FOUND from the repo.
    #[tokio::test]
    async fn test_get_exclusion_group_returns_group_with_unit() {
        let (env_id, env_str) = env_uuid();
        let svc = make_service(env_id).with_exclusion_groups(
            Arc::new(MockGroupRepo::default()),
            Arc::new(MockGateWriter::default()),
        );
        let gid = ExclusionGroupId::new();
        let req = tonic::Request::new(
            stitchd_proto::experiments::v1::GetExclusionGroupRequest {
                env_id: env_str,
                group_id: gid.to_string(),
            },
        );
        let got = svc.get_exclusion_group(req).await.unwrap().into_inner();
        assert_eq!(got.id, gid.to_string());
        assert_eq!(got.unit_context_type, "user");
    }

    #[tokio::test]
    async fn test_get_exclusion_group_not_found() {
        let (env_id, env_str) = env_uuid();
        // FullGroupRepo::find_by_id returns Ok; use a repo that returns NotFound.
        struct MissingGroupRepo;
        #[async_trait]
        impl ExclusionGroupRepository for MissingGroupRepo {
            async fn find_by_id(
                &self,
                id: ExclusionGroupId,
            ) -> Result<ExclusionGroup, RepositoryError> {
                Err(RepositoryError::NotFound { id: id.to_string() })
            }
            async fn list_by_environment(
                &self,
                _env_id: EnvironmentId,
            ) -> Result<Vec<ExclusionGroup>, RepositoryError> {
                Ok(vec![])
            }
            async fn create(
                &self,
                _env_id: EnvironmentId,
                _name: &str,
                _description: Option<&str>,
                _unit_context_type: &str,
            ) -> Result<ExclusionGroup, RepositoryError> {
                unreachable!()
            }
            async fn update(
                &self,
                _id: ExclusionGroupId,
                _name: &str,
                _description: Option<&str>,
                _expected_version: i64,
            ) -> Result<ExclusionGroup, RepositoryError> {
                unreachable!()
            }
            async fn soft_delete(&self, _id: ExclusionGroupId) -> Result<(), RepositoryError> {
                Ok(())
            }
            async fn allocate_range(
                &self,
                _group_id: ExclusionGroupId,
                _experiment_id: ExperimentId,
                _requested_bp: u32,
            ) -> Result<BucketRange, RepositoryError> {
                unreachable!()
            }
            async fn free_range(
                &self,
                _experiment_id: ExperimentId,
            ) -> Result<(), RepositoryError> {
                Ok(())
            }
            async fn allocated_free_bp(
                &self,
                _group_id: ExclusionGroupId,
            ) -> Result<(u32, u32), RepositoryError> {
                Ok((0, 10_000))
            }
        }

        let svc = make_service(env_id).with_exclusion_groups(
            Arc::new(MissingGroupRepo),
            Arc::new(MockGateWriter::default()),
        );
        let req = tonic::Request::new(
            stitchd_proto::experiments::v1::GetExclusionGroupRequest {
                env_id: env_str,
                group_id: ExclusionGroupId::new().to_string(),
            },
        );
        let err = svc.get_exclusion_group(req).await.unwrap_err();
        assert_eq!(err.code(), tonic::Code::NotFound);
    }

    #[tokio::test]
    async fn test_unassign_clears_gate_and_frees_range() {
        let (env_id, _) = env_uuid();
        let group_repo = Arc::new(MockGroupRepo::default());
        let gate_writer = Arc::new(MockGateWriter::default());
        let svc = make_service(env_id)
            .with_exclusion_groups(group_repo.clone(), gate_writer.clone());

        let exp_id = ExperimentId::new();
        let req = tonic::Request::new(
            stitchd_proto::experiments::v1::UnassignExperimentRequest {
                env_id: env_id.to_string(),
                experiment_id: exp_id.to_string(),
            },
        );
        let resp = svc.unassign_experiment(req).await.unwrap().into_inner();
        let exp = resp.experiment.unwrap();
        assert!(exp.exclusion_group_id.is_none());

        assert_eq!(group_repo.freed.lock().unwrap().len(), 1);
        let sets = gate_writer.sets.lock().unwrap();
        assert_eq!(sets.len(), 1);
        assert!(sets[0].1.is_none(), "gate should be cleared (None)");
    }

    #[tokio::test]
    async fn test_unassign_rejected_when_flag_locked() {
        let (env_id, _) = env_uuid();
        let svc = ExperimentationServiceImpl::new(
            Arc::new(StatusRepo {
                env_id,
                status: ExperimentStatus::Paused,
            }),
            Arc::new(EmptyAnalyticsMock),
            Arc::new(NoScheduleRepo),
            None,
        )
        .with_exclusion_groups(
            Arc::new(MockGroupRepo::default()),
            Arc::new(MockGateWriter::default()),
        );

        let req = tonic::Request::new(
            stitchd_proto::experiments::v1::UnassignExperimentRequest {
                env_id: env_id.to_string(),
                experiment_id: ExperimentId::new().to_string(),
            },
        );
        let err = svc.unassign_experiment(req).await.unwrap_err();
        assert_eq!(err.code(), tonic::Code::FailedPrecondition);
    }

    #[tokio::test]
    async fn test_transition_to_stopped_releases_group() {
        let (env_id, _) = env_uuid();
        let group_repo = Arc::new(MockGroupRepo::default());
        let gate_writer = Arc::new(MockGateWriter::default());
        let svc = make_service(env_id)
            .with_exclusion_groups(group_repo.clone(), gate_writer.clone());

        let req = tonic::Request::new(TransitionExperimentRequest {
            experiment_id: ExperimentId::new().to_string(),
            new_status: core_status_to_proto(ExperimentStatus::Stopped),
            environment_id: String::new(),
            reason: String::new(),
        });
        svc.transition_experiment(req).await.unwrap();

        // The stopped transition freed the range and cleared the gate.
        assert_eq!(group_repo.freed.lock().unwrap().len(), 1);
        let sets = gate_writer.sets.lock().unwrap();
        assert_eq!(sets.len(), 1);
        assert!(sets[0].1.is_none());
    }

    #[tokio::test]
    async fn test_get_experiment_interactions_remains_unimplemented() {
        let (env_id, _) = env_uuid();
        let svc = make_service(env_id);
        let req = tonic::Request::new(
            stitchd_proto::experiments::v1::GetExperimentInteractionsRequest {
                env_id: env_id.to_string(),
                experiment_id: ExperimentId::new().to_string(),
            },
        );
        let err = svc.get_experiment_interactions(req).await.unwrap_err();
        assert_eq!(err.code(), tonic::Code::Unimplemented);
    }
}
