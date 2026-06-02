//! Cross-experiment interaction orchestration.
//!
//! Ties together the building blocks merged in earlier phases into the full
//! compute-and-persist pipeline that runs on the 60-minute scheduler tick and
//! on the on-demand recompute path:
//!
//! 1. [`crate::interaction_pairs::candidate_pairs`] enumerates the experiment
//!    pairs worth testing (distinct flags, overlapping windows, shared metric,
//!    not same exclusion group).
//! 2. For each pair × shared metric × shared `context_type`, the per-cell metric
//!    query ([`crate::queries::interaction_metric`]) aggregates the
//!    `(a_variant, b_variant)` grid over the shared population.
//! 3. The cells are fed to the interaction significance math
//!    ([`stitchd_core::experimentation::stats::interaction`]) — `binary_*` for
//!    conversion metrics, `continuous_*` for numeric metrics.
//! 4. A row is written to the ClickHouse `experiment_interactions` table.
//!
//! Funnel and ratio metrics are out of scope for v1 — they are skipped with a
//! `tracing::debug!` log rather than erroring.
//!
//! ## Testability
//!
//! ClickHouse access is hidden behind two traits — [`InteractionCellReader`]
//! and [`InteractionWriter`] — so the orchestration logic
//! ([`compute_and_persist_interactions`]) is unit-testable with in-memory fakes.
//! The production wiring uses [`ClickHouseInteractionCells`] /
//! [`ClickHouseInteractionWriter`], both backed by a `clickhouse::Client`.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use clickhouse::Client;
use serde::Serialize;
use uuid::Uuid;

use stitchd_core::experimentation::Experiment;
use stitchd_core::experimentation::stats::interaction::{
    BinaryCell, ContinuousCell, InteractionResult, binary_interaction, continuous_interaction,
};
use stitchd_core::metric::{MetricDefinition, MetricKind};
use stitchd_db::repository::MetricRepository;

use crate::dispatch::rewrite_placeholders_to_clickhouse;
use crate::interaction_pairs::{ExperimentMeta, candidate_pairs};
use crate::queries::QueryBind;
use crate::queries::interaction_metric::{MetricCellKind, build_interaction_metric_cells_query};

/// One per-(a_variant, b_variant) metric cell as returned by the metric-cells
/// query. Carries both conversion and continuous statistics; the caller reads
/// only the columns relevant to the metric kind.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CellAggregate {
    /// A-experiment variant key.
    pub a_variant_key: String,
    /// B-experiment variant key.
    pub b_variant_key: String,
    /// Number of shared contexts in this cell.
    pub n: u64,
    /// Shared contexts in this cell with ≥1 qualifying event (conversion).
    pub successes: u64,
    /// Σ per-context metric value (continuous).
    pub value_sum: f64,
    /// Σ per-context metric value² (continuous).
    pub value_sq_sum: f64,
}

/// One computed interaction row, ready to persist to `experiment_interactions`.
#[derive(Debug, Clone, PartialEq)]
pub struct InteractionRow {
    /// Environment UUID.
    pub env_id: Uuid,
    /// First experiment of the (sorted) pair.
    pub experiment_id_a: Uuid,
    /// Second experiment of the pair.
    pub experiment_id_b: Uuid,
    /// Context dimension the interaction was computed over.
    pub context_type: String,
    /// Shared metric key.
    pub metric_key: String,
    /// Total shared contexts across all cells.
    pub shared_count: u64,
    /// JSON of the per-cell stats (the `CellAggregate` grid).
    pub cell_stats: String,
    /// Interaction effect-size estimate (`NaN` when insufficient data).
    pub interaction_estimate: f64,
    /// Two-sided p-value (`NaN` when insufficient data).
    pub p_value: f64,
    /// Whether the interaction is significant at alpha = 0.05.
    pub significant: bool,
    /// When the row was computed.
    pub computed_at: DateTime<Utc>,
}

// ── Reader / writer traits ───────────────────────────────────────────────────

/// Reads the per-cell metric grid for one (pair, metric, context_type) from
/// ClickHouse.
#[async_trait]
pub trait InteractionCellReader: Send + Sync {
    /// Fetch the `(a_variant, b_variant)` metric cells for the shared population
    /// of `(exp_a, exp_b)` within `env_id`, restricted to `context_type` and the
    /// supplied aggregation metric.
    async fn fetch_cells(
        &self,
        cfg: &stitchd_core::metric::AggregationConfig,
        env_id: &str,
        exp_a: &str,
        exp_b: &str,
        context_type: &str,
        interaction_end: DateTime<Utc>,
    ) -> Result<Vec<CellAggregate>, anyhow::Error>;
}

/// Persists computed interaction rows to ClickHouse.
#[async_trait]
pub trait InteractionWriter: Send + Sync {
    /// Write one interaction row to `experiment_interactions`.
    async fn write_row(&self, row: &InteractionRow) -> Result<(), anyhow::Error>;
}

// ── Pure orchestration ───────────────────────────────────────────────────────

/// Compute and persist interaction rows for the given environment's running /
/// recently-active experiments.
///
/// `metrics` maps every metric UUID referenced by an experiment in
/// `experiments` to its definition (the caller resolves these in one batch).
/// Metrics absent from the map, or of a non-aggregation kind (funnel / ratio),
/// are skipped.
///
/// `context_types` is the set of context dimensions to compute over — typically
/// the union of the two experiments' unit context types; the caller passes the
/// dimensions it cares about (e.g. `["user"]`).
///
/// Returns the number of rows written. Same-group / same-flag / non-overlapping
/// / no-shared-metric pairs are already excluded by [`candidate_pairs`], so this
/// never writes a row for them.
///
/// # Errors
/// Propagates the first reader / writer error encountered. A per-cell read that
/// returns an empty grid is skipped (no shared population on that context_type —
/// nothing to persist).
pub async fn compute_and_persist_interactions(
    reader: &dyn InteractionCellReader,
    writer: &dyn InteractionWriter,
    env_id: Uuid,
    experiments: &[ExperimentMeta],
    metrics: &HashMap<Uuid, MetricDefinition>,
    context_types: &[String],
    computed_at: DateTime<Utc>,
) -> Result<usize, anyhow::Error> {
    let pairs = candidate_pairs(experiments);
    let by_id: HashMap<Uuid, &ExperimentMeta> = experiments.iter().map(|e| (e.id, e)).collect();
    let env_str = env_id.to_string();

    let mut written = 0usize;
    for (exp_a, exp_b) in pairs {
        let (Some(a), Some(b)) = (by_id.get(&exp_a), by_id.get(&exp_b)) else {
            continue;
        };

        // The overlap-window upper bound: earliest of the two end times, or
        // "now" when both are still running. candidate_pairs already proved the
        // windows overlap.
        let interaction_end = overlap_end(a.ended_at, b.ended_at, computed_at);

        // Shared metrics — intersection of the two experiments' metric sets.
        let shared_metric_ids: Vec<Uuid> = a
            .metric_ids
            .iter()
            .filter(|m| b.metric_ids.contains(m))
            .copied()
            .collect();

        for metric_id in shared_metric_ids {
            let Some(def) = metrics.get(&metric_id) else {
                tracing::debug!(%metric_id, "interaction: metric definition not resolved; skipping");
                continue;
            };
            let MetricKind::Aggregation(cfg) = &def.kind else {
                tracing::debug!(
                    %metric_id,
                    kind = def.kind.tag(),
                    "interaction: skipping non-aggregation metric (funnel/ratio out of scope v1)"
                );
                continue;
            };
            let cell_kind = MetricCellKind::classify(cfg);

            for context_type in context_types {
                let cells = reader
                    .fetch_cells(
                        cfg,
                        &env_str,
                        &exp_a.to_string(),
                        &exp_b.to_string(),
                        context_type,
                        interaction_end,
                    )
                    .await?;

                if cells.is_empty() {
                    continue; // no shared population on this context_type
                }

                let result = compute_result(cell_kind, &cells);
                let shared_count: u64 = cells.iter().map(|c| c.n).sum();
                let cell_stats = serde_json::to_string(&cells)
                    .unwrap_or_else(|_| "[]".to_owned());

                let row = InteractionRow {
                    env_id,
                    experiment_id_a: exp_a,
                    experiment_id_b: exp_b,
                    context_type: context_type.clone(),
                    metric_key: def.key.clone(),
                    shared_count,
                    cell_stats,
                    interaction_estimate: nan_to_zero(result.estimate),
                    p_value: nan_to_zero(result.p_value),
                    significant: result.significant,
                    computed_at,
                };
                writer.write_row(&row).await?;
                written += 1;
            }
        }
    }
    Ok(written)
}

/// Top-level sweep: compute and persist interactions for every environment
/// represented in `running` (the currently-running experiments).
///
/// Groups the running experiments by environment, projects each into an
/// [`ExperimentMeta`], resolves every referenced metric definition via
/// `metric_repo`, and runs [`compute_and_persist_interactions`] per environment.
/// The set of context types per environment is the union of the experiments'
/// `unit_context_types`.
///
/// `started_at` for each [`ExperimentMeta`] is taken from the experiment's
/// `created_at`, and `ended_at` is `None` (running experiments are still live) —
/// so [`candidate_pairs`]' window-overlap check treats every concurrently-running
/// pair as overlapping, which is correct.
///
/// Returns the total number of interaction rows written across all environments.
///
/// # Errors
/// Propagates the first metric-repo / reader / writer error encountered.
pub async fn run_interaction_sweep(
    reader: &dyn InteractionCellReader,
    writer: &dyn InteractionWriter,
    metric_repo: &dyn MetricRepository,
    running: &[Experiment],
    computed_at: DateTime<Utc>,
) -> Result<usize, anyhow::Error> {
    // Group running experiments by environment.
    let mut by_env: HashMap<Uuid, Vec<&Experiment>> = HashMap::new();
    for exp in running {
        by_env
            .entry(exp.environment_id.as_uuid())
            .or_default()
            .push(exp);
    }

    let mut total = 0usize;
    for (env_id, exps) in by_env {
        // Project to ExperimentMeta.
        let metas: Vec<ExperimentMeta> = exps
            .iter()
            .map(|e| ExperimentMeta {
                id: e.id.as_uuid(),
                flag_id: e.flag_id.as_uuid(),
                started_at: e.created_at,
                ended_at: None,
                metric_ids: e.metric_ids.iter().map(stitchd_core::id::MetricId::as_uuid).collect(),
                exclusion_group_id: e.exclusion_group_id.map(|g| g.as_uuid()),
            })
            .collect();

        // Resolve all referenced metric definitions in one batch.
        let mut metric_ids: Vec<stitchd_core::id::MetricId> =
            exps.iter().flat_map(|e| e.metric_ids.iter().copied()).collect();
        metric_ids.sort_by_key(stitchd_core::id::MetricId::as_uuid);
        metric_ids.dedup();
        let defs = metric_repo.find_batch_by_ids(&metric_ids).await?;
        let metrics: HashMap<Uuid, MetricDefinition> =
            defs.into_iter().map(|m| (m.id.as_uuid(), m)).collect();

        // Union of context types across this environment's experiments.
        let mut context_types: Vec<String> = exps
            .iter()
            .flat_map(|e| e.unit_context_types.iter().cloned())
            .collect();
        context_types.sort();
        context_types.dedup();
        if context_types.is_empty() {
            context_types.push("user".to_string());
        }

        total += compute_and_persist_interactions(
            reader,
            writer,
            env_id,
            &metas,
            &metrics,
            &context_types,
            computed_at,
        )
        .await?;
    }
    Ok(total)
}

/// Run the interaction significance test appropriate to the metric kind over a
/// per-cell grid, mapping variant keys to dense zero-based level indices.
#[must_use]
pub fn compute_result(kind: MetricCellKind, cells: &[CellAggregate]) -> InteractionResult {
    // Dense level indexing: assign each distinct variant key a stable
    // zero-based index in first-seen order, separately per factor. The
    // interaction math derives grid dimensions from the max level index, so the
    // indices must be contiguous from 0.
    let mut a_idx: HashMap<String, usize> = HashMap::new();
    let mut b_idx: HashMap<String, usize> = HashMap::new();
    for c in cells {
        let n_a = a_idx.len();
        a_idx.entry(c.a_variant_key.clone()).or_insert(n_a);
        let n_b = b_idx.len();
        b_idx.entry(c.b_variant_key.clone()).or_insert(n_b);
    }

    match kind {
        MetricCellKind::Conversion => {
            let binary: Vec<BinaryCell> = cells
                .iter()
                .map(|c| BinaryCell {
                    a_level: a_idx[&c.a_variant_key],
                    b_level: b_idx[&c.b_variant_key],
                    n: c.n,
                    successes: c.successes,
                })
                .collect();
            binary_interaction(&binary)
        }
        MetricCellKind::Continuous => {
            let cont: Vec<ContinuousCell> = cells
                .iter()
                .map(|c| ContinuousCell {
                    a_level: a_idx[&c.a_variant_key],
                    b_level: b_idx[&c.b_variant_key],
                    n: c.n,
                    sum: c.value_sum,
                    sum_sq: c.value_sq_sum,
                })
                .collect();
            continuous_interaction(&cont)
        }
    }
}

/// Replace `NaN` (insufficient-data sentinel) with `0.0` so the value
/// round-trips cleanly through the ClickHouse `Float64` column.
fn nan_to_zero(v: f64) -> f64 {
    if v.is_nan() { 0.0 } else { v }
}

// ── ClickHouse-backed reader ─────────────────────────────────────────────────

/// Row shape deserialised from the metric-cells query.
#[derive(Debug, Clone, serde::Deserialize, clickhouse::Row)]
struct ChCellRow {
    a_variant_key: String,
    b_variant_key: String,
    n: u64,
    successes: u64,
    value_sum: f64,
    value_sq_sum: f64,
}

/// Production [`InteractionCellReader`] backed by a `clickhouse::Client`.
pub struct ClickHouseInteractionCells {
    client: Arc<Client>,
}

impl ClickHouseInteractionCells {
    /// Wrap a shared CH client.
    #[must_use]
    pub fn new(client: Arc<Client>) -> Self {
        Self { client }
    }
}

#[async_trait]
impl InteractionCellReader for ClickHouseInteractionCells {
    async fn fetch_cells(
        &self,
        cfg: &stitchd_core::metric::AggregationConfig,
        env_id: &str,
        exp_a: &str,
        exp_b: &str,
        context_type: &str,
        interaction_end: DateTime<Utc>,
    ) -> Result<Vec<CellAggregate>, anyhow::Error> {
        let built = build_interaction_metric_cells_query(
            cfg,
            env_id,
            exp_a,
            exp_b,
            context_type,
            interaction_end,
        )?;
        let sql = rewrite_placeholders_to_clickhouse(built.sql);
        let mut query = self.client.query(&sql);
        for b in built.binds {
            query = match b {
                QueryBind::Str(s) => query.bind(s),
                QueryBind::I64(n) => query.bind(n),
                QueryBind::F64(f) => query.bind(f),
            };
        }
        let rows = query.fetch_all::<ChCellRow>().await?;
        Ok(rows
            .into_iter()
            .map(|r| CellAggregate {
                a_variant_key: r.a_variant_key,
                b_variant_key: r.b_variant_key,
                n: r.n,
                successes: r.successes,
                value_sum: r.value_sum,
                value_sq_sum: r.value_sq_sum,
            })
            .collect())
    }
}

// ── ClickHouse-backed writer ─────────────────────────────────────────────────

/// Row shape inserted into the `experiment_interactions` table.
#[derive(Debug, Clone, Serialize, clickhouse::Row)]
struct ChInteractionRow {
    #[serde(with = "clickhouse::serde::uuid")]
    env_id: Uuid,
    #[serde(with = "clickhouse::serde::uuid")]
    experiment_id_a: Uuid,
    #[serde(with = "clickhouse::serde::uuid")]
    experiment_id_b: Uuid,
    context_type: String,
    metric_key: String,
    shared_count: u64,
    cell_stats: String,
    interaction_estimate: f64,
    p_value: f64,
    significant: bool,
    #[serde(with = "clickhouse::serde::chrono::datetime64::millis")]
    computed_at: DateTime<Utc>,
}

/// Production [`InteractionWriter`] backed by a `clickhouse::Client`.
pub struct ClickHouseInteractionWriter {
    client: Arc<Client>,
}

impl ClickHouseInteractionWriter {
    /// Wrap a shared CH client.
    #[must_use]
    pub fn new(client: Arc<Client>) -> Self {
        Self { client }
    }
}

#[async_trait]
impl InteractionWriter for ClickHouseInteractionWriter {
    async fn write_row(&self, row: &InteractionRow) -> Result<(), anyhow::Error> {
        let mut insert = self
            .client
            .insert::<ChInteractionRow>("experiment_interactions")
            .await?;
        insert
            .write(&ChInteractionRow {
                env_id: row.env_id,
                experiment_id_a: row.experiment_id_a,
                experiment_id_b: row.experiment_id_b,
                context_type: row.context_type.clone(),
                metric_key: row.metric_key.clone(),
                shared_count: row.shared_count,
                cell_stats: row.cell_stats.clone(),
                interaction_estimate: row.interaction_estimate,
                p_value: row.p_value,
                significant: row.significant,
                computed_at: row.computed_at,
            })
            .await?;
        insert.end().await?;
        Ok(())
    }
}

/// Earliest finite end among the two experiment windows, falling back to `now`
/// when both are open-ended (still running).
fn overlap_end(
    a_end: Option<DateTime<Utc>>,
    b_end: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
) -> DateTime<Utc> {
    match (a_end, b_end) {
        (Some(a), Some(b)) => a.min(b),
        (Some(a), None) => a.min(now),
        (None, Some(b)) => b.min(now),
        (None, None) => now,
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    use chrono::TimeZone;
    use stitchd_core::id::{EnvironmentId, MetricId};
    use stitchd_core::metric::{
        AggregationConfig, AggregationOperator, GoalDirection, MetricDefinition, MetricKind,
    };

    fn ts(day: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 5, day, 0, 0, 0).unwrap()
    }

    fn meta(id: Uuid, flag: Uuid, metric: Uuid) -> ExperimentMeta {
        ExperimentMeta {
            id,
            flag_id: flag,
            started_at: ts(1),
            ended_at: Some(ts(30)),
            metric_ids: vec![metric],
            exclusion_group_id: None,
        }
    }

    fn agg_metric(id: Uuid, key: &str, op: AggregationOperator) -> MetricDefinition {
        let now = Utc::now();
        MetricDefinition {
            id: MetricId::from_uuid(id),
            environment_id: EnvironmentId::new(),
            key: key.into(),
            name: key.into(),
            description: None,
            kind: MetricKind::Aggregation(AggregationConfig {
                event_key: format!("{key}_event"),
                aggregator: op,
                on_field: None,
                where_clause: None,
            }),
            goal_direction: GoalDirection::Increase,
            version: 1,
            created_at: now,
            updated_at: now,
            deleted_at: None,
        }
    }

    /// Fake reader returning canned cells; records the (env, exp_a, exp_b,
    /// context_type) it was asked for.
    struct FakeReader {
        cells: Vec<CellAggregate>,
        calls: Mutex<Vec<(String, String, String, String)>>,
    }

    #[async_trait]
    impl InteractionCellReader for FakeReader {
        async fn fetch_cells(
            &self,
            _cfg: &AggregationConfig,
            env_id: &str,
            exp_a: &str,
            exp_b: &str,
            context_type: &str,
            _interaction_end: DateTime<Utc>,
        ) -> Result<Vec<CellAggregate>, anyhow::Error> {
            self.calls.lock().unwrap().push((
                env_id.to_string(),
                exp_a.to_string(),
                exp_b.to_string(),
                context_type.to_string(),
            ));
            Ok(self.cells.clone())
        }
    }

    /// Fake writer capturing the rows written.
    #[derive(Default)]
    struct FakeWriter {
        rows: Mutex<Vec<InteractionRow>>,
    }

    #[async_trait]
    impl InteractionWriter for FakeWriter {
        async fn write_row(&self, row: &InteractionRow) -> Result<(), anyhow::Error> {
            self.rows.lock().unwrap().push(row.clone());
            Ok(())
        }
    }

    fn binary_cell(a: &str, b: &str, n: u64, succ: u64) -> CellAggregate {
        CellAggregate {
            a_variant_key: a.into(),
            b_variant_key: b.into(),
            n,
            successes: succ,
            value_sum: 0.0,
            value_sq_sum: 0.0,
        }
    }

    #[test]
    fn overlap_end_picks_earliest_finite_or_now() {
        let now = ts(20);
        assert_eq!(overlap_end(Some(ts(10)), Some(ts(15)), now), ts(10));
        assert_eq!(overlap_end(Some(ts(10)), None, now), ts(10));
        assert_eq!(overlap_end(None, Some(ts(25)), now), now);
        assert_eq!(overlap_end(None, None, now), now);
    }

    #[test]
    fn nan_to_zero_sanitizes() {
        assert_eq!(nan_to_zero(f64::NAN), 0.0);
        assert!((nan_to_zero(1.5) - 1.5).abs() < f64::EPSILON);
    }

    #[test]
    fn compute_result_maps_variant_keys_to_dense_levels() {
        // A planted strong interaction on a 2x2 conversion grid.
        let cells = vec![
            binary_cell("control", "control", 100, 10),
            binary_cell("control", "treatment", 100, 12),
            binary_cell("treatment", "control", 100, 14),
            binary_cell("treatment", "treatment", 100, 60),
        ];
        let r = compute_result(MetricCellKind::Conversion, &cells);
        assert!(!r.insufficient_data, "grid has ample data");
        assert!(r.significant, "planted super-additive cell should be significant");
    }

    #[tokio::test]
    async fn writes_a_row_for_a_valid_pair() {
        let metric = Uuid::new_v4();
        let env = Uuid::new_v4();
        let a = meta(Uuid::from_u128(1), Uuid::new_v4(), metric);
        let b = meta(Uuid::from_u128(2), Uuid::new_v4(), metric);
        let mut metrics = HashMap::new();
        metrics.insert(metric, agg_metric(metric, "checkout", AggregationOperator::Count));

        let reader = FakeReader {
            cells: vec![
                binary_cell("control", "control", 100, 10),
                binary_cell("control", "treatment", 100, 12),
                binary_cell("treatment", "control", 100, 14),
                binary_cell("treatment", "treatment", 100, 60),
            ],
            calls: Mutex::new(vec![]),
        };
        let writer = FakeWriter::default();

        let n = compute_and_persist_interactions(
            &reader,
            &writer,
            env,
            &[a, b],
            &metrics,
            &["user".to_string()],
            ts(15),
        )
        .await
        .unwrap();

        assert_eq!(n, 1);
        let rows = writer.rows.lock().unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].metric_key, "checkout");
        assert_eq!(rows[0].context_type, "user");
        assert_eq!(rows[0].shared_count, 400);
        assert!(rows[0].significant);
        assert!(rows[0].cell_stats.contains("a_variant_key"));
    }

    #[tokio::test]
    async fn skips_funnel_and_ratio_metrics() {
        let metric = Uuid::new_v4();
        let env = Uuid::new_v4();
        let a = meta(Uuid::from_u128(1), Uuid::new_v4(), metric);
        let b = meta(Uuid::from_u128(2), Uuid::new_v4(), metric);
        // Define the shared metric as a funnel — out of scope.
        let now = Utc::now();
        let funnel = MetricDefinition {
            id: MetricId::from_uuid(metric),
            environment_id: EnvironmentId::new(),
            key: "checkout_funnel".into(),
            name: "f".into(),
            description: None,
            kind: MetricKind::Funnel(stitchd_core::metric::FunnelConfig {
                steps: vec![
                    stitchd_core::metric::FunnelStep {
                        event_key: "view".into(),
                        where_clause: None,
                    },
                    stitchd_core::metric::FunnelStep {
                        event_key: "buy".into(),
                        where_clause: None,
                    },
                ],
                window_seconds: 3600,
                count_repeats: false,
            }),
            goal_direction: GoalDirection::Increase,
            version: 1,
            created_at: now,
            updated_at: now,
            deleted_at: None,
        };
        let mut metrics = HashMap::new();
        metrics.insert(metric, funnel);

        let reader = FakeReader {
            cells: vec![binary_cell("control", "control", 100, 10)],
            calls: Mutex::new(vec![]),
        };
        let writer = FakeWriter::default();

        let n = compute_and_persist_interactions(
            &reader,
            &writer,
            env,
            &[a, b],
            &metrics,
            &["user".to_string()],
            ts(15),
        )
        .await
        .unwrap();

        assert_eq!(n, 0, "funnel metric must be skipped");
        assert!(reader.calls.lock().unwrap().is_empty(), "no CH read for skipped metric");
        assert!(writer.rows.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn same_exclusion_group_pair_writes_nothing() {
        let metric = Uuid::new_v4();
        let env = Uuid::new_v4();
        let group = Uuid::new_v4();
        let mut a = meta(Uuid::from_u128(1), Uuid::new_v4(), metric);
        let mut b = meta(Uuid::from_u128(2), Uuid::new_v4(), metric);
        a.exclusion_group_id = Some(group);
        b.exclusion_group_id = Some(group);
        let mut metrics = HashMap::new();
        metrics.insert(metric, agg_metric(metric, "checkout", AggregationOperator::Count));

        let reader = FakeReader {
            cells: vec![binary_cell("control", "control", 100, 10)],
            calls: Mutex::new(vec![]),
        };
        let writer = FakeWriter::default();

        let n = compute_and_persist_interactions(
            &reader,
            &writer,
            env,
            &[a, b],
            &metrics,
            &["user".to_string()],
            ts(15),
        )
        .await
        .unwrap();

        assert_eq!(n, 0, "same-group pair excluded by candidate_pairs");
        assert!(reader.calls.lock().unwrap().is_empty());
        assert!(writer.rows.lock().unwrap().is_empty());
    }

    /// Minimal in-memory MetricRepository for the sweep test — only
    /// `find_batch_by_ids` is exercised.
    struct MapMetricRepo {
        by_id: HashMap<MetricId, MetricDefinition>,
    }

    #[async_trait]
    impl MetricRepository for MapMetricRepo {
        async fn find_by_id(
            &self,
            _id: MetricId,
        ) -> Result<MetricDefinition, stitchd_db::RepositoryError> {
            unimplemented!()
        }
        async fn find_by_key(
            &self,
            _key: &str,
            _environment_id: EnvironmentId,
        ) -> Result<MetricDefinition, stitchd_db::RepositoryError> {
            unimplemented!()
        }
        async fn find_batch_by_ids(
            &self,
            ids: &[MetricId],
        ) -> Result<Vec<MetricDefinition>, stitchd_db::RepositoryError> {
            Ok(ids
                .iter()
                .filter_map(|id| self.by_id.get(id).cloned())
                .collect())
        }
        async fn list_by_environment(
            &self,
            _environment_id: EnvironmentId,
        ) -> Result<Vec<MetricDefinition>, stitchd_db::RepositoryError> {
            unimplemented!()
        }
        async fn list_by_environment_paginated(
            &self,
            _environment_id: EnvironmentId,
            _offset: u64,
            _limit: u64,
        ) -> Result<(Vec<MetricDefinition>, u64), stitchd_db::RepositoryError> {
            unimplemented!()
        }
        async fn list_referencing_event(
            &self,
            _environment_id: EnvironmentId,
            _event_key: &str,
        ) -> Result<Vec<MetricDefinition>, stitchd_db::RepositoryError> {
            unimplemented!()
        }
        async fn create(
            &self,
            _metric: &MetricDefinition,
        ) -> Result<(), stitchd_db::RepositoryError> {
            unimplemented!()
        }
        async fn update(
            &self,
            _metric: &MetricDefinition,
        ) -> Result<MetricDefinition, stitchd_db::RepositoryError> {
            unimplemented!()
        }
        async fn soft_delete(&self, _id: MetricId) -> Result<(), stitchd_db::RepositoryError> {
            unimplemented!()
        }
    }

    fn running_experiment(env: EnvironmentId, flag: Uuid, metric: MetricId) -> Experiment {
        let now = Utc::now();
        Experiment {
            id: stitchd_core::id::ExperimentId::new(),
            environment_id: env,
            flag_id: stitchd_core::id::FlagId::from_uuid(flag),
            flag_key: None,
            variant_keys: vec![],
            flag_rule_id: Some(stitchd_core::id::RuleId::new()),
            targets_default_rule: false,
            name: "exp".into(),
            description: None,
            hypothesis: None,
            metric_ids: vec![metric],
            guardrail_metric_ids: vec![],
            traffic_allocation: 100.0,
            min_sample_size: None,
            pre_period_days: 0,
            unit_context_types: vec!["user".to_string()],
            scheduled_start_at: None,
            scheduled_end_at: None,
            status: stitchd_core::experimentation::ExperimentStatus::Running,
            created_at: now,
            updated_at: now,
            deleted_at: None,
            version: 1,
            exclusion_group_id: None,
            group_bucket_lo: None,
            group_bucket_hi: None,
        }
    }

    #[tokio::test]
    async fn sweep_groups_by_env_and_resolves_metrics() {
        let env = EnvironmentId::new();
        let metric_uuid = Uuid::new_v4();
        let metric_id = MetricId::from_uuid(metric_uuid);
        // Two running experiments in the same env on distinct flags sharing one
        // conversion metric → one candidate pair.
        let a = running_experiment(env, Uuid::new_v4(), metric_id);
        let b = running_experiment(env, Uuid::new_v4(), metric_id);

        let mut by_id = HashMap::new();
        by_id.insert(
            metric_id,
            agg_metric(metric_uuid, "checkout", AggregationOperator::Count),
        );
        let repo = MapMetricRepo { by_id };

        let reader = FakeReader {
            cells: vec![
                binary_cell("control", "control", 100, 10),
                binary_cell("control", "treatment", 100, 12),
                binary_cell("treatment", "control", 100, 14),
                binary_cell("treatment", "treatment", 100, 60),
            ],
            calls: Mutex::new(vec![]),
        };
        let writer = FakeWriter::default();

        let n = run_interaction_sweep(&reader, &writer, &repo, &[a, b], Utc::now())
            .await
            .unwrap();
        assert_eq!(n, 1, "one (pair, metric, context_type) row written");
        let rows = writer.rows.lock().unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].env_id, env.as_uuid());
        assert_eq!(rows[0].context_type, "user");
        assert!(rows[0].significant);
    }

    #[tokio::test]
    async fn sweep_does_not_pair_experiments_across_environments() {
        let env_a = EnvironmentId::new();
        let env_b = EnvironmentId::new();
        let metric_uuid = Uuid::new_v4();
        let metric_id = MetricId::from_uuid(metric_uuid);
        let a = running_experiment(env_a, Uuid::new_v4(), metric_id);
        let b = running_experiment(env_b, Uuid::new_v4(), metric_id);

        let mut by_id = HashMap::new();
        by_id.insert(
            metric_id,
            agg_metric(metric_uuid, "checkout", AggregationOperator::Count),
        );
        let repo = MapMetricRepo { by_id };

        let reader = FakeReader {
            cells: vec![binary_cell("control", "control", 100, 10)],
            calls: Mutex::new(vec![]),
        };
        let writer = FakeWriter::default();

        let n = run_interaction_sweep(&reader, &writer, &repo, &[a, b], Utc::now())
            .await
            .unwrap();
        assert_eq!(n, 0, "experiments in different envs never pair");
        assert!(writer.rows.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn empty_cells_grid_writes_nothing() {
        let metric = Uuid::new_v4();
        let env = Uuid::new_v4();
        let a = meta(Uuid::from_u128(1), Uuid::new_v4(), metric);
        let b = meta(Uuid::from_u128(2), Uuid::new_v4(), metric);
        let mut metrics = HashMap::new();
        metrics.insert(metric, agg_metric(metric, "checkout", AggregationOperator::Count));

        let reader = FakeReader {
            cells: vec![],
            calls: Mutex::new(vec![]),
        };
        let writer = FakeWriter::default();

        let n = compute_and_persist_interactions(
            &reader,
            &writer,
            env,
            &[a, b],
            &metrics,
            &["user".to_string()],
            ts(15),
        )
        .await
        .unwrap();

        assert_eq!(n, 0, "no shared population → no row");
        assert!(writer.rows.lock().unwrap().is_empty());
    }
}
