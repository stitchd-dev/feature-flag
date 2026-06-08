//! Cross-experiment **N-way** interaction orchestration.
//!
//! Ties together the building blocks from earlier phases into the full
//! compute-and-persist pipeline that runs on the 60-minute scheduler tick and
//! on the on-demand recompute path. Generalizes the original pairwise sweep to
//! any interaction order (2 or 3):
//!
//! 1. [`crate::interaction_pairs::candidate_pairs`] **and**
//!    [`crate::interaction_pairs::candidate_triples`] enumerate the experiment
//!    tuples worth testing (distinct flags, overlapping windows, shared metric,
//!    not same exclusion group; triples additionally need a common metric).
//! 2. For each tuple × shared metric × shared `context_type`, the metric is
//!    classified (conversion / continuous / ratio / funnel) and the matching
//!    k-way cell query ([`crate::queries::interaction_metric`]) aggregates the
//!    variant-tuple grid over the shared population into [`NdCellAggregate`]s.
//! 3. The cells are mapped to dense level indices
//!    ([`dense_levels`] / [`level_indices`]) and fed to the N-way decomposition
//!    ([`stitchd_core::experimentation::stats::interaction`]): the Frequentist
//!    `*_terms` plus the matching Bayesian `*_bayes`, merged per `TermKind`.
//! 4. Each decomposition **term** (main effect, pairwise, three-way) becomes one
//!    row in the ClickHouse `experiment_interactions` table.
//!
//! ## Multiple-comparison correction
//!
//! Every non-insufficient Frequentist p-value across the WHOLE sweep feeds a
//! single Benjamini–Hochberg pass at FDR = 0.05 ([`benjamini_hochberg`]); the
//! decisions set each row's `significant` (insufficient rows are never
//! significant). Bayesian fields are independent of the FDR correction.
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
    BayesianInteraction, NdBinaryCell, NdContinuousCell, NdRatioCell, TermKind, TermResult,
    binary_bayes, binary_terms, continuous_bayes, continuous_terms, ratio_bayes, ratio_terms,
};
use stitchd_core::metric::{AggregationConfig, MetricDefinition, MetricKind, RatioConfig};
use stitchd_db::repository::MetricRepository;

use crate::dispatch::rewrite_placeholders_to_clickhouse;
use crate::interaction_pairs::{ExperimentMeta, candidate_tuples};
use crate::queries::QueryBind;
use crate::queries::interaction_metric::{
    MetricCellKind, build_consolidated_aggregation_cells_query,
    build_interaction_funnel_cells_query, build_interaction_metric_cells_query,
    build_interaction_ratio_cells_query,
};

// ── N-dimensional (N-way) interaction cell ───────────────────────────────────

/// One N-dimensional interaction cell, keyed by the variant tuple across the
/// participating experiments (in experiment-tuple order — the same order as the
/// query aliases `a0, a1, …`).
///
/// Carries the sufficient statistics for *every* supported metric kind so a
/// single uniform SQL shape feeds binary (conversion/funnel), continuous, and
/// ratio interaction tests:
/// - **binary / funnel:** `n` + `successes`
/// - **continuous:** `n` + `value_sum` + `value_sq_sum`
/// - **ratio:** `n` + `num_sum` / `den_sum` (point estimate) + `num_sq_sum` /
///   `den_sq_sum` / `num_den_sum` (delta-method variance & covariance)
///
/// Serialized as the JSON array stored in `experiment_interactions.cell_stats`.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct NdCellAggregate {
    /// Variant key per participating experiment, in tuple order. `len()` equals
    /// the interaction order (2 for pairwise, 3 for three-way).
    pub variant_keys: Vec<String>,
    /// Shared contexts in this cell.
    pub n: u64,
    /// Shared contexts with ≥1 qualifying event (conversion) or that reached the
    /// funnel's final step.
    pub successes: u64,
    /// Σ per-context metric value (continuous).
    pub value_sum: f64,
    /// Σ per-context metric value² (continuous).
    pub value_sq_sum: f64,
    /// Σ per-context ratio numerator (ratio metrics).
    pub num_sum: f64,
    /// Σ per-context ratio denominator (ratio metrics).
    pub den_sum: f64,
    /// Σ numerator² — ratio delta-method variance term.
    pub num_sq_sum: f64,
    /// Σ denominator² — ratio delta-method variance term.
    pub den_sq_sum: f64,
    /// Σ numerator·denominator — ratio delta-method covariance term.
    pub num_den_sum: f64,
}

impl NdCellAggregate {
    /// Interaction order this cell belongs to (number of participating
    /// experiments) — the arity of its variant tuple.
    #[must_use]
    pub fn order(&self) -> usize {
        self.variant_keys.len()
    }
}

/// Serialize an N-D cell grid to the JSON string persisted in
/// `experiment_interactions.cell_stats`. Returns the empty array `"[]"` on the
/// (practically impossible) serialization failure, matching the pairwise path.
#[must_use]
pub fn cells_to_json(cells: &[NdCellAggregate]) -> String {
    serde_json::to_string(cells).unwrap_or_else(|_| "[]".to_owned())
}

/// Parse an N-D cell grid back from a `cell_stats` JSON string.
///
/// # Errors
/// Returns the `serde_json` error if the payload is not a valid cell array.
pub fn cells_from_json(s: &str) -> Result<Vec<NdCellAggregate>, serde_json::Error> {
    serde_json::from_str(s)
}

/// For an N-D cell set of the given `order`, return the per-dimension dense
/// level keys: `levels[d]` is the sorted distinct variant keys observed in
/// dimension `d`. This defines the stable 0-based level index used by the
/// contingency-table / multi-factor-ANOVA math downstream — every consumer
/// derives indices the same way, so the tensor layout is deterministic.
#[must_use]
pub fn dense_levels(cells: &[NdCellAggregate], order: usize) -> Vec<Vec<String>> {
    let mut levels: Vec<std::collections::BTreeSet<String>> = vec![Default::default(); order];
    for c in cells {
        for (d, key) in c.variant_keys.iter().enumerate().take(order) {
            levels[d].insert(key.clone());
        }
    }
    levels
        .into_iter()
        .map(|set| set.into_iter().collect())
        .collect()
}

/// Map a cell's variant tuple to its dense per-dimension level indices, given
/// the `levels` returned by [`dense_levels`].
///
/// Returns `None` when the cell's arity disagrees with `levels.len()` or any
/// variant key is absent from its dimension (neither happens for cells drawn
/// from the same set `levels` was built from — the `None` is a guard for misuse).
#[must_use]
pub fn level_indices(cell: &NdCellAggregate, levels: &[Vec<String>]) -> Option<Vec<usize>> {
    if cell.variant_keys.len() != levels.len() {
        return None;
    }
    cell.variant_keys
        .iter()
        .zip(levels)
        .map(|(key, dim)| dim.iter().position(|k| k == key))
        .collect()
}

/// One computed interaction **term** row, ready to persist to
/// `experiment_interactions`. One candidate tuple emits a full hierarchical
/// decomposition — one row per term (main / pairwise / three-way).
#[derive(Debug, Clone, PartialEq)]
pub struct InteractionRow {
    /// Environment UUID.
    pub env_id: Uuid,
    /// Participating experiment UUIDs, **sorted ascending**. `len()` equals
    /// `interaction_order`.
    pub experiment_ids: Vec<Uuid>,
    /// Interaction order of the candidate tuple (2 or 3) — the number of
    /// participating experiments.
    pub interaction_order: u8,
    /// Term identity string, encoding the participating experiment UUIDs at the
    /// term's factor positions:
    /// - `"main:<exp>"` — a single-experiment main effect,
    /// - `"2way:<a>x<b>"` — a pairwise interaction term,
    /// - `"3way:<a>x<b>x<c>"` — the top-order three-way interaction term.
    pub term: String,
    /// Context dimension the interaction was computed over.
    pub context_type: String,
    /// Shared metric key.
    pub metric_key: String,
    /// Total shared contexts across all cells of the tuple's grid.
    pub shared_count: u64,
    /// JSON of the per-cell stats — the N-dimensional [`NdCellAggregate`] grid
    /// (serialized via [`cells_to_json`]). Shared across every term row of the
    /// same tuple. On a serialization failure the empty array `"[]"` is
    /// persisted.
    pub cell_stats: String,
    /// Frequentist interaction effect-size estimate for this term (`0.0`
    /// sentinel when insufficient data).
    pub interaction_estimate: f64,
    /// Two-sided Frequentist p-value for this term (`0.0` sentinel when
    /// insufficient data).
    pub p_value: f64,
    /// Degrees of freedom of the term's Frequentist test.
    pub df: u32,
    /// Whether the term is significant after Benjamini–Hochberg FDR correction
    /// across the sweep batch (FDR = 0.05). Always `false` when
    /// `insufficient_data` is true.
    pub significant: bool,
    /// Whether the inputs were too sparse/degenerate to run the term's test.
    /// When true, `interaction_estimate` / `p_value` are `0.0` sentinels and
    /// `significant` is `false`.
    pub insufficient_data: bool,
    /// Posterior probability the interaction effect is non-negligible
    /// (Bayesian). `0.0` when no posterior was computed.
    pub bayes_prob: f64,
    /// Posterior mean of the interaction effect (Bayesian). `0.0` when absent.
    pub bayes_expected: f64,
    /// Lower bound of the Bayesian credible interval. `0.0` when absent.
    pub bayes_ci_low: f64,
    /// Upper bound of the Bayesian credible interval. `0.0` when absent.
    pub bayes_ci_high: f64,
    /// When the row was computed.
    pub computed_at: DateTime<Utc>,
}

// ── Reader / writer traits ───────────────────────────────────────────────────

/// Reads the per-variant-tuple metric grid for one (tuple, metric,
/// context_type) from ClickHouse, one method per metric family. Each returns
/// the N-dimensional cell grid keyed by variant tuple (in `exp_ids` order).
#[async_trait]
pub trait InteractionCellReader: Send + Sync {
    /// Conversion / continuous cells for an `Aggregation` metric over the shared
    /// population of `exp_ids` (length 2 or 3) within `env_id`.
    async fn fetch_aggregation_cells(
        &self,
        cfg: &AggregationConfig,
        env_id: &str,
        exp_ids: &[&str],
        context_type: &str,
        interaction_end: DateTime<Utc>,
    ) -> Result<Vec<NdCellAggregate>, anyhow::Error>;

    /// **Consolidated** conversion / continuous cells for an `Aggregation`
    /// metric shared by EVERY tuple in `tuples` (each a length-2-or-3 slice of
    /// experiment ids), over one `env_id` + `context_type`, in ONE ClickHouse
    /// query (review #12). Returns one cell grid PER tuple, in the same order as
    /// `tuples` — `result[i]` is the grid for `tuples[i]`, identical to what
    /// [`Self::fetch_aggregation_cells`] would return for that tuple alone.
    ///
    /// The default implementation falls back to issuing one
    /// [`Self::fetch_aggregation_cells`] per tuple, so existing fakes need not
    /// implement it; the production reader overrides it with the single
    /// consolidated scan.
    async fn fetch_aggregation_cells_consolidated(
        &self,
        cfg: &AggregationConfig,
        env_id: &str,
        tuples: &[&[&str]],
        context_type: &str,
        interaction_end: DateTime<Utc>,
    ) -> Result<Vec<Vec<NdCellAggregate>>, anyhow::Error> {
        let mut out = Vec::with_capacity(tuples.len());
        for exp_ids in tuples {
            out.push(
                self.fetch_aggregation_cells(cfg, env_id, exp_ids, context_type, interaction_end)
                    .await?,
            );
        }
        Ok(out)
    }

    /// Binary funnel cells (`successes` = reached final step) for a `Funnel`
    /// metric over the shared population of `exp_ids`.
    async fn fetch_funnel_cells(
        &self,
        cfg: &stitchd_core::metric::FunnelConfig,
        env_id: &str,
        exp_ids: &[&str],
        context_type: &str,
        interaction_end: DateTime<Utc>,
    ) -> Result<Vec<NdCellAggregate>, anyhow::Error>;

    /// Ratio cells (numerator/denominator second moments) for a `Ratio` metric
    /// over the shared population of `exp_ids`. `num_cfg`/`den_cfg` are the
    /// resolved aggregation legs.
    async fn fetch_ratio_cells(
        &self,
        num_cfg: &AggregationConfig,
        den_cfg: &AggregationConfig,
        env_id: &str,
        exp_ids: &[&str],
        context_type: &str,
        interaction_end: DateTime<Utc>,
    ) -> Result<Vec<NdCellAggregate>, anyhow::Error>;
}

/// Persists computed interaction rows to ClickHouse.
#[async_trait]
pub trait InteractionWriter: Send + Sync {
    /// Write one interaction term row to `experiment_interactions`.
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
/// ## Multiple-comparison correction (MED-1)
///
/// The sweep runs one significance test per `(pair × shared metric ×
/// context_type)`. Flagging each at the raw per-test α = 0.05 would inflate the
/// family-wise error across the batch. Instead, after computing every result we
/// apply a **Benjamini–Hochberg** procedure across the batch's *valid*
/// (non-`insufficient_data`) p-values and set each persisted `significant` from
/// the BH decision at FDR = 0.05. Insufficient-data rows never participate and
/// are never significant. See [`benjamini_hochberg`].
///
/// # Errors
/// Propagates the first reader / writer error encountered. A per-cell read that
/// returns an empty grid is skipped (no shared population on that context_type —
/// nothing to persist).
#[allow(clippy::too_many_arguments)]
pub async fn compute_and_persist_interactions(
    reader: &dyn InteractionCellReader,
    writer: &dyn InteractionWriter,
    env_id: Uuid,
    experiments: &[ExperimentMeta],
    metrics: &HashMap<Uuid, MetricDefinition>,
    context_types: &[String],
    computed_at: DateTime<Utc>,
    max_order: usize,
) -> Result<usize, anyhow::Error> {
    let by_id: HashMap<Uuid, &ExperimentMeta> = experiments.iter().map(|e| (e.id, e)).collect();
    let env_str = env_id.to_string();

    // The operator-bounded interaction order. Clamped to ≥ 2 (a 1-way "tuple"
    // is just a single experiment, no interaction). Tuples of order ≤ this cap
    // are enumerated; higher orders are intentionally NOT computed and the cap
    // is logged below when it actually truncates a possible higher-order tuple
    // (no silent truncation, per FR6).
    let cap = max_order.max(2);

    // Enumerate every candidate tuple of order 2..=cap as a uniform list of
    // experiment-id vectors so the rest of the pipeline is order-agnostic.
    let mut tuples: Vec<Vec<Uuid>> = Vec::new();
    for order in 2..=cap {
        for tuple in candidate_tuples(experiments, order) {
            tuples.push(tuple);
        }
    }

    // Cap-truncation visibility: if there exist enough mutually-overlapping,
    // common-metric experiments to FORM an (cap+1)-way candidate but the cap
    // forbids it, log so the truncation is never silent.
    if experiments.len() > cap && !candidate_tuples(experiments, cap + 1).is_empty() {
        tracing::info!(
            env_id = %env_id,
            max_interaction_order = cap,
            candidates_above_cap = candidate_tuples(experiments, cap + 1).len(),
            "interaction sweep: higher-order interaction candidates exist but are \
             truncated by the configured max interaction order cap"
        );
    }

    // Per-tuple context resolved once: metas, the tuple's overlap-window upper
    // bound, the metrics common to EVERY experiment in the tuple, and the
    // stringified experiment ids (kept alive for the &str views the readers
    // take). Tuples whose metas are not all present are dropped.
    struct TupleCtx<'a> {
        exp_ids: Vec<Uuid>,
        interaction_end: DateTime<Utc>,
        shared_metric_ids: Vec<Uuid>,
        id_strs: Vec<String>,
        _metas: Vec<&'a ExperimentMeta>,
    }
    let tuple_ctxs: Vec<TupleCtx> = tuples
        .into_iter()
        .filter_map(|exp_ids| {
            let metas: Vec<&ExperimentMeta> = exp_ids
                .iter()
                .map(|id| by_id.get(id).copied())
                .collect::<Option<_>>()?;
            // Overlap-window upper bound: earliest finite end across the tuple,
            // or "now" when all are still running. candidate_* proved the windows
            // overlap pairwise (⇒ a common live window by Helly's theorem).
            let interaction_end = metas.iter().fold(computed_at, |acc, m| match m.ended_at {
                Some(end) => acc.min(end),
                None => acc,
            });
            // Metrics common to EVERY experiment in the tuple.
            let shared_metric_ids: Vec<Uuid> = metas[0]
                .metric_ids
                .iter()
                .filter(|m| metas[1..].iter().all(|meta| meta.metric_ids.contains(m)))
                .copied()
                .collect();
            let id_strs: Vec<String> = exp_ids.iter().map(Uuid::to_string).collect();
            Some(TupleCtx {
                exp_ids,
                interaction_end,
                shared_metric_ids,
                id_strs,
                _metas: metas,
            })
        })
        .collect();

    // Phase 1 — compute every (tuple × metric × context_type) decomposition for
    // the batch. We collect all term rows before deciding significance so the
    // Benjamini–Hochberg correction can see the whole family of tests.
    //
    // The AGGREGATION metric path is consolidated (review #12): per
    // (context_type, aggregation metric, interaction_end) the tuples sharing
    // that metric are fetched in ONE ClickHouse query and routed back per tuple.
    // FUNNEL / RATIO metrics keep the per-tuple path (their CTEs are complex and
    // they are rarer). ALL downstream logic (decomposition, term_string,
    // NdCellAggregate construction, BH-FDR, persist) is identical regardless of
    // which fetch produced the cells.
    let mut rows: Vec<InteractionRow> = Vec::new();

    // Emit one row per decomposition term for a tuple's computed grid. The
    // factor index → experiment mapping is `factor i ↔ exp_ids[i]` (the query
    // alias / tuple order, the same order the readers key `variant_keys` by).
    let push_tuple_rows = |rows: &mut Vec<InteractionRow>,
                           exp_ids: &[Uuid],
                           metric_key: &str,
                           context_type: &str,
                           cells: &[NdCellAggregate],
                           terms: &[TermResult]| {
        let shared_count: u64 = cells.iter().map(|c| c.n).sum();
        let cell_stats = cells_to_json(cells);
        let order = exp_ids.len() as u8;
        for t in terms {
            let term = term_string(&t.kind, exp_ids);
            let bayes = t.bayes.unwrap_or(BayesianInteraction {
                prob: 0.0,
                expected: 0.0,
                ci_low: 0.0,
                ci_high: 0.0,
            });
            rows.push(InteractionRow {
                env_id,
                experiment_ids: exp_ids.to_vec(),
                interaction_order: order,
                term,
                context_type: context_type.to_owned(),
                metric_key: metric_key.to_owned(),
                shared_count,
                cell_stats: cell_stats.clone(),
                // Insufficient-data terms keep 0.0 sentinels (the column is
                // non-nullable Float64); the flag distinguishes them.
                interaction_estimate: nan_to_zero(t.freq.estimate),
                p_value: nan_to_zero(t.freq.p_value),
                df: t.freq.df,
                // Provisional; overwritten by the BH pass below.
                significant: false,
                insufficient_data: t.freq.insufficient_data,
                bayes_prob: nan_to_zero(bayes.prob),
                bayes_expected: nan_to_zero(bayes.expected),
                bayes_ci_low: nan_to_zero(bayes.ci_low),
                bayes_ci_high: nan_to_zero(bayes.ci_high),
                computed_at,
            });
        }
    };

    // ── Pass A: funnel / ratio metrics, per-tuple (unchanged fetch path) ────
    for ctx in &tuple_ctxs {
        let id_refs: Vec<&str> = ctx.id_strs.iter().map(String::as_str).collect();
        for &metric_id in &ctx.shared_metric_ids {
            let Some(def) = metrics.get(&metric_id) else {
                tracing::debug!(%metric_id, "interaction: metric definition not resolved; skipping");
                continue;
            };
            // Aggregation metrics are handled by the consolidated pass below.
            if matches!(def.kind, MetricKind::Aggregation(_)) {
                continue;
            }
            for context_type in context_types {
                let computed = compute_tuple_metric(
                    reader,
                    def,
                    metrics,
                    &env_str,
                    &id_refs,
                    context_type,
                    ctx.interaction_end,
                )
                .await?;
                let Some((cells, terms)) = computed else {
                    continue; // skipped (unresolved ratio legs) or empty grid
                };
                push_tuple_rows(
                    &mut rows,
                    &ctx.exp_ids,
                    &def.key,
                    context_type,
                    &cells,
                    &terms,
                );
            }
        }
    }

    // ── Pass B: aggregation metrics, consolidated per (context, metric, end) ─
    // Group the tuples that share a given aggregation metric on a given
    // context_type AND a given interaction_end (the consolidated query bounds
    // the shared scans with ONE end, so tuples with different ends — only when
    // experiments have distinct finite end dates — fall into separate groups).
    // In the common all-running case every tuple shares `computed_at`, so the
    // whole family collapses to one query per (context_type, metric).
    //
    // Key: (context_type index, metric_id, interaction_end). Value: indices into
    // `tuple_ctxs` (insertion-ordered for a deterministic tuple_idx layout).
    let mut groups: HashMap<(usize, Uuid, DateTime<Utc>), Vec<usize>> = HashMap::new();
    let mut group_order: Vec<(usize, Uuid, DateTime<Utc>)> = Vec::new();
    for (ti, ctx) in tuple_ctxs.iter().enumerate() {
        for &metric_id in &ctx.shared_metric_ids {
            let Some(def) = metrics.get(&metric_id) else {
                continue; // already logged in Pass A
            };
            if !matches!(def.kind, MetricKind::Aggregation(_)) {
                continue;
            }
            for (ci, _context_type) in context_types.iter().enumerate() {
                let key = (ci, metric_id, ctx.interaction_end);
                let bucket = groups.entry(key).or_insert_with(|| {
                    group_order.push(key);
                    Vec::new()
                });
                bucket.push(ti);
            }
        }
    }

    for key in &group_order {
        let (ci, metric_id, interaction_end) = *key;
        let tuple_indices = &groups[key];
        let context_type = &context_types[ci];
        let def = &metrics[&metric_id];
        let MetricKind::Aggregation(cfg) = &def.kind else {
            continue; // unreachable: only aggregation metrics keyed this map
        };

        // Build the &[&str] tuple slices for the consolidated fetch, in the
        // same order they will be routed back (tuple_idx == position here).
        let id_refs_per_tuple: Vec<Vec<&str>> = tuple_indices
            .iter()
            .map(|&ti| tuple_ctxs[ti].id_strs.iter().map(String::as_str).collect())
            .collect();
        let tuple_slices: Vec<&[&str]> = id_refs_per_tuple.iter().map(Vec::as_slice).collect();

        let grids = reader
            .fetch_aggregation_cells_consolidated(
                cfg,
                &env_str,
                &tuple_slices,
                context_type,
                interaction_end,
            )
            .await?;

        for (slot, &ti) in tuple_indices.iter().enumerate() {
            let Some(cells) = grids.get(slot) else {
                continue; // reader returned fewer grids than tuples (defensive)
            };
            if cells.is_empty() {
                continue; // no shared population on this context_type
            }
            let exp_ids = &tuple_ctxs[ti].exp_ids;
            let terms = decompose_aggregation(cfg, cells, exp_ids.len());
            push_tuple_rows(&mut rows, exp_ids, &def.key, context_type, cells, &terms);
        }
    }

    // Phase 2 — Benjamini–Hochberg correction across the batch's valid
    // *interaction* tests (2-way / 3-way terms), then persist. Main-effect terms
    // are single-experiment quantities, not cross-experiment interactions, so
    // they are excluded from the interaction FDR family and never marked
    // significant here — including their (typically tiny) p-values would inflate
    // the family size and shift the step-up threshold for the interaction terms
    // this analysis actually exists to surface. Only non-insufficient interaction
    // rows feed the correction. Bayesian fields are NOT part of the FDR decision.
    let valid_pvalues: Vec<f64> = rows
        .iter()
        .filter(|r| !r.insufficient_data && is_interaction_term(&r.term))
        .map(|r| r.p_value)
        .collect();
    let reject = benjamini_hochberg(&valid_pvalues, 0.05);

    let mut reject_iter = reject.into_iter();
    let mut written = 0usize;
    for mut row in rows {
        // A row consumes the next BH decision iff it fed `valid_pvalues` (a
        // non-insufficient interaction term) — same iteration order. Insufficient
        // rows and main-effect rows are never significant and consume nothing.
        row.significant = if row.insufficient_data || !is_interaction_term(&row.term) {
            false
        } else {
            reject_iter.next().unwrap_or(false)
        };
        writer.write_row(&row).await?;
        written += 1;
    }
    Ok(written)
}

/// Whether a persisted `term` is a cross-experiment *interaction* term (2-way or
/// 3-way) as opposed to a single-experiment `main:` effect. Only interaction
/// terms participate in the Benjamini–Hochberg family and can be `significant`.
fn is_interaction_term(term: &str) -> bool {
    !term.starts_with("main:")
}

/// Classify `def`, fetch the matching k-way cell grid for `exp_ids`, and run the
/// N-way decomposition (Frequentist `*_terms` + matching Bayesian `*_bayes`,
/// merged per [`TermKind`]).
///
/// Returns `None` when the grid is empty (no shared population on this
/// `context_type`) or when a ratio metric's legs cannot be resolved as two
/// aggregation metrics (logged + skipped — never an error).
///
/// # Errors
/// Propagates the reader error encountered.
async fn compute_tuple_metric(
    reader: &dyn InteractionCellReader,
    def: &MetricDefinition,
    metrics: &HashMap<Uuid, MetricDefinition>,
    env_str: &str,
    exp_ids: &[&str],
    context_type: &str,
    interaction_end: DateTime<Utc>,
) -> Result<Option<(Vec<NdCellAggregate>, Vec<TermResult>)>, anyhow::Error> {
    let order = exp_ids.len();
    match &def.kind {
        MetricKind::Aggregation(cfg) => {
            let cells = reader
                .fetch_aggregation_cells(cfg, env_str, exp_ids, context_type, interaction_end)
                .await?;
            if cells.is_empty() {
                return Ok(None);
            }
            let terms = decompose_aggregation(cfg, &cells, order);
            Ok(Some((cells, terms)))
        }
        MetricKind::Funnel(cfg) => {
            let cells = reader
                .fetch_funnel_cells(cfg, env_str, exp_ids, context_type, interaction_end)
                .await?;
            if cells.is_empty() {
                return Ok(None);
            }
            let terms = decompose_binary(&cells, order, BinarySource::Funnel);
            Ok(Some((cells, terms)))
        }
        MetricKind::Ratio(ratio_cfg) => {
            let Some((num_cfg, den_cfg)) = resolve_ratio_legs(ratio_cfg, metrics) else {
                tracing::debug!(
                    metric_key = %def.key,
                    "interaction: ratio legs not resolvable to two aggregation metrics; skipping"
                );
                return Ok(None);
            };
            let cells = reader
                .fetch_ratio_cells(
                    &num_cfg,
                    &den_cfg,
                    env_str,
                    exp_ids,
                    context_type,
                    interaction_end,
                )
                .await?;
            if cells.is_empty() {
                return Ok(None);
            }
            let terms = decompose_ratio(&cells, order);
            Ok(Some((cells, terms)))
        }
    }
}

/// Run the N-way decomposition for an `Aggregation` metric's cell grid,
/// dispatching on whether the metric is a conversion (binary) or continuous
/// outcome. Shared by the per-tuple path ([`compute_tuple_metric`]) and the
/// consolidated path so both produce identical terms from identical cells.
fn decompose_aggregation(
    cfg: &AggregationConfig,
    cells: &[NdCellAggregate],
    order: usize,
) -> Vec<TermResult> {
    match MetricCellKind::classify(cfg) {
        MetricCellKind::Conversion => decompose_binary(cells, order, BinarySource::Conversion),
        MetricCellKind::Continuous => decompose_continuous(cells, order),
    }
}

/// Which binary source the cell grid represents — purely for documentation; the
/// `successes` column already carries the right semantics for both.
#[derive(Debug, Clone, Copy)]
enum BinarySource {
    /// `successes` = contexts with ≥1 qualifying event.
    Conversion,
    /// `successes` = contexts that reached the funnel's final step.
    Funnel,
}

/// Resolve a [`RatioConfig`] to its numerator + denominator
/// [`AggregationConfig`] legs, returning `None` if either referenced metric is
/// absent from `metrics` or is not itself an aggregation metric.
fn resolve_ratio_legs(
    ratio_cfg: &RatioConfig,
    metrics: &HashMap<Uuid, MetricDefinition>,
) -> Option<(AggregationConfig, AggregationConfig)> {
    let num = metrics.get(&ratio_cfg.numerator_metric_id.as_uuid())?;
    let den = metrics.get(&ratio_cfg.denominator_metric_id.as_uuid())?;
    let (MetricKind::Aggregation(num_cfg), MetricKind::Aggregation(den_cfg)) =
        (&num.kind, &den.kind)
    else {
        return None;
    };
    Some((num_cfg.clone(), den_cfg.clone()))
}

/// Map an [`NdCellAggregate`] grid to dense binary cells and run the binary
/// decomposition + matching Bayesian posteriors, merged by [`TermKind`].
fn decompose_binary(
    cells: &[NdCellAggregate],
    order: usize,
    _source: BinarySource,
) -> Vec<TermResult> {
    let levels = dense_levels(cells, order);
    let nd: Vec<NdBinaryCell> = cells
        .iter()
        .filter_map(|c| {
            level_indices(c, &levels).map(|lv| NdBinaryCell {
                levels: lv,
                n: c.n,
                successes: c.successes,
            })
        })
        .collect();
    let mut terms = binary_terms(&nd, order);
    merge_bayes(&mut terms, binary_bayes(&nd, order));
    terms
}

/// Map an [`NdCellAggregate`] grid to dense continuous cells and run the
/// continuous decomposition + matching Bayesian posteriors.
fn decompose_continuous(cells: &[NdCellAggregate], order: usize) -> Vec<TermResult> {
    let levels = dense_levels(cells, order);
    let nd: Vec<NdContinuousCell> = cells
        .iter()
        .filter_map(|c| {
            level_indices(c, &levels).map(|lv| NdContinuousCell {
                levels: lv,
                n: c.n,
                sum: c.value_sum,
                sum_sq: c.value_sq_sum,
            })
        })
        .collect();
    let mut terms = continuous_terms(&nd, order);
    merge_bayes(&mut terms, continuous_bayes(&nd, order));
    terms
}

/// Map an [`NdCellAggregate`] grid to dense ratio cells and run the ratio
/// decomposition + matching Bayesian posteriors.
fn decompose_ratio(cells: &[NdCellAggregate], order: usize) -> Vec<TermResult> {
    let levels = dense_levels(cells, order);
    let nd: Vec<NdRatioCell> = cells
        .iter()
        .filter_map(|c| {
            level_indices(c, &levels).map(|lv| NdRatioCell {
                levels: lv,
                n: c.n,
                num_sum: c.num_sum,
                den_sum: c.den_sum,
                num_sq_sum: c.num_sq_sum,
                den_sq_sum: c.den_sq_sum,
                num_den_sum: c.num_den_sum,
            })
        })
        .collect();
    let mut terms = ratio_terms(&nd, order);
    merge_bayes(&mut terms, ratio_bayes(&nd, order));
    terms
}

/// Attach each Bayesian posterior to the matching Frequentist term by
/// [`TermKind`] (the two inference models are computed independently).
fn merge_bayes(terms: &mut [TermResult], bayes: Vec<(TermKind, BayesianInteraction)>) {
    for t in terms.iter_mut() {
        if let Some((_, b)) = bayes.iter().find(|(k, _)| *k == t.kind) {
            t.bayes = Some(*b);
        }
    }
}

/// Render the `term` column string for a decomposition term, substituting the
/// ACTUAL experiment UUIDs at the term's factor positions (`exp_ids[factor]`,
/// tuple order):
/// - `"main:<exp>"`, `"2way:<a>x<b>"`, `"3way:<a>x<b>x<c>"`, and for order ≥ 4
///   `"<k>way:<exp>x…"` (k factors joined by `x`).
fn term_string(kind: &TermKind, exp_ids: &[Uuid]) -> String {
    match kind {
        TermKind::Main { factor } => format!("main:{}", exp_ids[*factor]),
        TermKind::TwoWay { a, b } => format!("2way:{}x{}", exp_ids[*a], exp_ids[*b]),
        TermKind::ThreeWay { a, b, c } => {
            format!("3way:{}x{}x{}", exp_ids[*a], exp_ids[*b], exp_ids[*c])
        }
        TermKind::NWay { factors } => {
            let joined = factors
                .iter()
                .map(|&f| exp_ids[f].to_string())
                .collect::<Vec<_>>()
                .join("x");
            format!("{}way:{}", factors.len(), joined)
        }
    }
}

/// Benjamini–Hochberg step-up procedure for controlling the false discovery
/// rate (FDR) across a family of independent tests.
///
/// Given the `pvalues` of the family and a target `fdr` (e.g. `0.05`), returns a
/// per-input boolean — `true` ⇒ reject the null (declare significant). The
/// returned vector is in the **same order** as the input; the ranking is done
/// internally on a sorted copy.
///
/// Method: sort the `m` p-values ascending, find the largest rank `k`
/// (1-indexed) for which `p_(k) ≤ (k / m) · fdr`, then reject every hypothesis
/// whose p-value is `≤ p_(k)` (the BH step-up threshold). With one true effect
/// among many nulls this rejects only the small p-value(s) that clear the
/// rank-scaled threshold, sharply curbing the family-wise false-positive rate a
/// raw per-test α would produce. An empty family returns an empty vector; `fdr`
/// is clamped to `[0, 1]`.
#[must_use]
pub fn benjamini_hochberg(pvalues: &[f64], fdr: f64) -> Vec<bool> {
    let m = pvalues.len();
    if m == 0 {
        return Vec::new();
    }
    let fdr = fdr.clamp(0.0, 1.0);

    // Rank the p-values ascending, remembering original positions.
    let mut indexed: Vec<(usize, f64)> = pvalues.iter().copied().enumerate().collect();
    indexed.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));

    // Largest k (1-indexed) with p_(k) <= (k/m)*fdr → BH threshold = p_(k).
    let mut k_max: Option<usize> = None;
    for (rank0, &(_, p)) in indexed.iter().enumerate() {
        let k = rank0 + 1;
        let threshold = (k as f64 / m as f64) * fdr;
        if p <= threshold {
            k_max = Some(k);
        }
    }

    let mut reject = vec![false; m];
    if let Some(k) = k_max {
        // Step-up: reject the k smallest-p hypotheses (ranks 1..=k).
        for &(orig_idx, _) in &indexed[..k] {
            reject[orig_idx] = true;
        }
    }
    reject
}

/// Environment-variable name for the operator-bounded maximum cross-experiment
/// interaction order. Default [`DEFAULT_MAX_INTERACTION_ORDER`] (3) preserves
/// the historical 2-/3-way behaviour; set to 4+ to enable higher-order
/// interaction detection (combinatorially more candidate tuples + decomposition
/// terms, so raise deliberately). Values below 2 are clamped to 2 downstream.
pub const MAX_INTERACTION_ORDER_ENV: &str = "STITCHD_STATS_MAX_INTERACTION_ORDER";

/// Default operator-bounded maximum interaction order (preserves pre-Phase-10
/// order-3 behaviour).
pub const DEFAULT_MAX_INTERACTION_ORDER: usize = 3;

/// Read the operator-bounded max interaction order from
/// `STITCHD_STATS_MAX_INTERACTION_ORDER`, falling back to
/// [`DEFAULT_MAX_INTERACTION_ORDER`] when unset / unparseable. Clamped to ≥ 2.
///
/// The env-var name is written as a string literal here (not via
/// [`MAX_INTERACTION_ORDER_ENV`]) so the `cargo xtask docs` env-var scraper —
/// which matches `env::var("STITCHD_…")` literals and resolves the
/// `.unwrap_or(CONST)` default against the in-file `const` — picks it up.
#[must_use]
pub fn max_interaction_order_from_env() -> usize {
    std::env::var("STITCHD_STATS_MAX_INTERACTION_ORDER")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(DEFAULT_MAX_INTERACTION_ORDER)
        .max(2)
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
/// so the window-overlap check treats every concurrently-running tuple as
/// overlapping, which is correct. Both candidate pairs and candidate triples are
/// enumerated inside [`compute_and_persist_interactions`].
///
/// Returns the total number of interaction term rows written across all
/// environments.
///
/// # Errors
/// Propagates the first metric-repo / reader / writer error encountered.
pub async fn run_interaction_sweep(
    reader: &dyn InteractionCellReader,
    writer: &dyn InteractionWriter,
    metric_repo: &dyn MetricRepository,
    running: &[Experiment],
    computed_at: DateTime<Utc>,
    max_order: usize,
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
                metric_ids: e
                    .metric_ids
                    .iter()
                    .map(stitchd_core::id::MetricId::as_uuid)
                    .collect(),
                exclusion_group_id: e.exclusion_group_id.map(|g| g.as_uuid()),
            })
            .collect();

        // Resolve all referenced metric definitions in one batch.
        let mut metric_ids: Vec<stitchd_core::id::MetricId> = exps
            .iter()
            .flat_map(|e| e.metric_ids.iter().copied())
            .collect();
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
            max_order,
        )
        .await?;
    }
    Ok(total)
}

/// Replace `NaN` (insufficient-data sentinel) with `0.0` so the value
/// round-trips cleanly through the ClickHouse `Float64` column.
fn nan_to_zero(v: f64) -> f64 {
    if v.is_nan() { 0.0 } else { v }
}

// ── ClickHouse-backed reader ─────────────────────────────────────────────────

/// Row shape deserialised from the aggregation metric-cells query. The
/// `variant_keys` array maps to a `Vec<String>` in tuple order.
#[derive(Debug, Clone, serde::Deserialize, clickhouse::Row)]
struct ChAggregationCellRow {
    variant_keys: Vec<String>,
    n: u64,
    successes: u64,
    value_sum: f64,
    value_sq_sum: f64,
}

/// Row shape deserialised from the **consolidated** aggregation metric-cells
/// query. Identical to [`ChAggregationCellRow`] plus the leading `tuple_idx`
/// that routes the cell back to the tuple it belongs to.
#[derive(Debug, Clone, serde::Deserialize, clickhouse::Row)]
struct ChConsolidatedAggregationCellRow {
    tuple_idx: u32,
    variant_keys: Vec<String>,
    n: u64,
    successes: u64,
    value_sum: f64,
    value_sq_sum: f64,
}

/// Row shape deserialised from the funnel metric-cells query (binary path).
#[derive(Debug, Clone, serde::Deserialize, clickhouse::Row)]
struct ChFunnelCellRow {
    variant_keys: Vec<String>,
    n: u64,
    successes: u64,
}

/// Row shape deserialised from the ratio metric-cells query (second moments).
#[derive(Debug, Clone, serde::Deserialize, clickhouse::Row)]
struct ChRatioCellRow {
    variant_keys: Vec<String>,
    n: u64,
    num_sum: f64,
    den_sum: f64,
    num_sq_sum: f64,
    den_sq_sum: f64,
    num_den_sum: f64,
}

/// Rewrite a [`crate::queries::BuiltQuery`]'s placeholders to `?` and bind its
/// values onto a fresh CH query (the returned `Query` owns a clone of the
/// client, so it does not borrow `client`).
fn bind_query(client: &Client, sql: String, binds: Vec<QueryBind>) -> clickhouse::query::Query {
    let sql = rewrite_placeholders_to_clickhouse(sql);
    let mut query = client.query(&sql);
    for b in binds {
        query = match b {
            QueryBind::Str(s) => query.bind(s),
            QueryBind::I64(n) => query.bind(n),
            QueryBind::F64(f) => query.bind(f),
        };
    }
    query
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
    async fn fetch_aggregation_cells(
        &self,
        cfg: &AggregationConfig,
        env_id: &str,
        exp_ids: &[&str],
        context_type: &str,
        interaction_end: DateTime<Utc>,
    ) -> Result<Vec<NdCellAggregate>, anyhow::Error> {
        let built = build_interaction_metric_cells_query(
            cfg,
            env_id,
            exp_ids,
            context_type,
            interaction_end,
        )?;
        let query = bind_query(&self.client, built.sql, built.binds);
        let rows = query.fetch_all::<ChAggregationCellRow>().await?;
        Ok(rows
            .into_iter()
            .map(|r| NdCellAggregate {
                variant_keys: r.variant_keys,
                n: r.n,
                successes: r.successes,
                value_sum: r.value_sum,
                value_sq_sum: r.value_sq_sum,
                num_sum: 0.0,
                den_sum: 0.0,
                num_sq_sum: 0.0,
                den_sq_sum: 0.0,
                num_den_sum: 0.0,
            })
            .collect())
    }

    async fn fetch_aggregation_cells_consolidated(
        &self,
        cfg: &AggregationConfig,
        env_id: &str,
        tuples: &[&[&str]],
        context_type: &str,
        interaction_end: DateTime<Utc>,
    ) -> Result<Vec<Vec<NdCellAggregate>>, anyhow::Error> {
        // Empty tuple slice → nothing to fetch (the builder rejects it).
        if tuples.is_empty() {
            return Ok(Vec::new());
        }
        let built = build_consolidated_aggregation_cells_query(
            cfg,
            env_id,
            tuples,
            context_type,
            interaction_end,
        )?;
        let query = bind_query(&self.client, built.sql, built.binds);
        let rows = query
            .fetch_all::<ChConsolidatedAggregationCellRow>()
            .await?;

        // Route each row to its tuple bucket by `tuple_idx`. Buckets line up
        // 1:1 with `tuples` (a tuple with no shared population yields an empty
        // grid — the same as the per-tuple path returning an empty Vec).
        let mut grids: Vec<Vec<NdCellAggregate>> = vec![Vec::new(); tuples.len()];
        for r in rows {
            let idx = r.tuple_idx as usize;
            let Some(bucket) = grids.get_mut(idx) else {
                // The query only ever emits indices in 0..tuples.len(); a stray
                // index would indicate a builder/reader drift — skip defensively.
                tracing::warn!(
                    tuple_idx = r.tuple_idx,
                    tuples = tuples.len(),
                    "consolidated aggregation cell with out-of-range tuple_idx; skipping"
                );
                continue;
            };
            bucket.push(NdCellAggregate {
                variant_keys: r.variant_keys,
                n: r.n,
                successes: r.successes,
                value_sum: r.value_sum,
                value_sq_sum: r.value_sq_sum,
                num_sum: 0.0,
                den_sum: 0.0,
                num_sq_sum: 0.0,
                den_sq_sum: 0.0,
                num_den_sum: 0.0,
            });
        }
        Ok(grids)
    }

    async fn fetch_funnel_cells(
        &self,
        cfg: &stitchd_core::metric::FunnelConfig,
        env_id: &str,
        exp_ids: &[&str],
        context_type: &str,
        interaction_end: DateTime<Utc>,
    ) -> Result<Vec<NdCellAggregate>, anyhow::Error> {
        let built = build_interaction_funnel_cells_query(
            cfg,
            env_id,
            exp_ids,
            context_type,
            interaction_end,
        )?;
        let query = bind_query(&self.client, built.sql, built.binds);
        let rows = query.fetch_all::<ChFunnelCellRow>().await?;
        Ok(rows
            .into_iter()
            .map(|r| NdCellAggregate {
                variant_keys: r.variant_keys,
                n: r.n,
                successes: r.successes,
                value_sum: 0.0,
                value_sq_sum: 0.0,
                num_sum: 0.0,
                den_sum: 0.0,
                num_sq_sum: 0.0,
                den_sq_sum: 0.0,
                num_den_sum: 0.0,
            })
            .collect())
    }

    async fn fetch_ratio_cells(
        &self,
        num_cfg: &AggregationConfig,
        den_cfg: &AggregationConfig,
        env_id: &str,
        exp_ids: &[&str],
        context_type: &str,
        interaction_end: DateTime<Utc>,
    ) -> Result<Vec<NdCellAggregate>, anyhow::Error> {
        let built = build_interaction_ratio_cells_query(
            num_cfg,
            den_cfg,
            env_id,
            exp_ids,
            context_type,
            interaction_end,
        )?;
        let query = bind_query(&self.client, built.sql, built.binds);
        let rows = query.fetch_all::<ChRatioCellRow>().await?;
        Ok(rows
            .into_iter()
            .map(|r| NdCellAggregate {
                variant_keys: r.variant_keys,
                n: r.n,
                successes: 0,
                value_sum: 0.0,
                value_sq_sum: 0.0,
                num_sum: r.num_sum,
                den_sum: r.den_sum,
                num_sq_sum: r.num_sq_sum,
                den_sq_sum: r.den_sq_sum,
                num_den_sum: r.num_den_sum,
            })
            .collect())
    }
}

// ── ClickHouse-backed writer ─────────────────────────────────────────────────

/// Serde adapter for a ClickHouse `Array(UUID)` column.
///
/// clickhouse 0.15 ships only a scalar [`clickhouse::serde::uuid`] helper (which
/// encodes a UUID as the `(u64, u64)` hi/lo pair the RowBinary UUID layout
/// expects) — there is no `uuid::vec`. This module serializes a `Vec<Uuid>` as a
/// sequence where each element is that same `(u64, u64)` pair, which the
/// RowBinary validator accepts for `Array(UUID)` (an outer `Seq` over the
/// `UUID`-as-2-tuple element type).
mod uuid_vec {
    use serde::de::{SeqAccess, Visitor};
    use serde::ser::SerializeSeq;
    use serde::{Deserializer, Serializer};
    use std::fmt;
    use uuid::Uuid;

    pub(super) fn serialize<S>(ids: &[Uuid], serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut seq = serializer.serialize_seq(Some(ids.len()))?;
        for id in ids {
            // Mirror clickhouse::serde::uuid for the non-human-readable
            // RowBinary path: the UUID is a (u64, u64) hi/lo pair.
            seq.serialize_element(&id.as_u64_pair())?;
        }
        seq.end()
    }

    // Kept for symmetry with `clickhouse::serde::uuid` and to document the wire
    // format; the production reader of this column lives in another crate, so
    // this side is currently unused here.
    #[allow(dead_code)]
    pub(super) fn deserialize<'de, D>(deserializer: D) -> Result<Vec<Uuid>, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct VecVisitor;
        impl<'de> Visitor<'de> for VecVisitor {
            type Value = Vec<Uuid>;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("an Array(UUID)")
            }
            fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
            where
                A: SeqAccess<'de>,
            {
                let mut out = Vec::new();
                while let Some((hi, lo)) = seq.next_element::<(u64, u64)>()? {
                    out.push(Uuid::from_u64_pair(hi, lo));
                }
                Ok(out)
            }
        }
        deserializer.deserialize_seq(VecVisitor)
    }
}

/// Row shape inserted into the `experiment_interactions` table — column list
/// matches the migrated schema exactly (env, ids array, order, term, context,
/// metric, count, cell_stats, Frequentist columns, Bayesian columns, time).
#[derive(Debug, Clone, Serialize, clickhouse::Row)]
struct ChInteractionRow {
    #[serde(with = "clickhouse::serde::uuid")]
    env_id: Uuid,
    #[serde(with = "uuid_vec")]
    experiment_ids: Vec<Uuid>,
    interaction_order: u8,
    term: String,
    context_type: String,
    metric_key: String,
    shared_count: u64,
    cell_stats: String,
    interaction_estimate: f64,
    p_value: f64,
    df: u32,
    significant: bool,
    insufficient_data: bool,
    bayes_prob: f64,
    bayes_expected: f64,
    bayes_ci_low: f64,
    bayes_ci_high: f64,
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
                experiment_ids: row.experiment_ids.clone(),
                interaction_order: row.interaction_order,
                term: row.term.clone(),
                context_type: row.context_type.clone(),
                metric_key: row.metric_key.clone(),
                shared_count: row.shared_count,
                cell_stats: row.cell_stats.clone(),
                interaction_estimate: row.interaction_estimate,
                p_value: row.p_value,
                df: row.df,
                significant: row.significant,
                insufficient_data: row.insufficient_data,
                bayes_prob: row.bayes_prob,
                bayes_expected: row.bayes_expected,
                bayes_ci_low: row.bayes_ci_low,
                bayes_ci_high: row.bayes_ci_high,
                computed_at: row.computed_at,
            })
            .await?;
        insert.end().await?;
        Ok(())
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

    /// Fake reader returning canned N-D cells per metric family; records the
    /// (env, exp_ids joined, context_type) it was asked for. `agg_cells` is also
    /// served for funnel reads (both are the binary path), and `ratio_cells` for
    /// ratio reads.
    #[derive(Default)]
    struct FakeReader {
        agg_cells: Vec<NdCellAggregate>,
        ratio_cells: Vec<NdCellAggregate>,
        calls: Mutex<Vec<(String, String, String)>>,
    }

    impl FakeReader {
        fn with_agg(cells: Vec<NdCellAggregate>) -> Self {
            Self {
                agg_cells: cells,
                ..Default::default()
            }
        }
        fn record(&self, env_id: &str, exp_ids: &[&str], context_type: &str) {
            self.calls.lock().unwrap().push((
                env_id.to_string(),
                exp_ids.join(","),
                context_type.to_string(),
            ));
        }
    }

    #[async_trait]
    impl InteractionCellReader for FakeReader {
        async fn fetch_aggregation_cells(
            &self,
            _cfg: &AggregationConfig,
            env_id: &str,
            exp_ids: &[&str],
            context_type: &str,
            _interaction_end: DateTime<Utc>,
        ) -> Result<Vec<NdCellAggregate>, anyhow::Error> {
            self.record(env_id, exp_ids, context_type);
            Ok(self.agg_cells.clone())
        }
        async fn fetch_funnel_cells(
            &self,
            _cfg: &stitchd_core::metric::FunnelConfig,
            env_id: &str,
            exp_ids: &[&str],
            context_type: &str,
            _interaction_end: DateTime<Utc>,
        ) -> Result<Vec<NdCellAggregate>, anyhow::Error> {
            self.record(env_id, exp_ids, context_type);
            Ok(self.agg_cells.clone())
        }
        async fn fetch_ratio_cells(
            &self,
            _num_cfg: &AggregationConfig,
            _den_cfg: &AggregationConfig,
            env_id: &str,
            exp_ids: &[&str],
            context_type: &str,
            _interaction_end: DateTime<Utc>,
        ) -> Result<Vec<NdCellAggregate>, anyhow::Error> {
            self.record(env_id, exp_ids, context_type);
            Ok(self.ratio_cells.clone())
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

    /// Build an N-D binary cell from a variant tuple.
    fn nd_binary(variants: &[&str], n: u64, succ: u64) -> NdCellAggregate {
        NdCellAggregate {
            variant_keys: variants.iter().map(|s| (*s).to_owned()).collect(),
            n,
            successes: succ,
            value_sum: 0.0,
            value_sq_sum: 0.0,
            num_sum: 0.0,
            den_sum: 0.0,
            num_sq_sum: 0.0,
            den_sq_sum: 0.0,
            num_den_sum: 0.0,
        }
    }

    /// A 2×2 conversion grid with a planted super-additive (1,1) corner.
    fn strong_2x2() -> Vec<NdCellAggregate> {
        vec![
            nd_binary(&["control", "control"], 100, 10),
            nd_binary(&["control", "treatment"], 100, 12),
            nd_binary(&["treatment", "control"], 100, 14),
            nd_binary(&["treatment", "treatment"], 100, 60),
        ]
    }

    #[test]
    fn nan_to_zero_sanitizes() {
        assert_eq!(nan_to_zero(f64::NAN), 0.0);
        assert!((nan_to_zero(1.5) - 1.5).abs() < f64::EPSILON);
    }

    #[test]
    fn bh_empty_family_is_empty() {
        assert!(benjamini_hochberg(&[], 0.05).is_empty());
    }

    #[test]
    fn bh_one_true_effect_among_many_nulls_survives() {
        // One genuinely small p-value (a true effect) plus 19 null p-values
        // sampled across (0,1). At a raw α=0.05 several nulls near the low end
        // would be spuriously "significant"; BH at FDR=0.05 must reject ONLY the
        // true effect.
        let mut pvalues = vec![0.0001]; // the true effect
        // 19 nulls, deliberately including some smallish ones (0.04, 0.06, …)
        // that a per-test α=0.05 would (partly) flag.
        let nulls = [
            0.04, 0.06, 0.11, 0.17, 0.23, 0.29, 0.35, 0.41, 0.47, 0.53, 0.59, 0.65, 0.71, 0.77,
            0.83, 0.89, 0.93, 0.97, 0.99,
        ];
        pvalues.extend_from_slice(&nulls);

        let reject = benjamini_hochberg(&pvalues, 0.05);
        assert!(reject[0], "the true effect (p=0.0001) must survive BH");
        assert_eq!(
            reject.iter().filter(|&&r| r).count(),
            1,
            "only the single true effect should be declared significant under BH"
        );

        // Sanity: a raw per-test α=0.05 would over-flag (the 0.04 null too).
        let raw_flagged = pvalues.iter().filter(|&&p| p <= 0.05).count();
        assert!(
            raw_flagged > 1,
            "raw α=0.05 over-flags ({raw_flagged}); BH is stricter"
        );
    }

    #[test]
    fn bh_all_nulls_rejects_nothing() {
        let pvalues = [0.2, 0.4, 0.6, 0.8, 0.95];
        let reject = benjamini_hochberg(&pvalues, 0.05);
        assert!(reject.iter().all(|&r| !r), "no nulls should be rejected");
    }

    #[test]
    fn bh_preserves_input_order() {
        // Smallest p is in the middle of the slice; its rejection must map back
        // to its original index.
        let pvalues = [0.9, 0.0001, 0.8];
        let reject = benjamini_hochberg(&pvalues, 0.05);
        assert_eq!(reject, vec![false, true, false]);
    }

    #[test]
    fn term_string_uses_actual_experiment_uuids() {
        let ids = vec![
            Uuid::from_u128(10),
            Uuid::from_u128(20),
            Uuid::from_u128(30),
        ];
        assert_eq!(
            term_string(&TermKind::Main { factor: 1 }, &ids),
            format!("main:{}", ids[1])
        );
        assert_eq!(
            term_string(&TermKind::TwoWay { a: 0, b: 2 }, &ids),
            format!("2way:{}x{}", ids[0], ids[2])
        );
        assert_eq!(
            term_string(&TermKind::ThreeWay { a: 0, b: 1, c: 2 }, &ids),
            format!("3way:{}x{}x{}", ids[0], ids[1], ids[2])
        );
    }

    #[tokio::test]
    async fn writes_three_term_rows_for_a_valid_pair() {
        // A 2-way candidate yields a 2-main + 1-two-way = 3-term decomposition.
        let metric = Uuid::new_v4();
        let env = Uuid::new_v4();
        let a = meta(Uuid::from_u128(1), Uuid::new_v4(), metric);
        let b = meta(Uuid::from_u128(2), Uuid::new_v4(), metric);
        let mut metrics = HashMap::new();
        metrics.insert(
            metric,
            agg_metric(metric, "checkout", AggregationOperator::Count),
        );

        let reader = FakeReader::with_agg(strong_2x2());
        let writer = FakeWriter::default();

        let n = compute_and_persist_interactions(
            &reader,
            &writer,
            env,
            &[a, b],
            &metrics,
            &["user".to_string()],
            ts(15),
            3,
        )
        .await
        .unwrap();

        assert_eq!(n, 3, "2 main effects + 1 two-way term");
        let rows = writer.rows.lock().unwrap();
        assert_eq!(rows.len(), 3);
        // Every row shares tuple-level metadata.
        let (lo, hi) = (
            Uuid::from_u128(1).min(Uuid::from_u128(2)),
            Uuid::from_u128(1).max(Uuid::from_u128(2)),
        );
        for r in rows.iter() {
            assert_eq!(r.metric_key, "checkout");
            assert_eq!(r.context_type, "user");
            assert_eq!(r.shared_count, 400);
            assert_eq!(r.interaction_order, 2);
            assert_eq!(r.experiment_ids, vec![lo, hi]);
            assert!(r.cell_stats.contains("variant_keys"));
        }
        // The two-way term must carry the expected term string and be significant.
        let two = rows
            .iter()
            .find(|r| r.term.starts_with("2way:"))
            .expect("two-way row present");
        assert_eq!(two.term, format!("2way:{lo}x{hi}"));
        assert!(
            two.significant,
            "planted super-additive pair is significant"
        );
        // Bayesian posterior attached (non-zero for a strong effect).
        assert!(
            two.bayes_prob > 0.0,
            "two-way posterior probability attached"
        );
        // Two main-effect rows.
        assert_eq!(
            rows.iter().filter(|r| r.term.starts_with("main:")).count(),
            2
        );
    }

    #[tokio::test]
    async fn funnel_metric_now_produces_rows() {
        // Funnel is in-scope for the N-way pipeline (binary path) — it must be
        // read and produce term rows, not skipped.
        let metric = Uuid::new_v4();
        let env = Uuid::new_v4();
        let a = meta(Uuid::from_u128(1), Uuid::new_v4(), metric);
        let b = meta(Uuid::from_u128(2), Uuid::new_v4(), metric);
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

        let reader = FakeReader::with_agg(strong_2x2());
        let writer = FakeWriter::default();

        let n = compute_and_persist_interactions(
            &reader,
            &writer,
            env,
            &[a, b],
            &metrics,
            &["user".to_string()],
            ts(15),
            3,
        )
        .await
        .unwrap();

        assert_eq!(
            n, 3,
            "funnel decomposition yields 3 terms (2 main + 1 two-way)"
        );
        assert_eq!(
            reader.calls.lock().unwrap().len(),
            1,
            "funnel metric IS read (one fetch for the pair × context_type)"
        );
        assert_eq!(writer.rows.lock().unwrap().len(), 3);
    }

    #[tokio::test]
    async fn ratio_metric_resolves_legs_and_produces_rows() {
        // A ratio metric whose two legs resolve to aggregation metrics is read
        // via the ratio path and produces term rows.
        let num_id = Uuid::new_v4();
        let den_id = Uuid::new_v4();
        let ratio_id = Uuid::new_v4();
        let env = Uuid::new_v4();
        let a = meta(Uuid::from_u128(1), Uuid::new_v4(), ratio_id);
        let b = meta(Uuid::from_u128(2), Uuid::new_v4(), ratio_id);

        let now = Utc::now();
        let ratio = MetricDefinition {
            id: MetricId::from_uuid(ratio_id),
            environment_id: EnvironmentId::new(),
            key: "completion_rate".into(),
            name: "r".into(),
            description: None,
            kind: MetricKind::Ratio(RatioConfig {
                numerator_metric_id: MetricId::from_uuid(num_id),
                denominator_metric_id: MetricId::from_uuid(den_id),
                min_denominator: 0,
            }),
            goal_direction: GoalDirection::Increase,
            version: 1,
            created_at: now,
            updated_at: now,
            deleted_at: None,
        };
        let mut metrics = HashMap::new();
        metrics.insert(ratio_id, ratio);
        metrics.insert(num_id, agg_metric(num_id, "num", AggregationOperator::Sum));
        metrics.insert(den_id, agg_metric(den_id, "den", AggregationOperator::Sum));

        // Ratio cells with non-degenerate second moments so the test can run.
        let ratio_cells = (0..2)
            .flat_map(|ai| {
                (0..2).map(move |bi| {
                    let av = if ai == 0 { "control" } else { "treatment" };
                    let bv = if bi == 0 { "control" } else { "treatment" };
                    NdCellAggregate {
                        variant_keys: vec![av.to_owned(), bv.to_owned()],
                        n: 100,
                        successes: 0,
                        value_sum: 0.0,
                        value_sq_sum: 0.0,
                        num_sum: 50.0 + (ai * bi) as f64 * 30.0,
                        den_sum: 100.0,
                        num_sq_sum: 40.0,
                        den_sq_sum: 120.0,
                        num_den_sum: 60.0,
                    }
                })
            })
            .collect::<Vec<_>>();

        let reader = FakeReader {
            ratio_cells,
            ..Default::default()
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
            3,
        )
        .await
        .unwrap();

        assert_eq!(
            n, 3,
            "ratio decomposition yields 3 terms (2 main + 1 two-way)"
        );
        let rows = writer.rows.lock().unwrap();
        assert!(rows.iter().any(|r| r.term.starts_with("2way:")));
        assert_eq!(rows[0].metric_key, "completion_rate");
    }

    #[tokio::test]
    async fn ratio_with_unresolvable_legs_is_skipped() {
        // Numerator metric absent from the map → ratio skipped, no read, no rows.
        let ratio_id = Uuid::new_v4();
        let env = Uuid::new_v4();
        let a = meta(Uuid::from_u128(1), Uuid::new_v4(), ratio_id);
        let b = meta(Uuid::from_u128(2), Uuid::new_v4(), ratio_id);
        let now = Utc::now();
        let ratio = MetricDefinition {
            id: MetricId::from_uuid(ratio_id),
            environment_id: EnvironmentId::new(),
            key: "completion_rate".into(),
            name: "r".into(),
            description: None,
            kind: MetricKind::Ratio(RatioConfig {
                numerator_metric_id: MetricId::new(),
                denominator_metric_id: MetricId::new(),
                min_denominator: 0,
            }),
            goal_direction: GoalDirection::Increase,
            version: 1,
            created_at: now,
            updated_at: now,
            deleted_at: None,
        };
        let mut metrics = HashMap::new();
        metrics.insert(ratio_id, ratio);

        let reader = FakeReader::default();
        let writer = FakeWriter::default();

        let n = compute_and_persist_interactions(
            &reader,
            &writer,
            env,
            &[a, b],
            &metrics,
            &["user".to_string()],
            ts(15),
            3,
        )
        .await
        .unwrap();

        assert_eq!(n, 0, "unresolvable ratio legs → skipped");
        assert!(reader.calls.lock().unwrap().is_empty(), "no CH read");
        assert!(writer.rows.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn three_way_candidate_emits_full_seven_term_decomposition() {
        // Three experiments on distinct flags sharing one conversion metric →
        // one candidate triple. The decomposition is 3 main + 3 two-way + 1
        // three-way = 7 term rows, all order=3, bayes attached, FDR applied.
        let metric = Uuid::new_v4();
        let env = Uuid::new_v4();
        let id_a = Uuid::from_u128(1);
        let id_b = Uuid::from_u128(2);
        let id_c = Uuid::from_u128(3);
        let a = meta(id_a, Uuid::new_v4(), metric);
        let b = meta(id_b, Uuid::new_v4(), metric);
        let c = meta(id_c, Uuid::new_v4(), metric);
        let mut metrics = HashMap::new();
        metrics.insert(
            metric,
            agg_metric(metric, "checkout", AggregationOperator::Count),
        );

        // 2×2×2 grid with a planted three-way bump in the (1,1,1) corner.
        let variants = ["control", "treatment"];
        let mut cells = Vec::new();
        for (ai, av) in variants.iter().enumerate() {
            for (bi, bv) in variants.iter().enumerate() {
                for (ci, cv) in variants.iter().enumerate() {
                    let succ = 100 + 120 * (ai * bi * ci) as u64;
                    cells.push(nd_binary(&[av, bv, cv], 1000, succ));
                }
            }
        }

        let reader = FakeReader::with_agg(cells);
        let writer = FakeWriter::default();

        let n = compute_and_persist_interactions(
            &reader,
            &writer,
            env,
            &[a, b, c],
            &metrics,
            &["user".to_string()],
            ts(15),
            3,
        )
        .await
        .unwrap();

        // Among 3 experiments sharing a metric the sweep enumerates 3 candidate
        // pairs (3 terms each = 9) AND 1 candidate triple (7 terms) = 16 rows.
        assert_eq!(n, 16, "3 pairs × 3 terms + 1 triple × 7 terms");
        let rows = writer.rows.lock().unwrap();
        assert_eq!(rows.len(), 16);

        // Sorted (lo, mid, hi) tuple from candidate_triples.
        let mut ids = [id_a, id_b, id_c];
        ids.sort();
        let [lo, mid, hi] = ids;

        // The three-way candidate's rows: the full 7-term hierarchical
        // decomposition, all order=3, on the sorted tuple.
        let triple_rows: Vec<&InteractionRow> =
            rows.iter().filter(|r| r.interaction_order == 3).collect();
        assert_eq!(
            triple_rows.len(),
            7,
            "the triple emits a 7-term decomposition"
        );
        for r in &triple_rows {
            assert_eq!(r.experiment_ids, vec![lo, mid, hi]);
            assert_eq!(r.context_type, "user");
            assert_eq!(r.metric_key, "checkout");
        }
        assert_eq!(
            triple_rows
                .iter()
                .filter(|r| r.term.starts_with("main:"))
                .count(),
            3
        );
        assert_eq!(
            triple_rows
                .iter()
                .filter(|r| r.term.starts_with("2way:"))
                .count(),
            3
        );
        let three: Vec<&&InteractionRow> = triple_rows
            .iter()
            .filter(|r| r.term.starts_with("3way:"))
            .collect();
        assert_eq!(three.len(), 1, "exactly one three-way term");
        // The three-way term names all three experiment UUIDs in factor order.
        assert_eq!(three[0].term, format!("3way:{lo}x{mid}x{hi}"));

        // The 9 pairwise rows are order=2.
        assert_eq!(rows.iter().filter(|r| r.interaction_order == 2).count(), 9);

        // Bayes attached to terms (populated from the merge; non-zero for the
        // well-powered grid).
        let bayes_attached = rows
            .iter()
            .any(|r| r.bayes_prob != 0.0 || r.bayes_expected != 0.0);
        assert!(bayes_attached, "Bayesian posteriors merged into terms");
        // FDR applied across the whole 16-test family: at least one term in the
        // well-powered planted grid clears the BH threshold.
        assert!(
            rows.iter().any(|r| r.significant),
            "well-powered planted interaction yields at least one significant term"
        );
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
        metrics.insert(
            metric,
            agg_metric(metric, "checkout", AggregationOperator::Count),
        );

        let reader = FakeReader::with_agg(strong_2x2());
        let writer = FakeWriter::default();

        let n = compute_and_persist_interactions(
            &reader,
            &writer,
            env,
            &[a, b],
            &metrics,
            &["user".to_string()],
            ts(15),
            3,
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
            sequential_testing_enabled: false,
            sequential_alpha: 0.05,
            sequential_tau_squared: None,
            sequential_min_sample_size: 100,
            experiment_mode: stitchd_core::experimentation::bandit::ExperimentMode::Fixed,
            bandit_config: None,
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

        let reader = FakeReader::with_agg(strong_2x2());
        let writer = FakeWriter::default();

        let n = run_interaction_sweep(&reader, &writer, &repo, &[a, b], Utc::now(), 3)
            .await
            .unwrap();
        assert_eq!(n, 3, "one pair × one metric × one context → 3 term rows");
        let rows = writer.rows.lock().unwrap();
        assert_eq!(rows.len(), 3);
        assert!(rows.iter().all(|r| r.env_id == env.as_uuid()));
        assert!(rows.iter().all(|r| r.context_type == "user"));
        assert!(
            rows.iter()
                .any(|r| r.term.starts_with("2way:") && r.significant),
            "the planted two-way term is significant"
        );
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

        let reader = FakeReader::with_agg(strong_2x2());
        let writer = FakeWriter::default();

        let n = run_interaction_sweep(&reader, &writer, &repo, &[a, b], Utc::now(), 3)
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
        metrics.insert(
            metric,
            agg_metric(metric, "checkout", AggregationOperator::Count),
        );

        let reader = FakeReader::default(); // empty grids
        let writer = FakeWriter::default();

        let n = compute_and_persist_interactions(
            &reader,
            &writer,
            env,
            &[a, b],
            &metrics,
            &["user".to_string()],
            ts(15),
            3,
        )
        .await
        .unwrap();

        assert_eq!(n, 0, "no shared population → no row");
        assert!(writer.rows.lock().unwrap().is_empty());
    }

    // ── Phase 10: operator-bounded order 4+ ───────────────────────────────────

    /// A reader that returns an `order`-dimensional fully-populated conversion
    /// grid sized to the requested tuple arity (variant_keys length == arity), so
    /// the sweep can drive any order with a self-consistent grid. The (all-top)
    /// corner carries a strong lift so the top-order interaction is detectable.
    #[derive(Default)]
    struct ArityReader;

    fn arity_grid(order: usize) -> Vec<NdCellAggregate> {
        // Full 2^order grid over {control, treatment}; plant a big lift only in
        // the all-"treatment" corner so the top-order interaction is non-trivial.
        let mut cells = Vec::with_capacity(1 << order);
        for corner in 0u32..(1u32 << order) {
            let variants: Vec<&str> = (0..order)
                .map(|s| {
                    if corner & (1 << s) != 0 {
                        "treatment"
                    } else {
                        "control"
                    }
                })
                .collect();
            let all_top = corner == (1u32 << order) - 1;
            let succ = if all_top { 600 } else { 100 };
            cells.push(nd_binary(&variants, 1000, succ));
        }
        cells
    }

    #[async_trait]
    impl InteractionCellReader for ArityReader {
        async fn fetch_aggregation_cells(
            &self,
            _cfg: &AggregationConfig,
            _env_id: &str,
            exp_ids: &[&str],
            _context_type: &str,
            _interaction_end: DateTime<Utc>,
        ) -> Result<Vec<NdCellAggregate>, anyhow::Error> {
            Ok(arity_grid(exp_ids.len()))
        }
        async fn fetch_funnel_cells(
            &self,
            _cfg: &stitchd_core::metric::FunnelConfig,
            _env_id: &str,
            exp_ids: &[&str],
            _context_type: &str,
            _interaction_end: DateTime<Utc>,
        ) -> Result<Vec<NdCellAggregate>, anyhow::Error> {
            Ok(arity_grid(exp_ids.len()))
        }
        async fn fetch_ratio_cells(
            &self,
            _num_cfg: &AggregationConfig,
            _den_cfg: &AggregationConfig,
            _env_id: &str,
            _exp_ids: &[&str],
            _context_type: &str,
            _interaction_end: DateTime<Utc>,
        ) -> Result<Vec<NdCellAggregate>, anyhow::Error> {
            Ok(Vec::new())
        }
    }

    /// With four overlapping experiments sharing one metric and `max_order = 4`,
    /// the sweep emits every tuple of order 2..=4 with its full hierarchical
    /// decomposition, INCLUDING the single order-4 tuple's 15 terms with the top
    /// `4way:` term present. A `main:` term distinguishes per-tuple rows.
    #[tokio::test]
    async fn order4_sweep_emits_fourway_terms() {
        let metric = Uuid::new_v4();
        let env = Uuid::new_v4();
        let exps: Vec<ExperimentMeta> = (1..=4)
            .map(|i| meta(Uuid::from_u128(i), Uuid::new_v4(), metric))
            .collect();
        let mut metrics = HashMap::new();
        metrics.insert(
            metric,
            agg_metric(metric, "checkout", AggregationOperator::Count),
        );

        let reader = ArityReader;
        let writer = FakeWriter::default();
        let n = compute_and_persist_interactions(
            &reader,
            &writer,
            env,
            &exps,
            &metrics,
            &["user".to_string()],
            ts(15),
            4,
        )
        .await
        .unwrap();

        // Tuple counts: C(4,2)=6 pairs ×3, C(4,3)=4 triples ×7, C(4,4)=1 quad ×15.
        let expected = 6 * 3 + 4 * 7 + 15;
        assert_eq!(
            n, expected,
            "pairs + triples + the order-4 quad decomposition"
        );

        let rows = writer.rows.lock().unwrap();
        // Exactly one order-4 tuple, with 15 terms, the top one a `4way:`.
        let order4: Vec<_> = rows.iter().filter(|r| r.interaction_order == 4).collect();
        assert_eq!(order4.len(), 15, "the single quad's full hierarchical set");
        let fourway = order4
            .iter()
            .filter(|r| r.term.starts_with("4way:"))
            .count();
        assert_eq!(fourway, 1, "exactly one top four-way term");
        let threeway_subsets = order4
            .iter()
            .filter(|r| r.term.starts_with("3way:"))
            .count();
        assert_eq!(
            threeway_subsets, 4,
            "all four three-way subsets within the quad"
        );
        // The persisted experiment_ids on a 4-way row name all four (sorted).
        let four_row = order4.iter().find(|r| r.term.starts_with("4way:")).unwrap();
        assert_eq!(four_row.experiment_ids.len(), 4);
        let mut sorted = four_row.experiment_ids.clone();
        sorted.sort();
        assert_eq!(four_row.experiment_ids, sorted, "ids sorted ascending");
    }

    /// The default cap (`max_order = 3`) over the SAME four experiments emits
    /// NO order-4 rows — the cap is enforced (pairs + triples only).
    #[tokio::test]
    async fn default_cap_excludes_order4() {
        let metric = Uuid::new_v4();
        let env = Uuid::new_v4();
        let exps: Vec<ExperimentMeta> = (1..=4)
            .map(|i| meta(Uuid::from_u128(i), Uuid::new_v4(), metric))
            .collect();
        let mut metrics = HashMap::new();
        metrics.insert(
            metric,
            agg_metric(metric, "checkout", AggregationOperator::Count),
        );

        let reader = ArityReader;
        let writer = FakeWriter::default();
        let n = compute_and_persist_interactions(
            &reader,
            &writer,
            env,
            &exps,
            &metrics,
            &["user".to_string()],
            ts(15),
            3, // default cap
        )
        .await
        .unwrap();

        // C(4,2)=6 pairs ×3 + C(4,3)=4 triples ×7 = 18 + 28 = 46; no order-4.
        assert_eq!(n, 6 * 3 + 4 * 7);
        let rows = writer.rows.lock().unwrap();
        assert!(
            rows.iter().all(|r| r.interaction_order <= 3),
            "the order-3 cap must exclude any order-4 row"
        );
    }
}

#[cfg(test)]
mod nd_cell_tests {
    use super::{NdCellAggregate, cells_from_json, cells_to_json, dense_levels, level_indices};

    fn cell(variants: &[&str], n: u64, successes: u64) -> NdCellAggregate {
        NdCellAggregate {
            variant_keys: variants.iter().map(|s| (*s).to_owned()).collect(),
            n,
            successes,
            value_sum: n as f64 * 1.5,
            value_sq_sum: n as f64 * 3.0,
            num_sum: successes as f64,
            den_sum: n as f64,
            num_sq_sum: successes as f64,
            den_sq_sum: n as f64,
            num_den_sum: successes as f64,
        }
    }

    #[test]
    fn order_is_variant_arity() {
        assert_eq!(cell(&["a", "b"], 10, 3).order(), 2);
        assert_eq!(cell(&["a", "b", "c"], 10, 3).order(), 3);
    }

    #[test]
    fn json_round_trips_pairwise() {
        let cells = vec![
            cell(&["control", "on"], 100, 20),
            cell(&["t1", "off"], 50, 5),
        ];
        let json = cells_to_json(&cells);
        let back = cells_from_json(&json).expect("valid json");
        assert_eq!(cells, back);
    }

    #[test]
    fn json_round_trips_three_way() {
        let cells = vec![
            cell(&["control", "on", "blue"], 100, 20),
            cell(&["treat", "off", "red"], 80, 30),
        ];
        let json = cells_to_json(&cells);
        let back = cells_from_json(&json).expect("valid json");
        assert_eq!(cells, back);
        // every variant tuple has arity 3
        assert!(back.iter().all(|c| c.order() == 3));
    }

    #[test]
    fn bad_json_is_an_error_not_a_panic() {
        assert!(cells_from_json("not json").is_err());
    }

    #[test]
    fn dense_levels_are_sorted_distinct_per_dimension() {
        // dim0: {control, treat}; dim1: {off, on}; dim2: {blue, green, red}
        let cells = vec![
            cell(&["treat", "on", "red"], 1, 0),
            cell(&["control", "off", "blue"], 1, 0),
            cell(&["control", "on", "green"], 1, 0),
        ];
        let levels = dense_levels(&cells, 3);
        assert_eq!(levels[0], vec!["control", "treat"]);
        assert_eq!(levels[1], vec!["off", "on"]);
        assert_eq!(levels[2], vec!["blue", "green", "red"]);
    }

    #[test]
    fn level_indices_map_tuple_to_dense_grid_position() {
        let cells = vec![
            cell(&["treat", "on", "red"], 1, 0),
            cell(&["control", "off", "blue"], 1, 0),
            cell(&["control", "on", "green"], 1, 0),
        ];
        let levels = dense_levels(&cells, 3);
        // ("treat","on","red") -> (1, 1, 2)
        assert_eq!(level_indices(&cells[0], &levels), Some(vec![1, 1, 2]));
        // ("control","off","blue") -> (0, 0, 0)
        assert_eq!(level_indices(&cells[1], &levels), Some(vec![0, 0, 0]));
        // ("control","on","green") -> (0, 1, 1)
        assert_eq!(level_indices(&cells[2], &levels), Some(vec![0, 1, 1]));
    }

    #[test]
    fn level_indices_guards_arity_mismatch() {
        let levels = dense_levels(&[cell(&["a", "b"], 1, 0)], 2);
        // a 3-arity cell against 2-D levels → None (misuse guard)
        assert_eq!(level_indices(&cell(&["a", "b", "c"], 1, 0), &levels), None);
    }
}
