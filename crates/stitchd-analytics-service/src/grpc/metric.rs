//! gRPC handlers for MetricService CRUD + preview RPCs.
//!
//! # Trait boundary
//! Handlers depend on a concrete [`PgMetricRepository`] from `stitchd-db`.
//! Tests use real Postgres via `#[sqlx::test]` rather than a mock trait —
//! aligns with `crates/stitchd-db/tests/metric_repository.rs` and avoids
//! a redundant mock when the repo trait is small.
//!
//! # Proto ↔ domain mapping
//! `stitchd_proto::analytics::v1::MetricDefinition` ↔
//! `stitchd_core::metric::MetricDefinition`:
//! - UUIDs are exchanged as strings; we parse with `Uuid::parse_str`.
//! - The proto `oneof kind` (Aggregation/Ratio/Funnel) maps to the
//!   `MetricKind` enum; aggregator string ↔ `AggregationOperator`.
//! - `where_clause_json` is a stringified JSON; domain stores it as
//!   `serde_json::Value`.
//! - Timestamps round-trip as RFC 3339 UTC.
//!
//! # PreviewMetric
//! The actual ClickHouse query lives in Phase 4 (stats-service).
//! [`handle_preview_metric`] currently returns an empty `buckets` array
//! and logs a `tracing::warn!` so callers know the wiring is pending.

use std::sync::Arc;

use chrono::Utc;
use tonic::{Request, Response, Status};
use tracing::{instrument, warn};
use uuid::Uuid;

use stitchd_core::{
    id::{EnvironmentId, MetricId},
    metric::{
        AggregationConfig, AggregationOperator, FunnelConfig, FunnelStep, GoalDirection,
        MetricDefinition as DomainMetric, MetricKind, RatioConfig,
    },
};
use stitchd_db::{
    ExperimentRepository, MetricRepository as _, RepositoryError,
    repository::pg::PgMetricRepository,
};
use stitchd_stats_service::recompute_trigger::{
    RecomputeTrigger, trigger_recompute_for_metric,
};
use stitchd_proto::analytics::v1::{
    AggregationConfig as ProtoAggregationConfig, CreateMetricRequest,
    DeleteMetricRequest, DeleteMetricResponse, FunnelConfig as ProtoFunnelConfig,
    FunnelStep as ProtoFunnelStep, GetMetricRequest, ListMetricsRequest, ListMetricsResponse,
    MetricDefinition as ProtoMetric, PreviewBucket, PreviewMetricRequest, PreviewMetricResponse,
    RatioConfig as ProtoRatioConfig, UpdateMetricRequest, create_metric_request,
    metric_definition, preview_metric_request, update_metric_request,
};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Default page size when `ListMetricsRequest.limit` is unset/zero.
const DEFAULT_LIST_LIMIT: u64 = 50;
/// Hard cap on page size (matches proto doc).
const MAX_LIST_LIMIT: u64 = 200;

// ---------------------------------------------------------------------------
// Proto ↔ domain conversion helpers
// ---------------------------------------------------------------------------

#[allow(clippy::result_large_err)]
fn parse_uuid(s: &str, field: &str) -> Result<Uuid, Status> {
    Uuid::parse_str(s).map_err(|_| Status::invalid_argument(format!("invalid {field}: {s}")))
}

#[allow(clippy::result_large_err)]
fn parse_metric_id(s: &str, field: &str) -> Result<MetricId, Status> {
    Ok(MetricId::from_uuid(parse_uuid(s, field)?))
}

#[allow(clippy::result_large_err)]
fn parse_environment_id(s: &str, field: &str) -> Result<EnvironmentId, Status> {
    Ok(EnvironmentId::from_uuid(parse_uuid(s, field)?))
}

#[allow(clippy::result_large_err)]
fn parse_goal_direction(s: &str) -> Result<GoalDirection, Status> {
    match s {
        "increase" => Ok(GoalDirection::Increase),
        "decrease" => Ok(GoalDirection::Decrease),
        "neutral" => Ok(GoalDirection::Neutral),
        other => Err(Status::invalid_argument(format!(
            "invalid goal_direction `{other}` (want increase|decrease|neutral)"
        ))),
    }
}

const fn goal_direction_to_str(g: GoalDirection) -> &'static str {
    match g {
        GoalDirection::Increase => "increase",
        GoalDirection::Decrease => "decrease",
        GoalDirection::Neutral => "neutral",
    }
}

#[allow(clippy::result_large_err)]
fn parse_aggregator(s: &str) -> Result<AggregationOperator, Status> {
    match s {
        "count" => Ok(AggregationOperator::Count),
        "sum" => Ok(AggregationOperator::Sum),
        "avg" => Ok(AggregationOperator::Avg),
        "p50" => Ok(AggregationOperator::P50),
        "p90" => Ok(AggregationOperator::P90),
        "p99" => Ok(AggregationOperator::P99),
        "uniq" => Ok(AggregationOperator::Uniq),
        other => Err(Status::invalid_argument(format!(
            "invalid aggregator `{other}` (want count|sum|avg|p50|p90|p99|uniq)"
        ))),
    }
}

const fn aggregator_to_str(op: AggregationOperator) -> &'static str {
    match op {
        AggregationOperator::Count => "count",
        AggregationOperator::Sum => "sum",
        AggregationOperator::Avg => "avg",
        AggregationOperator::P50 => "p50",
        AggregationOperator::P90 => "p90",
        AggregationOperator::P99 => "p99",
        AggregationOperator::Uniq => "uniq",
    }
}

#[allow(clippy::result_large_err)]
fn parse_where_clause(
    s: Option<&str>,
    field: &str,
) -> Result<Option<serde_json::Value>, Status> {
    match s {
        None | Some("") => Ok(None),
        Some(raw) => serde_json::from_str(raw)
            .map(Some)
            .map_err(|e| Status::invalid_argument(format!("{field} is not valid JSON: {e}"))),
    }
}

fn where_clause_to_proto(v: &Option<serde_json::Value>) -> Option<String> {
    v.as_ref()
        .map(|val| serde_json::to_string(val).unwrap_or_default())
}

#[allow(clippy::result_large_err)]
fn proto_aggregation_to_domain(
    cfg: ProtoAggregationConfig,
) -> Result<AggregationConfig, Status> {
    let aggregator = parse_aggregator(&cfg.aggregator)?;
    let where_clause = parse_where_clause(cfg.where_clause_json.as_deref(), "where_clause_json")?;
    Ok(AggregationConfig {
        event_key: cfg.event_key,
        aggregator,
        on_field: cfg.on_field,
        where_clause,
    })
}

#[allow(clippy::result_large_err)]
fn proto_ratio_to_domain(cfg: ProtoRatioConfig) -> Result<RatioConfig, Status> {
    Ok(RatioConfig {
        numerator_metric_id: parse_metric_id(&cfg.numerator_metric_id, "numerator_metric_id")?,
        denominator_metric_id: parse_metric_id(
            &cfg.denominator_metric_id,
            "denominator_metric_id",
        )?,
        min_denominator: cfg.min_denominator,
    })
}

#[allow(clippy::result_large_err)]
fn proto_funnel_to_domain(cfg: ProtoFunnelConfig) -> Result<FunnelConfig, Status> {
    let mut steps = Vec::with_capacity(cfg.steps.len());
    for s in cfg.steps {
        steps.push(FunnelStep {
            event_key: s.event_key,
            where_clause: parse_where_clause(
                s.where_clause_json.as_deref(),
                "funnel.step.where_clause_json",
            )?,
        });
    }
    Ok(FunnelConfig {
        steps,
        window_seconds: cfg.window_seconds,
        count_repeats: cfg.count_repeats,
    })
}

#[allow(clippy::result_large_err)]
fn create_kind_to_domain(kind: Option<create_metric_request::Kind>) -> Result<MetricKind, Status> {
    match kind {
        None => Err(Status::invalid_argument(
            "metric kind is required (aggregation|ratio|funnel)",
        )),
        Some(create_metric_request::Kind::Aggregation(c)) => {
            Ok(MetricKind::Aggregation(proto_aggregation_to_domain(c)?))
        }
        Some(create_metric_request::Kind::Ratio(c)) => {
            Ok(MetricKind::Ratio(proto_ratio_to_domain(c)?))
        }
        Some(create_metric_request::Kind::Funnel(c)) => {
            Ok(MetricKind::Funnel(proto_funnel_to_domain(c)?))
        }
    }
}

#[allow(clippy::result_large_err)]
fn update_kind_to_domain(kind: Option<update_metric_request::Kind>) -> Result<MetricKind, Status> {
    match kind {
        None => Err(Status::invalid_argument(
            "metric kind is required on update (aggregation|ratio|funnel)",
        )),
        Some(update_metric_request::Kind::Aggregation(c)) => {
            Ok(MetricKind::Aggregation(proto_aggregation_to_domain(c)?))
        }
        Some(update_metric_request::Kind::Ratio(c)) => {
            Ok(MetricKind::Ratio(proto_ratio_to_domain(c)?))
        }
        Some(update_metric_request::Kind::Funnel(c)) => {
            Ok(MetricKind::Funnel(proto_funnel_to_domain(c)?))
        }
    }
}

fn domain_kind_to_proto(kind: &MetricKind) -> metric_definition::Kind {
    match kind {
        MetricKind::Aggregation(c) => metric_definition::Kind::Aggregation(ProtoAggregationConfig {
            event_key: c.event_key.clone(),
            aggregator: aggregator_to_str(c.aggregator).to_string(),
            on_field: c.on_field.clone(),
            where_clause_json: where_clause_to_proto(&c.where_clause),
        }),
        MetricKind::Ratio(c) => metric_definition::Kind::Ratio(ProtoRatioConfig {
            numerator_metric_id: c.numerator_metric_id.to_string(),
            denominator_metric_id: c.denominator_metric_id.to_string(),
            min_denominator: c.min_denominator,
        }),
        MetricKind::Funnel(c) => metric_definition::Kind::Funnel(ProtoFunnelConfig {
            steps: c
                .steps
                .iter()
                .map(|s| ProtoFunnelStep {
                    event_key: s.event_key.clone(),
                    where_clause_json: where_clause_to_proto(&s.where_clause),
                })
                .collect(),
            window_seconds: c.window_seconds,
            count_repeats: c.count_repeats,
        }),
    }
}

fn domain_to_proto(m: &DomainMetric) -> ProtoMetric {
    ProtoMetric {
        id: m.id.to_string(),
        environment_id: m.environment_id.to_string(),
        key: m.key.clone(),
        name: m.name.clone(),
        description: m.description.clone(),
        kind: Some(domain_kind_to_proto(&m.kind)),
        goal_direction: goal_direction_to_str(m.goal_direction).to_string(),
        version: m.version,
        created_at: m.created_at.to_rfc3339(),
        updated_at: m.updated_at.to_rfc3339(),
        deleted_at: m.deleted_at.map(|t| t.to_rfc3339()),
    }
}

#[allow(clippy::result_large_err)]
fn validation_status(err: stitchd_core::metric::MetricValidationError) -> Status {
    Status::invalid_argument(err.to_string())
}

#[allow(clippy::result_large_err)]
fn repo_err_to_status(err: RepositoryError, ctx: &str) -> Status {
    match err {
        RepositoryError::NotFound { id } => {
            Status::not_found(format!("{ctx}: metric not found (id={id})"))
        }
        RepositoryError::VersionConflict { expected, actual } => Status::aborted(format!(
            "{ctx}: version conflict — expected={expected}, actual={actual}"
        )),
        RepositoryError::UniqueViolation { field } => Status::already_exists(format!(
            "{ctx}: unique violation on `{field}` (metric.key must be unique per environment)"
        )),
        RepositoryError::ForeignKeyViolation { constraint } => Status::failed_precondition(
            format!("{ctx}: foreign key violation on `{constraint}`"),
        ),
        RepositoryError::InvalidState { reason } => {
            Status::failed_precondition(format!("{ctx}: invalid state — {reason}"))
        }
        RepositoryError::Database(e) => Status::internal(format!("{ctx}: database error — {e}")),
        RepositoryError::Unexpected(e) => {
            Status::internal(format!("{ctx}: unexpected error — {e}"))
        }
    }
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// Handle `CreateMetric` — validates kind config and persists a fresh
/// metric with `version = 1`.
#[allow(clippy::result_large_err)]
#[instrument(skip(repo, request), name = "analytics.create_metric")]
pub async fn handle_create_metric(
    repo: &PgMetricRepository,
    request: Request<CreateMetricRequest>,
) -> Result<Response<ProtoMetric>, Status> {
    let req = request.into_inner();

    let environment_id = parse_environment_id(&req.environment_id, "environment_id")?;
    let goal_direction = parse_goal_direction(&req.goal_direction)?;
    let kind = create_kind_to_domain(req.kind)?;

    let now = Utc::now();
    let metric = DomainMetric {
        id: MetricId::new(),
        environment_id,
        key: req.key,
        name: req.name,
        description: req.description,
        kind,
        goal_direction,
        version: 1,
        created_at: now,
        updated_at: now,
        deleted_at: None,
    };

    metric.validate().map_err(validation_status)?;

    repo.create(&metric)
        .await
        .map_err(|e| repo_err_to_status(e, "create_metric"))?;

    Ok(Response::new(domain_to_proto(&metric)))
}

/// Handle `GetMetric` — supports lookup by `id` or by
/// `(environment_id, key)`.
#[allow(clippy::result_large_err)]
#[instrument(skip(repo, request), name = "analytics.get_metric")]
pub async fn handle_get_metric(
    repo: &PgMetricRepository,
    request: Request<GetMetricRequest>,
) -> Result<Response<ProtoMetric>, Status> {
    let req = request.into_inner();

    let by_id = req.id.as_deref().filter(|s| !s.is_empty());
    let by_env = req.environment_id.as_deref().filter(|s| !s.is_empty());
    let by_key = req.key.as_deref().filter(|s| !s.is_empty());

    let metric = if let Some(id_str) = by_id {
        let id = parse_metric_id(id_str, "id")?;
        repo.find_by_id(id)
            .await
            .map_err(|e| repo_err_to_status(e, "get_metric"))?
    } else if let (Some(env_str), Some(key)) = (by_env, by_key) {
        let env = parse_environment_id(env_str, "environment_id")?;
        repo.find_by_key(key, env)
            .await
            .map_err(|e| repo_err_to_status(e, "get_metric"))?
    } else {
        return Err(Status::invalid_argument(
            "must provide either `id` or both `environment_id` and `key`",
        ));
    };

    Ok(Response::new(domain_to_proto(&metric)))
}

/// Handle `ListMetrics` — paginated list of metrics in an environment,
/// optionally filtered by `kind`.
#[allow(clippy::result_large_err)]
#[instrument(skip(repo, request), name = "analytics.list_metrics")]
pub async fn handle_list_metrics(
    repo: &PgMetricRepository,
    request: Request<ListMetricsRequest>,
) -> Result<Response<ListMetricsResponse>, Status> {
    let req = request.into_inner();
    let env_id = parse_environment_id(&req.environment_id, "environment_id")?;

    let offset = req.offset.unwrap_or(0);
    let limit = match req.limit.unwrap_or(DEFAULT_LIST_LIMIT) {
        0 => DEFAULT_LIST_LIMIT,
        n => n.min(MAX_LIST_LIMIT),
    };

    let kind_filter = req.kind.as_deref().filter(|s| !s.is_empty());
    let event_key_filter = req.event_key.as_deref().filter(|s| !s.is_empty());

    // Fast path: when caller supplies `event_key`, bypass pagination and
    // serve the bounded result set from the dedicated repo method. The
    // EventDetail back-link UI surfaces ALL referencing metrics in one
    // section and never paginates them. `kind` filter still applies on
    // top of the result in case a caller combines both.
    if let Some(ek) = event_key_filter {
        let rows = repo
            .list_referencing_event(env_id, ek)
            .await
            .map_err(|e| repo_err_to_status(e, "list_metrics"))?;
        let items: Vec<ProtoMetric> = rows
            .into_iter()
            .filter(|m| kind_filter.is_none_or(|k| m.kind.tag() == k))
            .map(|m| domain_to_proto(&m))
            .collect();
        let total = items.len() as u64;
        return Ok(Response::new(ListMetricsResponse {
            items,
            total,
            offset: 0,
            limit: total,
        }));
    }

    let (rows, total) = repo
        .list_by_environment_paginated(env_id, offset, limit)
        .await
        .map_err(|e| repo_err_to_status(e, "list_metrics"))?;

    let items: Vec<ProtoMetric> = rows
        .into_iter()
        .filter(|m| kind_filter.is_none_or(|k| m.kind.tag() == k))
        .map(|m| domain_to_proto(&m))
        .collect();

    Ok(Response::new(ListMetricsResponse {
        items,
        total,
        offset,
        limit,
    }))
}

/// Handle `UpdateMetric` — optimistic-locking update; rebuilds the
/// metric from the existing row, applies the patch, then calls
/// `repo.update`.
///
/// # Event-driven recompute (Phase 4 Task 3)
/// On a successful update, this handler fires (via `tokio::spawn`) a
/// `TriggerRecompute` for every *running* experiment that references
/// the metric's `key`. This keeps the analytics-service PATCH response
/// fast while ensuring stale results don't sit in the cache until the
/// next scheduled stats run.
///
/// The dispatch is **fire-and-forget** — repo / RPC errors are
/// logged at WARN but never propagate to the caller. See
/// `crates/stitchd-stats-service/src/recompute_trigger.rs`.
///
/// Pass `recompute_dispatcher = None` (e.g. in unit tests or before
/// the channel is wired) to skip the side effect entirely.
#[allow(clippy::result_large_err)]
#[instrument(
    skip(repo, experiment_repo, recompute_dispatcher, request),
    name = "analytics.update_metric"
)]
pub async fn handle_update_metric(
    repo: &PgMetricRepository,
    experiment_repo: Option<Arc<dyn ExperimentRepository>>,
    recompute_dispatcher: Option<Arc<dyn RecomputeTrigger>>,
    request: Request<UpdateMetricRequest>,
) -> Result<Response<ProtoMetric>, Status> {
    let req = request.into_inner();
    let id = parse_metric_id(&req.id, "id")?;

    // Fetch existing to merge patch onto. NotFound surfaces immediately.
    let mut metric = repo
        .find_by_id(id)
        .await
        .map_err(|e| repo_err_to_status(e, "update_metric"))?;

    // The caller's `expected_version` is what the repo uses for the
    // optimistic-locking check. We overwrite `metric.version` so the
    // SQL UPDATE's `WHERE version = $X` clause uses the caller's value.
    metric.version = req.expected_version;

    if let Some(n) = req.name {
        metric.name = n;
    }
    if req.description.is_some() {
        metric.description = req.description;
    }
    metric.kind = update_kind_to_domain(req.kind)?;
    if let Some(gd) = req.goal_direction {
        metric.goal_direction = parse_goal_direction(&gd)?;
    }

    metric.validate().map_err(validation_status)?;

    let updated = repo
        .update(&metric)
        .await
        .map_err(|e| repo_err_to_status(e, "update_metric"))?;

    // Fire-and-forget recompute trigger for running experiments that
    // reference this metric. Failure here MUST NOT surface to the
    // caller — log + drop.
    if let (Some(exp_repo), Some(dispatch)) = (experiment_repo, recompute_dispatcher) {
        let metric_id = updated.id;
        let env_id = updated.environment_id;
        tokio::spawn(async move {
            match trigger_recompute_for_metric(
                dispatch.as_ref(),
                exp_repo.as_ref(),
                metric_id,
                env_id,
            )
            .await
            {
                Ok(n) => {
                    tracing::info!(
                        metric_id = %metric_id,
                        environment_id = %env_id,
                        triggers_fired = n,
                        "update_metric: recompute triggers dispatched",
                    );
                }
                Err(e) => {
                    tracing::warn!(
                        metric_id = %metric_id,
                        environment_id = %env_id,
                        "update_metric: recompute dispatch failed: {e}",
                    );
                }
            }
        });
    }

    Ok(Response::new(domain_to_proto(&updated)))
}

/// Handle `DeleteMetric` — soft-deletes the row by setting `deleted_at`.
#[allow(clippy::result_large_err)]
#[instrument(skip(repo, request), name = "analytics.delete_metric")]
pub async fn handle_delete_metric(
    repo: &PgMetricRepository,
    request: Request<DeleteMetricRequest>,
) -> Result<Response<DeleteMetricResponse>, Status> {
    let req = request.into_inner();
    let id = parse_metric_id(&req.id, "id")?;

    repo.soft_delete(id)
        .await
        .map_err(|e| repo_err_to_status(e, "delete_metric"))?;

    Ok(Response::new(DeleteMetricResponse {}))
}

/// Handle `PreviewMetric` — returns an empty `buckets` array for now.
///
/// The actual ClickHouse query (last N-day time-series) is wired in
/// Phase 4 by the stats-service. This stub validates that the target
/// reference (id OR inline draft) is well-formed and logs a warn so
/// callers know the data path is pending.
#[allow(clippy::result_large_err)]
#[instrument(skip(_repo, request), name = "analytics.preview_metric")]
pub async fn handle_preview_metric(
    _repo: &PgMetricRepository,
    request: Request<PreviewMetricRequest>,
) -> Result<Response<PreviewMetricResponse>, Status> {
    let req = request.into_inner();

    // Validate `target` is present so callers get a clear error early.
    match req.target {
        None => {
            return Err(Status::invalid_argument(
                "preview_metric requires `id` or `build_inline`",
            ));
        }
        Some(preview_metric_request::Target::Id(s)) => {
            // Validate id parses; we don't fetch the row in this phase.
            let _ = parse_metric_id(&s, "id")?;
        }
        Some(preview_metric_request::Target::BuildInline(_inline)) => {
            // For inline drafts the caller has the definition in hand;
            // we accept it as-is (validation happens at the stats layer
            // when the query is built).
        }
    }

    warn!(
        "PreviewMetric not yet implemented — Phase 4 wires the ClickHouse query \
         (days={})",
        req.days
    );

    Ok(Response::new(PreviewMetricResponse { buckets: Vec::<PreviewBucket>::new() }))
}

// ---------------------------------------------------------------------------
// Internal pure-function tests (no DB)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod proto_mapping_tests {
    use super::*;
    use serde_json::json;
    use stitchd_proto::analytics::v1::create_metric_request;

    #[test]
    fn parse_aggregator_matches_all_operators() {
        assert_eq!(parse_aggregator("count").unwrap(), AggregationOperator::Count);
        assert_eq!(parse_aggregator("sum").unwrap(), AggregationOperator::Sum);
        assert_eq!(parse_aggregator("avg").unwrap(), AggregationOperator::Avg);
        assert_eq!(parse_aggregator("p50").unwrap(), AggregationOperator::P50);
        assert_eq!(parse_aggregator("p90").unwrap(), AggregationOperator::P90);
        assert_eq!(parse_aggregator("p99").unwrap(), AggregationOperator::P99);
        assert_eq!(parse_aggregator("uniq").unwrap(), AggregationOperator::Uniq);
    }

    #[test]
    fn parse_aggregator_rejects_unknown() {
        let err = parse_aggregator("median").unwrap_err();
        assert_eq!(err.code(), tonic::Code::InvalidArgument);
        assert!(err.message().contains("median"));
    }

    #[test]
    fn parse_goal_direction_round_trip() {
        for (s, want) in [
            ("increase", GoalDirection::Increase),
            ("decrease", GoalDirection::Decrease),
            ("neutral", GoalDirection::Neutral),
        ] {
            let g = parse_goal_direction(s).unwrap();
            assert_eq!(g, want);
            assert_eq!(goal_direction_to_str(g), s);
        }
    }

    #[test]
    fn parse_where_clause_handles_empty_and_absent() {
        assert!(parse_where_clause(None, "wc").unwrap().is_none());
        assert!(parse_where_clause(Some(""), "wc").unwrap().is_none());
        let v = parse_where_clause(Some(r#"{"==":[1,1]}"#), "wc").unwrap();
        assert_eq!(v, Some(json!({"==":[1,1]})));
    }

    #[test]
    fn parse_where_clause_rejects_garbage_json() {
        let err = parse_where_clause(Some("not-json"), "wc").unwrap_err();
        assert_eq!(err.code(), tonic::Code::InvalidArgument);
        assert!(err.message().contains("wc"));
    }

    #[test]
    fn proto_aggregation_round_trips() {
        let proto = ProtoAggregationConfig {
            event_key: "checkout_completed".into(),
            aggregator: "sum".into(),
            on_field: Some("revenue".into()),
            where_clause_json: Some(r#"{"==":[{"var":"c"},"USD"]}"#.into()),
        };
        let dom = proto_aggregation_to_domain(proto).unwrap();
        assert_eq!(dom.event_key, "checkout_completed");
        assert_eq!(dom.aggregator, AggregationOperator::Sum);
        assert_eq!(dom.on_field.as_deref(), Some("revenue"));
        assert!(dom.where_clause.is_some());

        let kind = MetricKind::Aggregation(dom);
        match domain_kind_to_proto(&kind) {
            metric_definition::Kind::Aggregation(p) => {
                assert_eq!(p.aggregator, "sum");
                assert_eq!(p.on_field.as_deref(), Some("revenue"));
                assert!(p.where_clause_json.unwrap().contains("USD"));
            }
            other => panic!("expected Aggregation, got {other:?}"),
        }
    }

    #[test]
    fn create_kind_rejects_missing_oneof() {
        let err = create_kind_to_domain(None).unwrap_err();
        assert_eq!(err.code(), tonic::Code::InvalidArgument);
    }

    #[test]
    fn create_kind_accepts_funnel() {
        let proto = create_metric_request::Kind::Funnel(ProtoFunnelConfig {
            steps: vec![
                ProtoFunnelStep {
                    event_key: "view".into(),
                    where_clause_json: None,
                },
                ProtoFunnelStep {
                    event_key: "buy".into(),
                    where_clause_json: None,
                },
            ],
            window_seconds: 3600,
            count_repeats: false,
        });
        let domain = create_kind_to_domain(Some(proto)).unwrap();
        match domain {
            MetricKind::Funnel(f) => {
                assert_eq!(f.steps.len(), 2);
                assert_eq!(f.window_seconds, 3600);
            }
            other => panic!("expected Funnel, got {other:?}"),
        }
    }

    #[test]
    fn repo_err_version_conflict_maps_to_aborted() {
        let s = repo_err_to_status(
            RepositoryError::VersionConflict {
                expected: 1,
                actual: 2,
            },
            "update_metric",
        );
        assert_eq!(s.code(), tonic::Code::Aborted);
        assert!(s.message().contains("expected=1"));
        assert!(s.message().contains("actual=2"));
    }

    #[test]
    fn repo_err_not_found_maps_to_not_found() {
        let s = repo_err_to_status(
            RepositoryError::NotFound {
                id: "abc".into(),
            },
            "get_metric",
        );
        assert_eq!(s.code(), tonic::Code::NotFound);
        assert!(s.message().contains("abc"));
    }

    #[test]
    fn repo_err_unique_violation_maps_to_already_exists() {
        let s = repo_err_to_status(
            RepositoryError::UniqueViolation {
                field: "metric_definitions_env_key_unique".into(),
            },
            "create_metric",
        );
        assert_eq!(s.code(), tonic::Code::AlreadyExists);
    }

    #[test]
    fn domain_to_proto_round_trips_aggregation() {
        let m = DomainMetric {
            id: MetricId::new(),
            environment_id: EnvironmentId::new(),
            key: "k".into(),
            name: "n".into(),
            description: Some("d".into()),
            kind: MetricKind::Aggregation(AggregationConfig {
                event_key: "ev".into(),
                aggregator: AggregationOperator::Count,
                on_field: None,
                where_clause: None,
            }),
            goal_direction: GoalDirection::Increase,
            version: 3,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            deleted_at: None,
        };
        let p = domain_to_proto(&m);
        assert_eq!(p.key, "k");
        assert_eq!(p.version, 3);
        assert_eq!(p.goal_direction, "increase");
        match p.kind.unwrap() {
            metric_definition::Kind::Aggregation(c) => assert_eq!(c.aggregator, "count"),
            other => panic!("expected Aggregation, got {other:?}"),
        }
    }

}

// ---------------------------------------------------------------------------
// End-to-end DB-backed handler tests (sqlx::test)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod handler_tests {
    use super::*;
    use std::sync::Arc;
    use stitchd_core::{
        id::{OrganisationId, ProjectId},
        tenant::{Environment, Organisation, Project},
    };
    use stitchd_db::{
        EnvironmentRepository, OrganisationRepository, ProjectRepository,
        repository::pg::{
            PgAuditLogger, PgEnvironmentRepository, PgOrganisationRepository,
            PgProjectRepository,
        },
    };

    async fn seed_env(pool: &sqlx::PgPool) -> (PgMetricRepository, EnvironmentId) {
        let audit = Arc::new(PgAuditLogger::new(pool.clone()));
        let org_repo = PgOrganisationRepository::new(pool.clone(), audit.clone());
        let proj_repo = PgProjectRepository::new(pool.clone(), audit.clone());
        let env_repo = PgEnvironmentRepository::new(pool.clone(), audit.clone());
        let metric_repo = PgMetricRepository::new(pool.clone(), audit);

        let org = Organisation {
            id: OrganisationId::new(),
            name: "Org".into(),
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            deleted_at: None,
            version: 1,
            is_system: false,
        };
        org_repo.create(&org).await.unwrap();
        let project = Project {
            id: ProjectId::new(),
            organisation_id: org.id,
            name: "Proj".into(),
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            deleted_at: None,
            version: 1,
        };
        proj_repo.create(&project).await.unwrap();
        let env = Environment {
            id: EnvironmentId::new(),
            project_id: project.id,
            name: "Env".into(),
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            deleted_at: None,
            version: 1,
        };
        env_repo.create(&env).await.unwrap();
        (metric_repo, env.id)
    }

    fn agg_create_request(env: EnvironmentId, key: &str) -> Request<CreateMetricRequest> {
        Request::new(CreateMetricRequest {
            environment_id: env.to_string(),
            key: key.into(),
            name: format!("Metric {key}"),
            description: Some("Test metric".into()),
            goal_direction: "increase".into(),
            kind: Some(create_metric_request::Kind::Aggregation(
                ProtoAggregationConfig {
                    event_key: "checkout_completed".into(),
                    aggregator: "count".into(),
                    on_field: None,
                    where_clause_json: None,
                },
            )),
        })
    }

    #[sqlx::test(migrations = "../stitchd-db/migrations")]
    async fn test_create_metric_persists_and_returns(pool: sqlx::PgPool) {
        let (repo, env) = seed_env(&pool).await;
        let resp = handle_create_metric(&repo, agg_create_request(env, "checkout_rate"))
            .await
            .expect("create ok");
        let proto = resp.into_inner();
        assert_eq!(proto.key, "checkout_rate");
        assert_eq!(proto.environment_id, env.to_string());
        assert_eq!(proto.goal_direction, "increase");
        assert_eq!(proto.version, 1);
        assert!(matches!(
            proto.kind.unwrap(),
            metric_definition::Kind::Aggregation(_)
        ));

        // Verify persistence — find_by_key should hit it.
        let fetched = repo.find_by_key("checkout_rate", env).await.unwrap();
        assert_eq!(fetched.key, "checkout_rate");
    }

    #[sqlx::test(migrations = "../stitchd-db/migrations")]
    async fn test_create_metric_validates_kind_config(pool: sqlx::PgPool) {
        let (repo, env) = seed_env(&pool).await;
        // Funnel with only 1 step → InvalidArgument from validate()
        let req = Request::new(CreateMetricRequest {
            environment_id: env.to_string(),
            key: "bad_funnel".into(),
            name: "Bad Funnel".into(),
            description: None,
            goal_direction: "increase".into(),
            kind: Some(create_metric_request::Kind::Funnel(ProtoFunnelConfig {
                steps: vec![ProtoFunnelStep {
                    event_key: "only".into(),
                    where_clause_json: None,
                }],
                window_seconds: 60,
                count_repeats: false,
            })),
        });
        let err = handle_create_metric(&repo, req).await.unwrap_err();
        assert_eq!(err.code(), tonic::Code::InvalidArgument);
        assert!(err.message().contains("funnel"));
    }

    #[sqlx::test(migrations = "../stitchd-db/migrations")]
    async fn test_create_metric_rejects_invalid_goal_direction(pool: sqlx::PgPool) {
        let (repo, env) = seed_env(&pool).await;
        let mut req = agg_create_request(env, "k").into_inner();
        req.goal_direction = "sideways".into();
        let err = handle_create_metric(&repo, Request::new(req))
            .await
            .unwrap_err();
        assert_eq!(err.code(), tonic::Code::InvalidArgument);
    }

    #[sqlx::test(migrations = "../stitchd-db/migrations")]
    async fn test_create_metric_rejects_invalid_env_uuid(pool: sqlx::PgPool) {
        let (repo, _env) = seed_env(&pool).await;
        let mut req = agg_create_request(EnvironmentId::new(), "k").into_inner();
        req.environment_id = "not-a-uuid".into();
        let err = handle_create_metric(&repo, Request::new(req))
            .await
            .unwrap_err();
        assert_eq!(err.code(), tonic::Code::InvalidArgument);
        assert!(err.message().contains("environment_id"));
    }

    #[sqlx::test(migrations = "../stitchd-db/migrations")]
    async fn test_get_metric_by_id_returns_existing(pool: sqlx::PgPool) {
        let (repo, env) = seed_env(&pool).await;
        let created = handle_create_metric(&repo, agg_create_request(env, "g_by_id"))
            .await
            .unwrap()
            .into_inner();

        let req = Request::new(GetMetricRequest {
            id: Some(created.id.clone()),
            environment_id: None,
            key: None,
        });
        let fetched = handle_get_metric(&repo, req).await.unwrap().into_inner();
        assert_eq!(fetched.id, created.id);
        assert_eq!(fetched.key, "g_by_id");
    }

    #[sqlx::test(migrations = "../stitchd-db/migrations")]
    async fn test_get_metric_by_key_returns_existing(pool: sqlx::PgPool) {
        let (repo, env) = seed_env(&pool).await;
        handle_create_metric(&repo, agg_create_request(env, "g_by_key"))
            .await
            .unwrap();

        let req = Request::new(GetMetricRequest {
            id: None,
            environment_id: Some(env.to_string()),
            key: Some("g_by_key".into()),
        });
        let fetched = handle_get_metric(&repo, req).await.unwrap().into_inner();
        assert_eq!(fetched.key, "g_by_key");
    }

    #[sqlx::test(migrations = "../stitchd-db/migrations")]
    async fn test_get_metric_not_found_returns_status_not_found(pool: sqlx::PgPool) {
        let (repo, _env) = seed_env(&pool).await;
        let req = Request::new(GetMetricRequest {
            id: Some(MetricId::new().to_string()),
            environment_id: None,
            key: None,
        });
        let err = handle_get_metric(&repo, req).await.unwrap_err();
        assert_eq!(err.code(), tonic::Code::NotFound);
    }

    #[sqlx::test(migrations = "../stitchd-db/migrations")]
    async fn test_get_metric_requires_id_or_env_key(pool: sqlx::PgPool) {
        let (repo, _env) = seed_env(&pool).await;
        let req = Request::new(GetMetricRequest {
            id: None,
            environment_id: None,
            key: None,
        });
        let err = handle_get_metric(&repo, req).await.unwrap_err();
        assert_eq!(err.code(), tonic::Code::InvalidArgument);
    }

    #[sqlx::test(migrations = "../stitchd-db/migrations")]
    async fn test_list_metrics_returns_paginated_total(pool: sqlx::PgPool) {
        let (repo, env) = seed_env(&pool).await;
        for i in 0..3_u32 {
            handle_create_metric(&repo, agg_create_request(env, &format!("m{i}")))
                .await
                .unwrap();
        }

        let req = Request::new(ListMetricsRequest {
            environment_id: env.to_string(),
            offset: Some(0),
            limit: Some(2),
            kind: None,
            event_key: None,
        });
        let resp = handle_list_metrics(&repo, req).await.unwrap().into_inner();
        assert_eq!(resp.items.len(), 2);
        assert_eq!(resp.total, 3);
        assert_eq!(resp.offset, 0);
        assert_eq!(resp.limit, 2);
    }

    #[sqlx::test(migrations = "../stitchd-db/migrations")]
    async fn test_list_metrics_applies_default_limit(pool: sqlx::PgPool) {
        let (repo, env) = seed_env(&pool).await;
        handle_create_metric(&repo, agg_create_request(env, "only_one"))
            .await
            .unwrap();
        let req = Request::new(ListMetricsRequest {
            environment_id: env.to_string(),
            offset: None,
            limit: None,
            kind: None,
            event_key: None,
        });
        let resp = handle_list_metrics(&repo, req).await.unwrap().into_inner();
        assert_eq!(resp.limit, DEFAULT_LIST_LIMIT);
        assert_eq!(resp.total, 1);
    }

    #[sqlx::test(migrations = "../stitchd-db/migrations")]
    async fn test_list_metrics_filters_by_kind(pool: sqlx::PgPool) {
        let (repo, env) = seed_env(&pool).await;
        handle_create_metric(&repo, agg_create_request(env, "agg1"))
            .await
            .unwrap();
        handle_create_metric(&repo, agg_create_request(env, "agg2"))
            .await
            .unwrap();
        // Filter to ratio — should yield 0 items but total reflects the
        // raw page row count (filter is post-pagination, by design).
        let req = Request::new(ListMetricsRequest {
            environment_id: env.to_string(),
            offset: Some(0),
            limit: Some(50),
            kind: Some("ratio".into()),
            event_key: None,
        });
        let resp = handle_list_metrics(&repo, req).await.unwrap().into_inner();
        assert!(resp.items.is_empty());
    }

    #[sqlx::test(migrations = "../stitchd-db/migrations")]
    async fn test_update_metric_increments_version(pool: sqlx::PgPool) {
        let (repo, env) = seed_env(&pool).await;
        let created = handle_create_metric(&repo, agg_create_request(env, "mutable"))
            .await
            .unwrap()
            .into_inner();
        assert_eq!(created.version, 1);

        let req = Request::new(UpdateMetricRequest {
            id: created.id.clone(),
            expected_version: 1,
            name: Some("New Name".into()),
            description: Some("Updated".into()),
            goal_direction: Some("decrease".into()),
            kind: Some(update_metric_request::Kind::Aggregation(
                ProtoAggregationConfig {
                    event_key: "checkout_completed".into(),
                    aggregator: "count".into(),
                    on_field: None,
                    where_clause_json: None,
                },
            )),
        });
        let updated = handle_update_metric(&repo, None, None, req)
            .await
            .unwrap()
            .into_inner();
        assert_eq!(updated.version, 2);
        assert_eq!(updated.name, "New Name");
        assert_eq!(updated.description.as_deref(), Some("Updated"));
        assert_eq!(updated.goal_direction, "decrease");
    }

    #[sqlx::test(migrations = "../stitchd-db/migrations")]
    async fn test_update_metric_stale_version_returns_aborted(pool: sqlx::PgPool) {
        let (repo, env) = seed_env(&pool).await;
        let created = handle_create_metric(&repo, agg_create_request(env, "raced"))
            .await
            .unwrap()
            .into_inner();

        // First update bumps to v2.
        handle_update_metric(
            &repo,
            None,
            None,
            Request::new(UpdateMetricRequest {
                id: created.id.clone(),
                expected_version: 1,
                name: Some("v2".into()),
                description: None,
                goal_direction: None,
                kind: Some(update_metric_request::Kind::Aggregation(
                    ProtoAggregationConfig {
                        event_key: "checkout_completed".into(),
                        aggregator: "count".into(),
                        on_field: None,
                        where_clause_json: None,
                    },
                )),
            }),
        )
        .await
        .unwrap();

        // Second update with stale `expected_version=1` → Aborted.
        let err = handle_update_metric(
            &repo,
            None,
            None,
            Request::new(UpdateMetricRequest {
                id: created.id.clone(),
                expected_version: 1,
                name: Some("stale".into()),
                description: None,
                goal_direction: None,
                kind: Some(update_metric_request::Kind::Aggregation(
                    ProtoAggregationConfig {
                        event_key: "checkout_completed".into(),
                        aggregator: "count".into(),
                        on_field: None,
                        where_clause_json: None,
                    },
                )),
            }),
        )
        .await
        .unwrap_err();
        assert_eq!(err.code(), tonic::Code::Aborted);
        assert!(err.message().contains("expected=1"));
    }

    #[sqlx::test(migrations = "../stitchd-db/migrations")]
    async fn test_update_metric_validates_kind_config(pool: sqlx::PgPool) {
        let (repo, env) = seed_env(&pool).await;
        let created = handle_create_metric(&repo, agg_create_request(env, "validation"))
            .await
            .unwrap()
            .into_inner();

        // Funnel with 1 step on update → validation error.
        let req = Request::new(UpdateMetricRequest {
            id: created.id.clone(),
            expected_version: 1,
            name: None,
            description: None,
            goal_direction: None,
            kind: Some(update_metric_request::Kind::Funnel(ProtoFunnelConfig {
                steps: vec![ProtoFunnelStep {
                    event_key: "only".into(),
                    where_clause_json: None,
                }],
                window_seconds: 60,
                count_repeats: false,
            })),
        });
        let err = handle_update_metric(&repo, None, None, req).await.unwrap_err();
        assert_eq!(err.code(), tonic::Code::InvalidArgument);
    }

    #[sqlx::test(migrations = "../stitchd-db/migrations")]
    async fn test_delete_metric_soft_deletes(pool: sqlx::PgPool) {
        let (repo, env) = seed_env(&pool).await;
        let created = handle_create_metric(&repo, agg_create_request(env, "delete_me"))
            .await
            .unwrap()
            .into_inner();

        let req = Request::new(DeleteMetricRequest {
            id: created.id.clone(),
        });
        handle_delete_metric(&repo, req).await.unwrap();

        // After delete, GET by id should yield NotFound.
        let err = handle_get_metric(
            &repo,
            Request::new(GetMetricRequest {
                id: Some(created.id.clone()),
                environment_id: None,
                key: None,
            }),
        )
        .await
        .unwrap_err();
        assert_eq!(err.code(), tonic::Code::NotFound);
    }

    #[sqlx::test(migrations = "../stitchd-db/migrations")]
    async fn test_delete_metric_already_deleted_returns_not_found(pool: sqlx::PgPool) {
        let (repo, _env) = seed_env(&pool).await;
        let req = Request::new(DeleteMetricRequest {
            id: MetricId::new().to_string(),
        });
        let err = handle_delete_metric(&repo, req).await.unwrap_err();
        assert_eq!(err.code(), tonic::Code::NotFound);
    }

    #[sqlx::test(migrations = "../stitchd-db/migrations")]
    async fn test_preview_metric_returns_empty_buckets_with_warning_for_now(
        pool: sqlx::PgPool,
    ) {
        let (repo, env) = seed_env(&pool).await;
        let created = handle_create_metric(&repo, agg_create_request(env, "preview_me"))
            .await
            .unwrap()
            .into_inner();

        let req = Request::new(PreviewMetricRequest {
            target: Some(preview_metric_request::Target::Id(created.id.clone())),
            days: 7,
        });
        let resp = handle_preview_metric(&repo, req)
            .await
            .unwrap()
            .into_inner();
        assert!(
            resp.buckets.is_empty(),
            "Phase 3 preview returns empty buckets; Phase 4 wires the ClickHouse query"
        );
    }

    #[sqlx::test(migrations = "../stitchd-db/migrations")]
    async fn test_preview_metric_rejects_missing_target(pool: sqlx::PgPool) {
        let (repo, _env) = seed_env(&pool).await;
        let req = Request::new(PreviewMetricRequest {
            target: None,
            days: 7,
        });
        let err = handle_preview_metric(&repo, req).await.unwrap_err();
        assert_eq!(err.code(), tonic::Code::InvalidArgument);
    }

    // -----------------------------------------------------------------------
    // Phase 4 Task 3: update_metric dispatches recompute trigger
    // -----------------------------------------------------------------------

    use async_trait::async_trait;
    use stitchd_core::experimentation::{Experiment, ExperimentIteration, ExperimentStatus};
    use stitchd_core::id::ExperimentId;
    use stitchd_core::id::RuleId;
    use stitchd_db::ExperimentRepository;
    use stitchd_stats_service::recompute_trigger::RecomputeTrigger;
    use std::sync::Mutex;

    /// Hand-rolled `RecomputeTrigger` that records every call.
    /// Optionally returns `Err` from `trigger()` to validate the
    /// fire-and-forget contract.
    #[derive(Default)]
    struct CapturingDispatcher {
        calls: Mutex<Vec<ExperimentId>>,
        always_fail: bool,
    }

    impl CapturingDispatcher {
        fn calls(&self) -> Vec<ExperimentId> {
            self.calls.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl RecomputeTrigger for CapturingDispatcher {
        async fn trigger(&self, experiment_id: ExperimentId) -> Result<(), String> {
            self.calls.lock().unwrap().push(experiment_id);
            if self.always_fail {
                Err("forced".into())
            } else {
                Ok(())
            }
        }
    }

    /// In-memory experiment repo containing a single Running experiment
    /// referencing one metric key. All other methods panic (unused).
    struct OneRunningExperimentRepo {
        rows: Vec<Experiment>,
    }

    #[async_trait]
    impl ExperimentRepository for OneRunningExperimentRepo {
        async fn find_by_id(
            &self,
            _id: ExperimentId,
        ) -> Result<Experiment, stitchd_db::RepositoryError> {
            unimplemented!()
        }
        async fn list_by_environment(
            &self,
            env_id: EnvironmentId,
            status_filter: Option<ExperimentStatus>,
        ) -> Result<Vec<Experiment>, stitchd_db::RepositoryError> {
            Ok(self
                .rows
                .iter()
                .filter(|r| r.environment_id == env_id)
                .filter(|r| status_filter.is_none_or(|s| r.status == s))
                .cloned()
                .collect())
        }
        async fn list_by_environment_paginated(
            &self,
            _env_id: EnvironmentId,
            _offset: u64,
            _limit: u64,
        ) -> Result<(Vec<Experiment>, u64), stitchd_db::RepositoryError> {
            unimplemented!()
        }
        async fn create(
            &self,
            _experiment: &Experiment,
        ) -> Result<(), stitchd_db::RepositoryError> {
            unimplemented!()
        }
        async fn update(
            &self,
            _experiment: &Experiment,
        ) -> Result<Experiment, stitchd_db::RepositoryError> {
            unimplemented!()
        }
        async fn soft_delete(
            &self,
            _id: ExperimentId,
        ) -> Result<(), stitchd_db::RepositoryError> {
            unimplemented!()
        }
        async fn list_iterations(
            &self,
            _experiment_id: ExperimentId,
        ) -> Result<Vec<ExperimentIteration>, stitchd_db::RepositoryError> {
            unimplemented!()
        }
        async fn apply_transition(
            &self,
            _id: ExperimentId,
            _to: ExperimentStatus,
            _actor_id: Option<stitchd_core::id::UserId>,
        ) -> Result<Experiment, stitchd_db::RepositoryError> {
            unimplemented!()
        }
        async fn list_all_running(
            &self,
        ) -> Result<Vec<Experiment>, stitchd_db::RepositoryError> {
            unimplemented!()
        }
        async fn find_iteration_by_id(
            &self,
            _iteration_id: stitchd_db::ExperimentIterationId,
        ) -> Result<ExperimentIteration, stitchd_db::RepositoryError> {
            unimplemented!()
        }
    }

    fn running_exp_with_metric(env: EnvironmentId, metric_id: MetricId) -> Experiment {
        Experiment {
            id: ExperimentId::new(),
            environment_id: env,
            flag_rule_id: RuleId::new(),
            name: "exp".into(),
            description: None,
            hypothesis: None,
            metric_ids: vec![metric_id],
            traffic_allocation: 100.0,
            min_sample_size: None,
            scheduled_start_at: None,
            scheduled_end_at: None,
            status: ExperimentStatus::Running,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            deleted_at: None,
            version: 1,
        }
    }

    #[sqlx::test(migrations = "../stitchd-db/migrations")]
    async fn test_handler_update_metric_dispatches_recompute_async(pool: sqlx::PgPool) {
        let (repo, env) = seed_env(&pool).await;
        let created = handle_create_metric(&repo, agg_create_request(env, "to_dispatch"))
            .await
            .unwrap()
            .into_inner();
        let created_metric_id = MetricId::from_uuid(uuid::Uuid::parse_str(&created.id).unwrap());

        let exp = running_exp_with_metric(env, created_metric_id);
        let exp_id = exp.id;
        let exp_repo: Arc<dyn ExperimentRepository> = Arc::new(OneRunningExperimentRepo {
            rows: vec![exp],
        });
        let dispatcher = Arc::new(CapturingDispatcher::default());
        let dispatcher_arc: Arc<dyn RecomputeTrigger> = dispatcher.clone();

        let req = Request::new(UpdateMetricRequest {
            id: created.id.clone(),
            expected_version: 1,
            name: Some("renamed".into()),
            description: None,
            goal_direction: None,
            kind: Some(update_metric_request::Kind::Aggregation(
                ProtoAggregationConfig {
                    event_key: "checkout_completed".into(),
                    aggregator: "count".into(),
                    on_field: None,
                    where_clause_json: None,
                },
            )),
        });
        let updated =
            handle_update_metric(&repo, Some(exp_repo), Some(dispatcher_arc), req)
                .await
                .unwrap()
                .into_inner();
        assert_eq!(updated.version, 2);

        // Trigger is fire-and-forget — yield the scheduler so the
        // spawned task gets a chance to run before we read calls.
        for _ in 0..50 {
            if !dispatcher.calls().is_empty() {
                break;
            }
            tokio::task::yield_now().await;
        }
        let calls = dispatcher.calls();
        assert_eq!(calls.len(), 1, "exactly one running experiment matched");
        assert_eq!(calls[0], exp_id);
    }

    #[sqlx::test(migrations = "../stitchd-db/migrations")]
    async fn test_handler_recompute_failure_does_not_fail_update(pool: sqlx::PgPool) {
        let (repo, env) = seed_env(&pool).await;
        let created =
            handle_create_metric(&repo, agg_create_request(env, "still_succeeds"))
                .await
                .unwrap()
                .into_inner();
        let created_metric_id = MetricId::from_uuid(uuid::Uuid::parse_str(&created.id).unwrap());

        let exp = running_exp_with_metric(env, created_metric_id);
        let exp_repo: Arc<dyn ExperimentRepository> = Arc::new(OneRunningExperimentRepo {
            rows: vec![exp],
        });
        // Dispatcher whose trigger() always returns Err — must NOT
        // surface to caller.
        let dispatcher = Arc::new(CapturingDispatcher {
            calls: Mutex::new(Vec::new()),
            always_fail: true,
        });
        let dispatcher_arc: Arc<dyn RecomputeTrigger> = dispatcher.clone();

        let req = Request::new(UpdateMetricRequest {
            id: created.id.clone(),
            expected_version: 1,
            name: Some("renamed".into()),
            description: None,
            goal_direction: None,
            kind: Some(update_metric_request::Kind::Aggregation(
                ProtoAggregationConfig {
                    event_key: "checkout_completed".into(),
                    aggregator: "count".into(),
                    on_field: None,
                    where_clause_json: None,
                },
            )),
        });
        let resp =
            handle_update_metric(&repo, Some(exp_repo), Some(dispatcher_arc), req).await;
        // PATCH still succeeds despite the dispatcher's forced Err.
        let updated = resp.expect("update should succeed").into_inner();
        assert_eq!(updated.version, 2);

        // Verify the dispatch was attempted (and failed) — confirms
        // the side effect ran and the error path was exercised.
        for _ in 0..50 {
            if !dispatcher.calls().is_empty() {
                break;
            }
            tokio::task::yield_now().await;
        }
        assert_eq!(
            dispatcher.calls().len(),
            1,
            "dispatcher must have been invoked (and failed) without aborting the update"
        );
    }
}
