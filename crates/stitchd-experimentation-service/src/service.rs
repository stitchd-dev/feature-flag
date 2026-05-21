//! gRPC service implementation for `ExperimentationService`.

use std::sync::Arc;

use async_trait::async_trait;
use chrono::Utc;
use tonic::{Request, Response, Status};
use tracing::instrument;

use stitchd_core::{
    experimentation::{Experiment, ExperimentStatus},
    id::{EnvironmentId, ExperimentId, ExperimentIterationId},
};
use stitchd_db::{ExperimentRepository, StatsScheduleRepository};
use stitchd_proto::experiments::v1::{
    CreateExperimentRequest, DeleteExperimentRequest, ExperimentIteration as ProtoIteration,
    ExperimentResults, GetExperimentIterationRequest, GetExperimentRequest, GetResultsRequest,
    ListExperimentsRequest, ListExperimentsResponse, ListIterationsRequest, ListIterationsResponse,
    ListRunningExperimentsRequest, RunningExperiment, TransitionExperimentRequest,
    UpdateExperimentRequest, UpdateIterationLastComputedRequest,
    UpdateIterationLastComputedResponse, VariantResult,
    experimentation_service_server::ExperimentationService,
};

use crate::analytics_client::AnalyticsResultsPort;
use crate::dict_refresh::{DictionaryRefresher, spawn_refresh};
use crate::flag_client::FlagClient;

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
    }
}

/// Map a core [`Experiment`] to the proto [`stitchd_proto::experiments::v1::Experiment`] message.
fn core_to_proto(e: &Experiment) -> stitchd_proto::experiments::v1::Experiment {
    stitchd_proto::experiments::v1::Experiment {
        id: e.id.to_string(),
        environment_id: e.environment_id.to_string(),
        name: e.name.clone(),
        description: e.description.clone().unwrap_or_default(),
        flag_key: String::new(), // flag_key not stored on Experiment; filled by caller if known
        status: core_status_to_proto(e.status),
        variant_keys: vec![],
        created_at_ms: e.created_at.timestamp_millis(),
        updated_at_ms: e.updated_at.timestamp_millis(),
        version: u64::try_from(e.version).unwrap_or(1),
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
        }
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
}

// ---------------------------------------------------------------------------
// gRPC trait impl
// ---------------------------------------------------------------------------

#[async_trait]
impl ExperimentationService for ExperimentationServiceImpl {
    /// Create a new experiment.
    ///
    /// If the request specifies `ACTIVE` status and a `flag_key` is provided,
    /// calls the Flag Service to verify the flag exists before creating.
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

        // Flag-lock: when activating (ACTIVE/Running), verify the flag exists.
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

        let now = Utc::now();
        // Proto layer doesn't yet carry the new attribution fields; placeholder
        // values land here. Phase 3 (Gateway API Surface) extends the proto
        // schema with flag_id/targets_default_rule/guardrails/pre_period_days/
        // unit_context_types and switches gateway validators on accordingly.
        let experiment = Experiment {
            id: ExperimentId::new(),
            environment_id: env_id,
            flag_id: stitchd_core::id::FlagId::new(),
            flag_rule_id: Some(stitchd_core::id::RuleId::new()),
            targets_default_rule: false,
            name: proto_exp.name.clone(),
            description: if proto_exp.description.is_empty() {
                None
            } else {
                Some(proto_exp.description.clone())
            },
            hypothesis: None,
            metric_ids: vec![],
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
        };

        self.experiment_repo
            .create(&experiment)
            .await
            .map_err(repo_err_to_status)?;

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
            .map_err(repo_err_to_status)?;

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
            .map_err(repo_err_to_status)?;

        let protos: Vec<_> = experiments.iter().map(core_to_proto).collect();
        metrics::counter!("experimentation_service.list_experiments.ok").increment(1);
        Ok(Response::new(ListExperimentsResponse {
            experiments: protos,
            total,
        }))
    }

    /// Update an existing experiment (name, description, variant keys).
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
            .map_err(repo_err_to_status)?;

        if !proto_exp.name.is_empty() {
            experiment.name = proto_exp.name.clone();
        }
        if !proto_exp.description.is_empty() {
            experiment.description = Some(proto_exp.description.clone());
        }
        if proto_exp.version > 0 {
            experiment.version = i64::try_from(proto_exp.version).unwrap_or(experiment.version);
        }
        experiment.updated_at = chrono::Utc::now();

        let updated = self
            .experiment_repo
            .update(&experiment)
            .await
            .map_err(repo_err_to_status)?;

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
            .map_err(repo_err_to_status)?;

        self.experiment_repo
            .soft_delete(exp_id)
            .await
            .map_err(repo_err_to_status)?;

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
            .map_err(repo_err_to_status)?;

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

    /// List all iterations for an experiment.
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
            .map_err(repo_err_to_status)?;

        metrics::counter!("experimentation_service.list_iterations.ok").increment(1);
        Ok(Response::new(ListIterationsResponse {
            iterations: iterations.iter().map(iteration_to_proto).collect(),
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

        // Aggregate streamed ExperimentResult protos into VariantResult messages.
        // variant_stats is a JSON string: {"<variant_key>": <participant_count>, ...}
        // We build one VariantResult per variant across all metric rows.
        use std::collections::HashMap;
        let mut by_variant: HashMap<String, VariantResult> = HashMap::new();
        let mut latest_computed_at_ms: i64 = 0;

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

            // variant_stats is a JSON object string.
            let variant_stats: serde_json::Value =
                serde_json::from_str(&result.variant_stats).unwrap_or(serde_json::Value::Null);

            if let Some(obj) = variant_stats.as_object() {
                for (variant_key, count_val) in obj {
                    let participant_count = count_val.as_u64().unwrap_or(0);
                    let entry =
                        by_variant
                            .entry(variant_key.clone())
                            .or_insert_with(|| VariantResult {
                                variant_key: variant_key.clone(),
                                participant_count,
                                metric_values: HashMap::new(),
                                p_value: 0.0,
                                p_value_present: false,
                            });

                    // Extract p_value from frequentist_result JSON string if present.
                    if let Some(freq_str) = &result.frequentist_result
                        && let Ok(freq_json) = serde_json::from_str::<serde_json::Value>(freq_str)
                        && let Some(p_val) = freq_json.get("p_value").and_then(|v| v.as_f64())
                    {
                        entry.p_value = p_val;
                        entry.p_value_present = true;
                    }

                    // Record participant_count as metric value.
                    entry
                        .metric_values
                        .insert(result.metric_key.clone(), participant_count as f64);
                }
            }
        }

        let variant_results: Vec<VariantResult> = by_variant.into_values().collect();

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
            .map_err(repo_err_to_status)?;

        // For each running experiment, fetch the active (un-ended) iteration.
        let mut items: Vec<Result<RunningExperiment, Status>> =
            Vec::with_capacity(experiments.len());
        for exp in experiments {
            let iterations = self
                .experiment_repo
                .list_iterations(exp.id)
                .await
                .map_err(repo_err_to_status)?;

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
            .map_err(repo_err_to_status)?;

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
}

// ---------------------------------------------------------------------------
// Error mapping
// ---------------------------------------------------------------------------

fn repo_err_to_status(e: stitchd_db::RepositoryError) -> Status {
    match e {
        stitchd_db::RepositoryError::NotFound { id } => {
            Status::not_found(format!("not found: {id}"))
        }
        stitchd_db::RepositoryError::VersionConflict { expected, actual } => Status::aborted(
            format!("version conflict: expected {expected}, actual {actual}"),
        ),
        stitchd_db::RepositoryError::UniqueViolation { field } => {
            Status::already_exists(format!("unique violation on: {field}"))
        }
        stitchd_db::RepositoryError::ForeignKeyViolation { constraint } => {
            Status::invalid_argument(format!("referenced entity does not exist: {constraint}"))
        }
        stitchd_db::RepositoryError::InvalidState { reason } => Status::failed_precondition(reason),
        stitchd_db::RepositoryError::Database(e) => Status::internal(format!("database: {e}")),
        stitchd_db::RepositoryError::Unexpected(e) => Status::internal(format!("unexpected: {e}")),
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
    // repo_err_to_status tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_repo_err_not_found_to_not_found_status() {
        let s = repo_err_to_status(RepositoryError::NotFound { id: "abc".into() });
        assert_eq!(s.code(), tonic::Code::NotFound);
        assert!(s.message().contains("abc"));
    }

    #[test]
    fn test_repo_err_version_conflict_to_aborted() {
        let s = repo_err_to_status(RepositoryError::VersionConflict {
            expected: 1,
            actual: 2,
        });
        assert_eq!(s.code(), tonic::Code::Aborted);
    }

    #[test]
    fn test_repo_err_unique_violation_to_already_exists() {
        let s = repo_err_to_status(RepositoryError::UniqueViolation {
            field: "flag_rule_id".into(),
        });
        assert_eq!(s.code(), tonic::Code::AlreadyExists);
    }

    #[test]
    fn test_repo_err_invalid_state_to_failed_precondition() {
        let s = repo_err_to_status(RepositoryError::InvalidState {
            reason: "cannot mutate running experiment".into(),
        });
        assert_eq!(s.code(), tonic::Code::FailedPrecondition);
    }

    #[test]
    fn test_repo_err_database_to_internal() {
        let s = repo_err_to_status(RepositoryError::Database(sqlx::Error::RowNotFound));
        assert_eq!(s.code(), tonic::Code::Internal);
    }

    #[test]
    fn test_repo_err_unexpected_to_internal() {
        let s = repo_err_to_status(RepositoryError::Unexpected(anyhow::anyhow!(
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
            id: String::new(),
            environment_id: "not-a-uuid".to_string(),
            name: "Test".to_string(),
            description: String::new(),
            flag_key: String::new(),
            status: 0,
            variant_keys: vec![],
            created_at_ms: 0,
            updated_at_ms: 0,
            version: 0,
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
        let proto_exp = stitchd_proto::experiments::v1::Experiment {
            id: String::new(),
            environment_id: env_id_str.clone(),
            name: "My Experiment".to_string(),
            description: "Testing".to_string(),
            flag_key: String::new(),
            status: 0,
            variant_keys: vec![],
            created_at_ms: 0,
            updated_at_ms: 0,
            version: 0,
        };
        let req = tonic::Request::new(CreateExperimentRequest {
            experiment: Some(proto_exp),
        });
        let result = svc.create_experiment(req).await;
        assert!(result.is_ok());
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
        let proto_exp = stitchd_proto::experiments::v1::Experiment {
            id: String::new(),
            environment_id: env_id.to_string(),
            name: "Fail".to_string(),
            description: String::new(),
            flag_key: String::new(),
            status: 0,
            variant_keys: vec![],
            created_at_ms: 0,
            updated_at_ms: 0,
            version: 0,
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
        // still create the experiment (flag verification skipped).
        let (env_id, env_id_str) = env_uuid();
        let svc = make_service(env_id); // flag_client = None
        let proto_exp = stitchd_proto::experiments::v1::Experiment {
            id: String::new(),
            environment_id: env_id_str.clone(),
            name: "Active Experiment".to_string(),
            description: String::new(),
            flag_key: "my-flag".to_string(),
            // ACTIVE status
            status: stitchd_proto::experiments::v1::ExperimentStatus::Active as i32,
            variant_keys: vec![],
            created_at_ms: 0,
            updated_at_ms: 0,
            version: 0,
        };
        let req = tonic::Request::new(CreateExperimentRequest {
            experiment: Some(proto_exp),
        });
        let result = svc.create_experiment(req).await;
        // No flag client = skip flag check → create succeeds
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_create_experiment_active_status_with_empty_flag_key_skips_flag_check() {
        let (env_id, env_id_str) = env_uuid();
        let svc = make_service(env_id);
        let proto_exp = stitchd_proto::experiments::v1::Experiment {
            id: String::new(),
            environment_id: env_id_str.clone(),
            name: "Active No Key".to_string(),
            description: String::new(),
            flag_key: String::new(), // empty key → skip verification
            status: stitchd_proto::experiments::v1::ExperimentStatus::Active as i32,
            variant_keys: vec![],
            created_at_ms: 0,
            updated_at_ms: 0,
            version: 0,
        };
        let req = tonic::Request::new(CreateExperimentRequest {
            experiment: Some(proto_exp),
        });
        let result = svc.create_experiment(req).await;
        assert!(result.is_ok());
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
}
