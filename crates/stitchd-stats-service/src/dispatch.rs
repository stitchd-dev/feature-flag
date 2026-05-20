//! Metric query dispatcher — routes a [`MetricDefinition`] to the
//! correct kind-specific query builder.
//!
//! The dispatcher is the single seam between the scheduler (which only
//! knows it needs a SQL query for some metric ID in some iteration) and
//! the pure per-kind builders under [`crate::queries`]. Two
//! responsibilities live here:
//!
//! 1. **Kind-based routing.** `Aggregation` and `Funnel` metrics are
//!    fully self-describing — the dispatcher just hands their config to
//!    the matching builder. `Ratio` metrics carry only two referenced
//!    `MetricId`s, so the dispatcher resolves both via
//!    [`MetricRepository::find_batch_by_ids`] and asserts they are
//!    `Aggregation` (the only legal denominator/numerator shape).
//!
//! 2. **Placeholder rewriting.** The pure builders emit `{p0}, {p1}, …`
//!    so they remain dialect-agnostic and unit-testable. The dispatcher
//!    converts those to `?` for `clickhouse-rs`, preserving bind order
//!    so the parallel `Vec<QueryBind>` continues to align positionally.
//!
//! The dispatcher itself performs no I/O against ClickHouse — it only
//! reads metric definitions from Postgres. The scheduler is responsible
//! for binding values and executing the query.
//!
//! [`MetricDefinition`]: stitchd_core::metric::MetricDefinition
//! [`MetricRepository::find_batch_by_ids`]: stitchd_db::repository::MetricRepository::find_batch_by_ids

use stitchd_core::metric::{AggregationConfig, MetricDefinition, MetricKind};
use stitchd_db::{RepositoryError, repository::MetricRepository};
use thiserror::Error;

use crate::queries::{
    BuiltQuery, QueryBuildError, aggregation::build_aggregation_query,
    funnel::build_funnel_query, ratio::build_ratio_query,
};

// ── DispatchError ────────────────────────────────────────────────────────────

/// Errors returned by [`dispatch_metric_query`].
///
/// The dispatcher fails in four situations:
///
/// 1. A `Ratio` metric references a non-aggregation metric (either side
///    is itself a `Ratio` or `Funnel`). This is structurally invalid —
///    `RatioConfig::validate` only enforces that the two sides are
///    distinct, not their kinds; the kind invariant lives here because
///    it depends on a repository lookup.
/// 2. A `Ratio` side resolves to no row in the repository (deleted
///    underneath us, or never existed).
/// 3. The downstream query builder rejected the config — propagated
///    verbatim from [`QueryBuildError`].
/// 4. The repository itself failed — propagated verbatim from
///    [`RepositoryError`].
#[derive(Debug, Error)]
pub enum DispatchError {
    /// A `Ratio` metric references a non-aggregation metric on one of
    /// its sides.
    #[error(
        "ratio metric {metric_id} references non-aggregation metric {referenced} (kind: {kind})"
    )]
    InvalidRatioMetric {
        /// String form of the owning ratio metric's ID.
        metric_id: String,
        /// String form of the offending referenced metric's ID.
        referenced: String,
        /// Short tag of the offending referenced metric's kind (e.g.
        /// `"funnel"`, `"ratio"`).
        kind: String,
    },

    /// A referenced metric (numerator or denominator) was not found in
    /// the repository — likely soft-deleted or never persisted.
    #[error("metric {0} not found")]
    MetricNotFound(String),

    /// The kind-specific query builder rejected the config.
    #[error("query build error: {0}")]
    QueryBuild(#[from] QueryBuildError),

    /// The metric repository returned an error.
    #[error("repository error: {0}")]
    Repository(#[from] RepositoryError),
}

// ── dispatch_metric_query ────────────────────────────────────────────────────

/// Route a metric definition to its kind-specific query builder and
/// return the resulting [`BuiltQuery`].
///
/// `experiment_id`, `iteration_id`, `env_id` and `variant_keys` are
/// forwarded verbatim to every builder. The dispatcher's only
/// kind-specific work is for [`MetricKind::Ratio`], where it resolves
/// both referenced metrics via the supplied repository and asserts
/// they are themselves `Aggregation` metrics.
///
/// # Errors
/// Returns [`DispatchError`] for invalid ratio references, missing
/// metrics, repository failures, or downstream query-build failures.
/// See the variant docs for the full taxonomy.
pub async fn dispatch_metric_query(
    metric: &MetricDefinition,
    metric_repo: &dyn MetricRepository,
    experiment_id: &str,
    iteration_id: &str,
    env_id: &str,
    variant_keys: &[&str],
) -> Result<BuiltQuery, DispatchError> {
    match &metric.kind {
        MetricKind::Aggregation(cfg) => Ok(build_aggregation_query(
            cfg,
            experiment_id,
            iteration_id,
            env_id,
            variant_keys,
        )?),
        MetricKind::Funnel(cfg) => Ok(build_funnel_query(
            cfg,
            experiment_id,
            iteration_id,
            env_id,
            variant_keys,
        )?),
        MetricKind::Ratio(cfg) => {
            // Resolve both referenced metrics in a single batch so the
            // dispatcher does one round-trip per ratio instead of two.
            let resolved = metric_repo
                .find_batch_by_ids(&[cfg.numerator_metric_id, cfg.denominator_metric_id])
                .await?;

            // Look up each side by ID rather than relying on the
            // repository to preserve input order — `find_batch_by_ids`
            // semantics are "the matching rows, in whatever order PG
            // returned them", and soft-deletes are filtered out so a
            // missing row can mean either deleted or never existed.
            let numerator_def = resolved
                .iter()
                .find(|m| m.id == cfg.numerator_metric_id)
                .ok_or_else(|| DispatchError::MetricNotFound(cfg.numerator_metric_id.to_string()))?;
            let denominator_def = resolved
                .iter()
                .find(|m| m.id == cfg.denominator_metric_id)
                .ok_or_else(|| {
                    DispatchError::MetricNotFound(cfg.denominator_metric_id.to_string())
                })?;

            let numerator_cfg = extract_aggregation_config(numerator_def, &metric.id.to_string())?;
            let denominator_cfg =
                extract_aggregation_config(denominator_def, &metric.id.to_string())?;

            Ok(build_ratio_query(
                cfg,
                numerator_cfg,
                denominator_cfg,
                experiment_id,
                iteration_id,
                env_id,
                variant_keys,
            )?)
        }
    }
}

/// Extract the [`AggregationConfig`] from a [`MetricDefinition`] or
/// return [`DispatchError::InvalidRatioMetric`] if the referenced
/// metric is not an aggregation.
///
/// `owner_id` is the ID of the *ratio* metric making the reference —
/// included in the error so debugging "which ratio is broken?" doesn't
/// require a second lookup.
fn extract_aggregation_config<'a>(
    def: &'a MetricDefinition,
    owner_id: &str,
) -> Result<&'a AggregationConfig, DispatchError> {
    match &def.kind {
        MetricKind::Aggregation(cfg) => Ok(cfg),
        other => Err(DispatchError::InvalidRatioMetric {
            metric_id: owner_id.to_owned(),
            referenced: def.id.to_string(),
            kind: other.tag().to_owned(),
        }),
    }
}

// ── rewrite_placeholders_to_clickhouse ───────────────────────────────────────

/// Rewrite every `{pN}` placeholder in `sql` to `?` for `clickhouse-rs`,
/// preserving left-to-right order.
///
/// The pure query builders emit positional placeholders of the form
/// `{p0}, {p1}, …, {p<n>}` and a parallel `Vec<QueryBind>` whose index
/// matches each placeholder's number. `clickhouse-rs::Client::query`
/// uses anonymous `?` placeholders and binds values in the order
/// `.bind()` is called — so the dispatcher rewrites the SQL but keeps
/// the bind vector untouched and the scheduler iterates over it in
/// order.
///
/// Multi-digit placeholder numbers (`{p10}`, `{p123}`) are supported.
/// Non-placeholder content is copied through verbatim — `'{plain text}'`
/// inside string literals would only collide if it starts with `{p`
/// followed by digits, which is intentionally rare in our generated SQL
/// and never appears in user-supplied event keys (those are bound, not
/// inlined).
#[must_use]
pub fn rewrite_placeholders_to_clickhouse(sql: String) -> String {
    // Walk the string once; any time we see `{p<digits>}`, emit `?`.
    // Anything else is copied byte-for-byte.
    let bytes = sql.as_bytes();
    let mut out = String::with_capacity(sql.len());
    let mut i = 0;
    while i < bytes.len() {
        // Look for the opening `{p`.
        if bytes[i] == b'{' && i + 1 < bytes.len() && bytes[i + 1] == b'p' {
            // Scan past the digits.
            let mut j = i + 2;
            while j < bytes.len() && bytes[j].is_ascii_digit() {
                j += 1;
            }
            // We need at least one digit AND a closing `}` for this to
            // count as a placeholder.
            if j > i + 2 && j < bytes.len() && bytes[j] == b'}' {
                out.push('?');
                i = j + 1;
                continue;
            }
        }
        // Non-placeholder byte — copy and advance.
        // Safe: bytes came from a UTF-8 String and we only branch on
        // ASCII codepoints, so character boundaries are preserved.
        out.push(bytes[i] as char);
        i += 1;
    }
    out
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use chrono::Utc;
    use std::collections::HashMap;
    use stitchd_core::id::{EnvironmentId, MetricId};
    use stitchd_core::metric::{
        AggregationConfig, AggregationOperator, FunnelConfig, FunnelStep, GoalDirection,
        MetricDefinition, MetricKind, RatioConfig,
    };

    const ENV_ID: &str = "00000000-0000-0000-0000-000000000001";
    const EXP_ID: &str = "00000000-0000-0000-0000-000000000002";
    const ITER_ID: &str = "00000000-0000-0000-0000-000000000003";

    // ── MockMetricRepository ──────────────────────────────────────────────────

    /// Hand-rolled mock — `mockall` is not a workspace dev-dep here, and
    /// only `find_batch_by_ids` is exercised by the dispatcher tests, so
    /// every other trait method just panics.
    struct MockMetricRepository {
        by_id: HashMap<MetricId, MetricDefinition>,
    }

    impl MockMetricRepository {
        fn new() -> Self {
            Self {
                by_id: HashMap::new(),
            }
        }

        fn with(mut self, m: MetricDefinition) -> Self {
            self.by_id.insert(m.id, m);
            self
        }
    }

    #[async_trait]
    impl MetricRepository for MockMetricRepository {
        async fn find_by_id(
            &self,
            _id: MetricId,
        ) -> Result<MetricDefinition, RepositoryError> {
            unimplemented!("dispatcher tests do not exercise find_by_id")
        }

        async fn find_by_key(
            &self,
            _key: &str,
            _environment_id: EnvironmentId,
        ) -> Result<MetricDefinition, RepositoryError> {
            unimplemented!("dispatcher tests do not exercise find_by_key")
        }

        async fn find_batch_by_ids(
            &self,
            ids: &[MetricId],
        ) -> Result<Vec<MetricDefinition>, RepositoryError> {
            let mut out = Vec::with_capacity(ids.len());
            for id in ids {
                if let Some(m) = self.by_id.get(id) {
                    out.push(m.clone());
                }
            }
            Ok(out)
        }

        async fn list_by_environment(
            &self,
            _environment_id: EnvironmentId,
        ) -> Result<Vec<MetricDefinition>, RepositoryError> {
            unimplemented!("dispatcher tests do not exercise list_by_environment")
        }

        async fn list_by_environment_paginated(
            &self,
            _environment_id: EnvironmentId,
            _offset: u64,
            _limit: u64,
        ) -> Result<(Vec<MetricDefinition>, u64), RepositoryError> {
            unimplemented!("dispatcher tests do not exercise list_by_environment_paginated")
        }

        async fn create(&self, _metric: &MetricDefinition) -> Result<(), RepositoryError> {
            unimplemented!("dispatcher tests do not exercise create")
        }

        async fn update(
            &self,
            _metric: &MetricDefinition,
        ) -> Result<MetricDefinition, RepositoryError> {
            unimplemented!("dispatcher tests do not exercise update")
        }

        async fn soft_delete(&self, _id: MetricId) -> Result<(), RepositoryError> {
            unimplemented!("dispatcher tests do not exercise soft_delete")
        }
    }

    // ── Builders for fixture metric definitions ──────────────────────────────

    fn make_metric(id: MetricId, kind: MetricKind) -> MetricDefinition {
        let now = Utc::now();
        MetricDefinition {
            id,
            environment_id: EnvironmentId::new(),
            key: format!("metric_{id}"),
            name: "Test metric".into(),
            description: None,
            kind,
            goal_direction: GoalDirection::Increase,
            version: 1,
            created_at: now,
            updated_at: now,
            deleted_at: None,
        }
    }

    fn count_agg(event_key: &str) -> AggregationConfig {
        AggregationConfig {
            event_key: event_key.into(),
            aggregator: AggregationOperator::Count,
            on_field: None,
            where_clause: None,
        }
    }

    fn variant_keys() -> Vec<&'static str> {
        vec!["control", "treatment"]
    }

    // ── Aggregation dispatch ─────────────────────────────────────────────────

    #[tokio::test]
    async fn test_dispatch_aggregation_returns_aggregation_query() {
        let metric = make_metric(
            MetricId::new(),
            MetricKind::Aggregation(count_agg("checkout_completed")),
        );
        let repo = MockMetricRepository::new();
        let vks = variant_keys();

        let q = dispatch_metric_query(&metric, &repo, EXP_ID, ITER_ID, ENV_ID, &vks)
            .await
            .expect("aggregation dispatch should succeed");

        // The aggregation builder's SQL always starts with `SELECT`
        // (no leading CTEs) and references `events_v2`.
        assert!(q.sql.starts_with("SELECT"), "got SQL: {}", q.sql);
        assert!(q.sql.contains("FROM events_v2"), "got SQL: {}", q.sql);
        // First three binds are env / experiment / iteration scope.
        assert!(q.binds.len() >= 3);
    }

    // ── Funnel dispatch ──────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_dispatch_funnel_returns_funnel_query() {
        let metric = make_metric(
            MetricId::new(),
            MetricKind::Funnel(FunnelConfig {
                steps: vec![
                    FunnelStep {
                        event_key: "view".into(),
                        where_clause: None,
                    },
                    FunnelStep {
                        event_key: "purchase".into(),
                        where_clause: None,
                    },
                ],
                window_seconds: 3600,
                count_repeats: false,
            }),
        );
        let repo = MockMetricRepository::new();
        let vks = variant_keys();

        let q = dispatch_metric_query(&metric, &repo, EXP_ID, ITER_ID, ENV_ID, &vks)
            .await
            .expect("funnel dispatch should succeed");

        // The funnel builder's SQL starts with `WITH levels AS (`.
        assert!(
            q.sql.starts_with("WITH levels AS"),
            "got SQL: {}",
            q.sql
        );
        // windowFunnel call is present.
        assert!(q.sql.contains("windowFunnel"), "got SQL: {}", q.sql);
    }

    // ── Ratio dispatch (happy path) ──────────────────────────────────────────

    #[tokio::test]
    async fn test_dispatch_ratio_resolves_both_sides_via_repo() {
        let num_id = MetricId::new();
        let den_id = MetricId::new();
        let numerator = make_metric(
            num_id,
            MetricKind::Aggregation(count_agg("checkout_completed")),
        );
        let denominator = make_metric(
            den_id,
            MetricKind::Aggregation(count_agg("checkout_started")),
        );

        let ratio_metric = make_metric(
            MetricId::new(),
            MetricKind::Ratio(RatioConfig {
                numerator_metric_id: num_id,
                denominator_metric_id: den_id,
                min_denominator: 100,
            }),
        );

        let repo = MockMetricRepository::new()
            .with(numerator)
            .with(denominator);
        let vks = variant_keys();

        let q = dispatch_metric_query(&ratio_metric, &repo, EXP_ID, ITER_ID, ENV_ID, &vks)
            .await
            .expect("ratio dispatch should succeed");

        // Both event keys should be bound in the final query.
        let str_binds: Vec<&str> = q
            .binds
            .iter()
            .filter_map(|b| b.as_str())
            .collect();
        assert!(
            str_binds.contains(&"checkout_completed"),
            "numerator event_key missing from binds: {str_binds:?}"
        );
        assert!(
            str_binds.contains(&"checkout_started"),
            "denominator event_key missing from binds: {str_binds:?}"
        );
        // Ratio SQL wraps the two legs in CTEs.
        assert!(q.sql.contains("WITH numerator AS"), "got SQL: {}", q.sql);
        assert!(
            q.sql.contains("denominator AS"),
            "got SQL: {}",
            q.sql
        );
    }

    // ── Ratio dispatch: non-aggregation side ─────────────────────────────────

    #[tokio::test]
    async fn test_dispatch_ratio_with_non_aggregation_side_returns_error() {
        let num_id = MetricId::new();
        let den_id = MetricId::new();
        // Numerator is itself a Ratio — illegal.
        let bad_numerator = make_metric(
            num_id,
            MetricKind::Ratio(RatioConfig {
                numerator_metric_id: MetricId::new(),
                denominator_metric_id: MetricId::new(),
                min_denominator: 0,
            }),
        );
        let denominator = make_metric(
            den_id,
            MetricKind::Aggregation(count_agg("checkout_started")),
        );

        let ratio_metric_id = MetricId::new();
        let ratio_metric = make_metric(
            ratio_metric_id,
            MetricKind::Ratio(RatioConfig {
                numerator_metric_id: num_id,
                denominator_metric_id: den_id,
                min_denominator: 0,
            }),
        );

        let repo = MockMetricRepository::new()
            .with(bad_numerator)
            .with(denominator);
        let vks = variant_keys();

        let err = dispatch_metric_query(&ratio_metric, &repo, EXP_ID, ITER_ID, ENV_ID, &vks)
            .await
            .expect_err("should reject non-aggregation numerator");

        match err {
            DispatchError::InvalidRatioMetric {
                metric_id,
                referenced,
                kind,
            } => {
                assert_eq!(metric_id, ratio_metric_id.to_string());
                assert_eq!(referenced, num_id.to_string());
                assert_eq!(kind, "ratio");
            }
            other => panic!("expected InvalidRatioMetric, got {other:?}"),
        }
    }

    // ── Ratio dispatch: missing side ─────────────────────────────────────────

    #[tokio::test]
    async fn test_dispatch_ratio_with_missing_side_returns_metric_not_found() {
        let num_id = MetricId::new();
        let den_id = MetricId::new();
        let ratio_metric = make_metric(
            MetricId::new(),
            MetricKind::Ratio(RatioConfig {
                numerator_metric_id: num_id,
                denominator_metric_id: den_id,
                min_denominator: 0,
            }),
        );

        // Empty repo — neither side resolves.
        let repo = MockMetricRepository::new();
        let vks = variant_keys();

        let err = dispatch_metric_query(&ratio_metric, &repo, EXP_ID, ITER_ID, ENV_ID, &vks)
            .await
            .expect_err("should report missing numerator");

        match err {
            DispatchError::MetricNotFound(id) => {
                // The numerator is looked up first, so it surfaces first.
                assert_eq!(id, num_id.to_string());
            }
            other => panic!("expected MetricNotFound, got {other:?}"),
        }
    }

    // ── rewrite_placeholders_to_clickhouse ───────────────────────────────────

    #[test]
    fn test_rewrite_placeholders_keeps_bind_order_and_handles_multi_digit() {
        // Single-digit + multi-digit placeholders in the same string,
        // mixed with non-placeholder content.
        let input = "SELECT {p0}, {p1}, {p10}".to_owned();
        assert_eq!(
            rewrite_placeholders_to_clickhouse(input),
            "SELECT ?, ?, ?"
        );

        // Bind ORDER is preserved — the rewrite is purely positional.
        let input = "WHERE a = {p2} AND b = {p0} AND c = {p1}".to_owned();
        assert_eq!(
            rewrite_placeholders_to_clickhouse(input),
            "WHERE a = ? AND b = ? AND c = ?"
        );

        // Tokens that look like placeholders but aren't (missing digits
        // or missing closing brace) are passed through verbatim.
        let input = "SELECT {p} AS x, {pq} AS y, {p3 AS z".to_owned();
        assert_eq!(
            rewrite_placeholders_to_clickhouse(input),
            "SELECT {p} AS x, {pq} AS y, {p3 AS z"
        );

        // Empty input.
        assert_eq!(rewrite_placeholders_to_clickhouse(String::new()), "");

        // No placeholders.
        assert_eq!(
            rewrite_placeholders_to_clickhouse("SELECT 1 + 1".into()),
            "SELECT 1 + 1"
        );
    }
}
