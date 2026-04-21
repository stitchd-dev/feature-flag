//! gRPC service implementation for `ExperimentationService`.

use std::sync::Arc;

use async_trait::async_trait;
use chrono::Utc;
use tonic::{Request, Response, Status};
use tracing::instrument;

use stitchd_core::{
    experimentation::{Experiment, ExperimentStatus},
    id::{EnvironmentId, ExperimentId},
};
use stitchd_db::{ExperimentRepository, ExperimentResultsRepository};
use stitchd_proto::experiments::v1::{
    CreateExperimentRequest, DeleteExperimentRequest, ExperimentIteration as ProtoIteration,
    ExperimentResults, GetExperimentRequest, GetResultsRequest, ListExperimentsRequest,
    ListExperimentsResponse, ListIterationsRequest, ListIterationsResponse,
    TransitionExperimentRequest, UpdateExperimentRequest, VariantResult,
    experimentation_service_server::ExperimentationService,
};

use crate::flag_client::FlagClient;

// ---------------------------------------------------------------------------
// Status mapping helpers
// ---------------------------------------------------------------------------

/// Map proto `ExperimentStatus` integer to core [`ExperimentStatus`].
#[allow(clippy::result_large_err)]
fn proto_status_to_core(
    status: i32,
) -> Result<ExperimentStatus, Status> {
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
        metric_keys: i.metric_keys.clone(),
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
pub struct ExperimentationServiceImpl {
    experiment_repo: Arc<dyn ExperimentRepository>,
    results_repo: Arc<dyn ExperimentResultsRepository>,
    /// Optional Flag Service client. When `None`, flag verification is skipped.
    flag_client: Option<FlagClient>,
}

impl ExperimentationServiceImpl {
    /// Construct a new service instance.
    #[must_use]
    pub fn new(
        experiment_repo: Arc<dyn ExperimentRepository>,
        results_repo: Arc<dyn ExperimentResultsRepository>,
        flag_client: Option<FlagClient>,
    ) -> Self {
        Self {
            experiment_repo,
            results_repo,
            flag_client,
        }
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
        let proto_exp = req.experiment.ok_or_else(|| {
            Status::invalid_argument("experiment field is required")
        })?;

        let target_status = proto_status_to_core(proto_exp.status)?;

        // Flag-lock: when activating (ACTIVE/Running), verify the flag exists.
        if target_status == ExperimentStatus::Running && !proto_exp.flag_key.is_empty() {
            if let Some(fc) = &self.flag_client {
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
        }

        // Parse IDs.
        let env_uuid = uuid::Uuid::parse_str(&proto_exp.environment_id).map_err(|_| {
            Status::invalid_argument("invalid environment_id UUID")
        })?;
        let env_id = EnvironmentId::from_uuid(env_uuid);

        let now = Utc::now();
        let experiment = Experiment {
            id: ExperimentId::new(),
            environment_id: env_id,
            // flag_rule_id not available in proto; use a placeholder
            flag_rule_id: stitchd_core::id::RuleId::new(),
            name: proto_exp.name.clone(),
            description: if proto_exp.description.is_empty() {
                None
            } else {
                Some(proto_exp.description.clone())
            },
            hypothesis: None,
            metric_keys: vec![],
            traffic_allocation: 100.0,
            min_sample_size: None,
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
        let exp_uuid = uuid::Uuid::parse_str(&req.experiment_id).map_err(|_| {
            Status::invalid_argument("invalid experiment_id UUID")
        })?;
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
        let env_uuid = uuid::Uuid::parse_str(&req.environment_id).map_err(|_| {
            Status::invalid_argument("invalid environment_id UUID")
        })?;
        let env_id = EnvironmentId::from_uuid(env_uuid);

        let experiments = self
            .experiment_repo
            .list_by_environment(env_id, None)
            .await
            .map_err(repo_err_to_status)?;

        let protos: Vec<_> = experiments.iter().map(core_to_proto).collect();
        metrics::counter!("experimentation_service.list_experiments.ok").increment(1);
        Ok(Response::new(ListExperimentsResponse {
            experiments: protos,
        }))
    }

    /// Update an existing experiment (name, description, variant keys).
    #[instrument(skip(self))]
    async fn update_experiment(
        &self,
        request: Request<UpdateExperimentRequest>,
    ) -> Result<Response<stitchd_proto::experiments::v1::Experiment>, Status> {
        let req = request.into_inner();
        let proto_exp = req.experiment.ok_or_else(|| {
            Status::invalid_argument("experiment field is required")
        })?;

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
    #[instrument(skip(self))]
    async fn get_results(
        &self,
        request: Request<GetResultsRequest>,
    ) -> Result<Response<ExperimentResults>, Status> {
        let req = request.into_inner();
        let exp_id_uuid = req.experiment_id.parse::<uuid::Uuid>().map_err(|_| {
            Status::invalid_argument("invalid experiment_id UUID")
        })?;

        let rows = self
            .results_repo
            .fetch_latest(exp_id_uuid)
            .await
            .map_err(|e| Status::internal(format!("database error: {e}")))?;

        // Aggregate rows into VariantResult messages keyed by variant from variant_stats.
        // variant_stats JSONB has shape: {"<variant_key>": <count>, ...}
        // We build one VariantResult per variant across all metric rows.
        use std::collections::HashMap;
        let mut by_variant: HashMap<String, VariantResult> = HashMap::new();
        let mut latest_computed_at_ms: i64 = 0;

        for row in &rows {
            let computed_ms = row.computed_at.timestamp_millis();
            if computed_ms > latest_computed_at_ms {
                latest_computed_at_ms = computed_ms;
            }

            // variant_stats is {"<variant_key>": <participant_count>}
            if let Some(obj) = row.variant_stats.as_object() {
                for (variant_key, count_val) in obj {
                    let participant_count = count_val.as_u64().unwrap_or(0);
                    let entry = by_variant.entry(variant_key.clone()).or_insert_with(|| {
                        VariantResult {
                            variant_key: variant_key.clone(),
                            participant_count,
                            metric_values: HashMap::new(),
                            p_value: 0.0,
                            p_value_present: false,
                        }
                    });

                    // Add metric value from frequentist_result if present.
                    if let Some(freq) = &row.frequentist_result {
                        if let Some(p_val) = freq.get("p_value").and_then(|v| v.as_f64()) {
                            entry.p_value = p_val;
                            entry.p_value_present = true;
                        }
                    }

                    // Record participant_count from variant_stats as metric value too.
                    entry
                        .metric_values
                        .insert(row.metric_key.clone(), participant_count as f64);
                }
            }
        }

        let variant_results: Vec<VariantResult> = by_variant.into_values().collect();

        metrics::counter!("experimentation_service.get_results.ok").increment(1);
        Ok(Response::new(ExperimentResults {
            experiment_id: req.experiment_id,
            variant_results,
            computed_at_ms: latest_computed_at_ms,
        }))
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
        stitchd_db::RepositoryError::VersionConflict { expected, actual } => {
            Status::aborted(format!("version conflict: expected {expected}, actual {actual}"))
        }
        stitchd_db::RepositoryError::UniqueViolation { field } => {
            Status::already_exists(format!("unique violation on: {field}"))
        }
        stitchd_db::RepositoryError::InvalidState { reason } => {
            Status::failed_precondition(reason)
        }
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
        id::{EnvironmentId, ExperimentId, RuleId},
    };
    use stitchd_db::{ExperimentResultsRepository, RepositoryError};
    use stitchd_db::experiment_results::{ExperimentResultRow, UpsertResultRow};
    use uuid::Uuid;

    // -----------------------------------------------------------------------
    // Stub repositories
    // -----------------------------------------------------------------------

    fn make_experiment(env_id: EnvironmentId) -> Experiment {
        let now = Utc::now();
        Experiment {
            id: ExperimentId::new(),
            environment_id: env_id,
            flag_rule_id: RuleId::new(),
            name: "Test Experiment".to_string(),
            description: Some("A description".to_string()),
            hypothesis: None,
            metric_keys: vec!["checkout".to_string()],
            traffic_allocation: 100.0,
            min_sample_size: None,
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
    }

    struct EmptyResultsRepo;

    #[async_trait]
    impl ExperimentResultsRepository for EmptyResultsRepo {
        async fn upsert(
            &self,
            _row: &UpsertResultRow,
        ) -> Result<ExperimentResultRow, sqlx::Error> {
            Err(sqlx::Error::RowNotFound)
        }

        async fn fetch_latest(
            &self,
            _experiment_id: Uuid,
        ) -> Result<Vec<ExperimentResultRow>, sqlx::Error> {
            Ok(vec![])
        }

        async fn fetch_by_iteration(
            &self,
            _experiment_id: Uuid,
            _iteration_id: Uuid,
        ) -> Result<Vec<ExperimentResultRow>, sqlx::Error> {
            Ok(vec![])
        }

        async fn is_stale(
            &self,
            _experiment_id: Uuid,
            _iteration_id: Uuid,
        ) -> Result<bool, sqlx::Error> {
            Ok(false)
        }
    }

    struct ResultsWithDataRepo {
        rows: Vec<ExperimentResultRow>,
    }

    #[async_trait]
    impl ExperimentResultsRepository for ResultsWithDataRepo {
        async fn upsert(
            &self,
            _row: &UpsertResultRow,
        ) -> Result<ExperimentResultRow, sqlx::Error> {
            Err(sqlx::Error::RowNotFound)
        }

        async fn fetch_latest(
            &self,
            _experiment_id: Uuid,
        ) -> Result<Vec<ExperimentResultRow>, sqlx::Error> {
            Ok(self.rows.clone())
        }

        async fn fetch_by_iteration(
            &self,
            _experiment_id: Uuid,
            _iteration_id: Uuid,
        ) -> Result<Vec<ExperimentResultRow>, sqlx::Error> {
            Ok(self.rows.clone())
        }

        async fn is_stale(
            &self,
            _experiment_id: Uuid,
            _iteration_id: Uuid,
        ) -> Result<bool, sqlx::Error> {
            Ok(false)
        }
    }

    fn make_result_row(
        experiment_id: Uuid,
        variant_key: &str,
        metric_key: &str,
        count: u64,
    ) -> ExperimentResultRow {
        ExperimentResultRow {
            id: Uuid::new_v4(),
            experiment_id,
            iteration_id: Uuid::new_v4(),
            metric_key: metric_key.to_string(),
            metric_type: "count".to_string(),
            variant_stats: serde_json::json!({ variant_key: count }),
            frequentist_result: Some(serde_json::json!({ "p_value": 0.04 })),
            bayesian_result: None,
            recommendation: "ship_treatment".to_string(),
            computed_at: Utc::now(),
            created_at: Utc::now(),
        }
    }

    fn make_service(
        env_id: EnvironmentId,
    ) -> ExperimentationServiceImpl {
        ExperimentationServiceImpl::new(
            Arc::new(AlwaysSucceedRepo { env_id }),
            Arc::new(EmptyResultsRepo),
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
        assert_eq!(core_status_to_proto(ExperimentStatus::Draft), PS::Draft as i32);
    }

    #[test]
    fn test_core_running_to_proto() {
        use stitchd_proto::experiments::v1::ExperimentStatus as PS;
        assert_eq!(core_status_to_proto(ExperimentStatus::Running), PS::Active as i32);
    }

    #[test]
    fn test_core_paused_to_proto() {
        use stitchd_proto::experiments::v1::ExperimentStatus as PS;
        assert_eq!(core_status_to_proto(ExperimentStatus::Paused), PS::Paused as i32);
    }

    #[test]
    fn test_core_stopped_to_proto() {
        use stitchd_proto::experiments::v1::ExperimentStatus as PS;
        assert_eq!(core_status_to_proto(ExperimentStatus::Stopped), PS::Concluded as i32);
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
        let s = repo_err_to_status(RepositoryError::Unexpected(
            anyhow::anyhow!("unexpected error"),
        ));
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
            Arc::new(EmptyResultsRepo),
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
            Arc::new(EmptyResultsRepo),
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
        });
        let result = svc.list_experiments(req).await;
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().code(), tonic::Code::InvalidArgument);
    }

    #[tokio::test]
    async fn test_list_experiments_repo_failure_returns_error() {
        let svc = ExperimentationServiceImpl::new(
            Arc::new(NotFoundRepo),
            Arc::new(EmptyResultsRepo),
            None,
        );
        let req = tonic::Request::new(ListExperimentsRequest {
            environment_id: EnvironmentId::new().to_string(),
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
        let (env_id, _) = env_uuid();
        let exp_id = Uuid::new_v4();
        let rows = vec![
            make_result_row(exp_id, "control", "checkout", 100),
            make_result_row(exp_id, "treatment", "checkout", 120),
        ];
        let svc = ExperimentationServiceImpl::new(
            Arc::new(AlwaysSucceedRepo { env_id }),
            Arc::new(ResultsWithDataRepo { rows }),
            None,
        );
        let req = tonic::Request::new(GetResultsRequest {
            environment_id: env_id.to_string(),
            experiment_id: exp_id.to_string(),
        });
        let result = svc.get_results(req).await;
        assert!(result.is_ok());
        let resp = result.unwrap().into_inner();
        assert_eq!(resp.experiment_id, exp_id.to_string());
        assert_eq!(resp.variant_results.len(), 2);
        // Both variants should have p_value_present since we set frequentist_result
        for vr in &resp.variant_results {
            assert!(vr.p_value_present);
            assert!((vr.p_value - 0.04).abs() < 1e-9);
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
}
