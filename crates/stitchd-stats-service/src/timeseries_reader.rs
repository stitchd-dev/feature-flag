//! Per-experiment, per-context-type, per-day timeseries reader.
//!
//! Powers the gateway's `GET /v1/environments/{env}/experiments/{id}/timeseries`
//! endpoint (Phase 7 Task 3). Each bucket is one `(day, variant_key, value)`
//! triple for a given metric, scoped to a single `context_type`.
//!
//! Abstracted behind the [`TimeseriesReader`] trait so the StatsService impl
//! can be unit-tested without standing up a real CH client + metric repo.
//! The production wiring lives in [`crate::grpc::timeseries`] (Task 7.3).

use async_trait::async_trait;
use uuid::Uuid;

/// One daily bucket of a timeseries — `(day, variant_key, value)`.
#[derive(Debug, Clone, PartialEq)]
pub struct TimeseriesBucket {
    /// RFC 3339 UTC day-start (00:00 UTC).
    pub day: String,
    pub variant_key: String,
    pub value: f64,
}

/// Trait over the per-metric daily aggregation query.
///
/// Production impl looks up the metric, dispatches to the experiment-scoped
/// preview query builder, executes against CH, and filters rows to the
/// requested `context_type`. Tests inject a stub returning canned buckets.
#[async_trait]
pub trait TimeseriesReader: Send + Sync + 'static {
    /// Fetch daily buckets for a metric scoped to `(experiment_id, context_type)`
    /// over the last `days` days.
    async fn get_timeseries(
        &self,
        experiment_id: Uuid,
        metric_id: Uuid,
        context_type: &str,
        days: u32,
    ) -> Result<Vec<TimeseriesBucket>, TimeseriesReaderError>;
}

/// Error variants the stats-service handler can branch on.
#[derive(Debug, thiserror::Error)]
pub enum TimeseriesReaderError {
    #[error("metric not found: {0}")]
    MetricNotFound(String),
    #[error("experiment iteration not found for experiment {0}")]
    IterationNotFound(String),
    #[error("invalid metric kind: {0}")]
    InvalidMetricKind(String),
    #[error("clickhouse error: {0}")]
    Clickhouse(String),
    #[error("internal error: {0}")]
    Internal(String),
}

// ─── Production impl ──────────────────────────────────────────────────────────

/// CH-backed implementation. Looks up the metric definition via
/// [`stitchd_db::MetricRepository`], finds the latest iteration of the
/// experiment via [`stitchd_db::ExperimentRepository`], and dispatches the
/// existing `build_experiment_preview_aggregation_query` builder against
/// `experiment_assignments + events`.
///
/// Only Aggregation metrics are supported in this first cut — Ratio + Funnel
/// raise `InvalidMetricKind`. The admin UI's Timeseries tab is currently
/// scoped to aggregation metrics per spec §5; broader kinds follow in
/// Phase 11.
pub struct ClickHouseTimeseriesReader {
    ch_client: std::sync::Arc<clickhouse::Client>,
    metric_repo: std::sync::Arc<dyn stitchd_db::MetricRepository>,
    experiment_repo: std::sync::Arc<dyn stitchd_db::ExperimentRepository>,
}

impl ClickHouseTimeseriesReader {
    /// Wire up a new reader.
    #[must_use]
    pub fn new(
        ch_client: std::sync::Arc<clickhouse::Client>,
        metric_repo: std::sync::Arc<dyn stitchd_db::MetricRepository>,
        experiment_repo: std::sync::Arc<dyn stitchd_db::ExperimentRepository>,
    ) -> Self {
        Self {
            ch_client,
            metric_repo,
            experiment_repo,
        }
    }
}

#[derive(Debug, Clone, serde::Deserialize, clickhouse::Row)]
struct CHTimeseriesRow {
    /// Epoch seconds at day-start UTC.
    day_ts: u32,
    context_type: String,
    variant_key: String,
    value: Option<f64>,
}

#[async_trait]
impl TimeseriesReader for ClickHouseTimeseriesReader {
    async fn get_timeseries(
        &self,
        experiment_id: Uuid,
        metric_id: Uuid,
        context_type: &str,
        _days: u32,
    ) -> Result<Vec<TimeseriesBucket>, TimeseriesReaderError> {
        use stitchd_core::metric::MetricKind;

        // Look up the metric — must be Aggregation kind.
        let metric = self
            .metric_repo
            .find_by_id(stitchd_core::id::MetricId::from_uuid(metric_id))
            .await
            .map_err(|e| match e {
                stitchd_db::RepositoryError::NotFound { id } => {
                    TimeseriesReaderError::MetricNotFound(id)
                }
                other => TimeseriesReaderError::Internal(other.to_string()),
            })?;

        let agg_cfg = match &metric.kind {
            MetricKind::Aggregation(cfg) => cfg.clone(),
            MetricKind::Ratio(_) => {
                return Err(TimeseriesReaderError::InvalidMetricKind(
                    "ratio metric kinds are not yet supported on the timeseries endpoint"
                        .to_string(),
                ));
            }
            MetricKind::Funnel(_) => {
                return Err(TimeseriesReaderError::InvalidMetricKind(
                    "funnel metric kinds are not yet supported on the timeseries endpoint"
                        .to_string(),
                ));
            }
        };

        // Find the latest iteration of the experiment.
        let exp_id = stitchd_core::id::ExperimentId::from_uuid(experiment_id);
        let iterations =
            self.experiment_repo
                .list_iterations(exp_id)
                .await
                .map_err(|e| match e {
                    stitchd_db::RepositoryError::NotFound { id } => {
                        TimeseriesReaderError::IterationNotFound(id)
                    }
                    other => TimeseriesReaderError::Internal(other.to_string()),
                })?;
        let latest = iterations
            .into_iter()
            .max_by_key(|i| i.started_at)
            .ok_or_else(|| TimeseriesReaderError::IterationNotFound(experiment_id.to_string()))?;
        let iteration_end = latest.ended_at.unwrap_or_else(chrono::Utc::now);
        let env_id_str = metric.environment_id.as_uuid().to_string();

        // Build the daily-bucket query.
        let built = crate::queries::preview::build_experiment_preview_aggregation_query(
            &agg_cfg,
            &experiment_id.to_string(),
            &latest.id.to_string(),
            &env_id_str,
            iteration_end,
        )
        .map_err(|e| TimeseriesReaderError::Internal(e.to_string()))?;

        // Run against CH.
        let sql = crate::dispatch::rewrite_placeholders_to_clickhouse(built.sql);
        let mut query = self.ch_client.query(&sql);
        for bind in &built.binds {
            query = match bind {
                crate::queries::QueryBind::Str(s) => query.bind(s),
                crate::queries::QueryBind::I64(n) => query.bind(*n),
                crate::queries::QueryBind::F64(f) => query.bind(*f),
            };
        }
        let rows: Vec<CHTimeseriesRow> = query
            .fetch_all()
            .await
            .map_err(|e| TimeseriesReaderError::Clickhouse(e.to_string()))?;

        // Filter to the requested context_type + convert.
        let buckets: Vec<TimeseriesBucket> = rows
            .into_iter()
            .filter(|r| r.context_type == context_type)
            .map(|r| {
                let day = chrono::DateTime::<chrono::Utc>::from_timestamp(i64::from(r.day_ts), 0)
                    .map(|d| d.to_rfc3339())
                    .unwrap_or_default();
                TimeseriesBucket {
                    day,
                    variant_key: r.variant_key,
                    value: r.value.unwrap_or(0.0),
                }
            })
            .collect();

        Ok(buckets)
    }
}

#[cfg(test)]
pub mod testing {
    use super::*;
    use std::sync::{Arc, Mutex};

    /// `(experiment_id, metric_id, context_type, days)` captured per call.
    pub type TimeseriesCall = (Uuid, Uuid, String, u32);

    /// Canned-response stub used in StatsService unit tests.
    pub struct StubTimeseriesReader {
        pub calls: Arc<Mutex<Vec<TimeseriesCall>>>,
        pub result: Result<Vec<TimeseriesBucket>, TimeseriesReaderError>,
    }

    impl StubTimeseriesReader {
        pub fn with_rows(rows: Vec<TimeseriesBucket>) -> Self {
            Self {
                calls: Arc::new(Mutex::new(Vec::new())),
                result: Ok(rows),
            }
        }

        pub fn with_err(err: TimeseriesReaderError) -> Self {
            Self {
                calls: Arc::new(Mutex::new(Vec::new())),
                result: Err(err),
            }
        }
    }

    #[async_trait]
    impl TimeseriesReader for StubTimeseriesReader {
        async fn get_timeseries(
            &self,
            experiment_id: Uuid,
            metric_id: Uuid,
            context_type: &str,
            days: u32,
        ) -> Result<Vec<TimeseriesBucket>, TimeseriesReaderError> {
            self.calls.lock().unwrap().push((
                experiment_id,
                metric_id,
                context_type.to_string(),
                days,
            ));
            match &self.result {
                Ok(rows) => Ok(rows.clone()),
                Err(e) => Err(match e {
                    TimeseriesReaderError::MetricNotFound(s) => {
                        TimeseriesReaderError::MetricNotFound(s.clone())
                    }
                    TimeseriesReaderError::IterationNotFound(s) => {
                        TimeseriesReaderError::IterationNotFound(s.clone())
                    }
                    TimeseriesReaderError::InvalidMetricKind(s) => {
                        TimeseriesReaderError::InvalidMetricKind(s.clone())
                    }
                    TimeseriesReaderError::Clickhouse(s) => {
                        TimeseriesReaderError::Clickhouse(s.clone())
                    }
                    TimeseriesReaderError::Internal(s) => {
                        TimeseriesReaderError::Internal(s.clone())
                    }
                }),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::testing::StubTimeseriesReader;
    use super::*;

    #[tokio::test]
    async fn stub_reader_records_invocation_and_returns_rows() {
        let bucket = TimeseriesBucket {
            day: "2026-05-21T00:00:00Z".to_string(),
            variant_key: "control".to_string(),
            value: 42.0,
        };
        let reader = StubTimeseriesReader::with_rows(vec![bucket.clone()]);
        let exp = Uuid::new_v4();
        let metric = Uuid::new_v4();
        let rows = reader
            .get_timeseries(exp, metric, "user", 7)
            .await
            .expect("rows");
        assert_eq!(rows, vec![bucket]);
        let calls = reader.calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].3, 7);
    }

    #[tokio::test]
    async fn stub_reader_propagates_error() {
        let reader = StubTimeseriesReader::with_err(TimeseriesReaderError::MetricNotFound(
            "abc".to_string(),
        ));
        let err = reader
            .get_timeseries(Uuid::new_v4(), Uuid::new_v4(), "user", 7)
            .await
            .unwrap_err();
        match err {
            TimeseriesReaderError::MetricNotFound(s) => assert_eq!(s, "abc"),
            other => panic!("unexpected variant: {other:?}"),
        }
    }
}
