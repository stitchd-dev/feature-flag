//! Sequential (always-valid mSPRT) result computation for the scheduled
//! stats-service pass (Phase 3).
//!
//! This module turns the per-variant sufficient statistics already gathered for
//! the frequentist / bayesian analyses into the `sequential_result` JSON blob
//! that mirrors `frequentist_result` / `bayesian_result`: one row per
//! `(experiment, iteration, metric_key, context_type)`, with the per-variant
//! breakdown packed as a JSON object keyed by `variant_key`.
//!
//! ## JSON shape (parsed by the read path / W4)
//!
//! ```json
//! {
//!   "<variant_key>": {
//!     "always_valid_p": <f64>,
//!     "p_crossed": <bool>,
//!     "ci_lower": <f64|null>,
//!     "ci_upper": <f64|null>,
//!     "insufficient_data": <bool>,
//!     "method": "msprt"
//!   },
//!   ...
//! }
//! ```
//!
//! The **control** variant is always included as a baseline entry
//! (`always_valid_p:1.0, p_crossed:false, ci_lower:0.0, ci_upper:0.0,
//! insufficient_data:false, method:"msprt"`). Every non-control variant is
//! tested against control with the matching family adapter using a
//! Bonferroni-split α (`split_alpha`).
//!
//! `ci_lower` / `ci_upper` are emitted as JSON `null` whenever the adapter
//! returns a non-finite bound (the insufficient-data case returns the whole
//! real line `(-∞, +∞)`); strict JSON cannot encode `±Infinity`, and `null`
//! cleanly signals "no usable interval at this look".
//!
//! ## Config resolution & the τ² default
//!
//! [`resolve_sequential_config`] reads the four sequential fields snapshotted on
//! the `ExperimentIteration` (Phase 2). When `sequential_tau_squared` is absent
//! the caller supplies a metric-scale default via [`default_tau_squared`] — a
//! unit-information-style choice equal to the pooled variance of the metric's
//! effect scale (`p̄(1−p̄)` for proportions, pooled sample variance for numeric),
//! clamped to a small positive floor so the mixture variance is always valid.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::{Value, json};
use uuid::Uuid;

use stitchd_core::experimentation::stats::VariantStats;
use stitchd_core::experimentation::stats::sequential::{
    RatioGroupStats, SequentialConfig, SequentialResult, sequential_count, sequential_funnel,
    sequential_numeric, sequential_ratio, split_alpha,
};

/// The smallest positive mixing variance we will ever feed the mSPRT engine.
///
/// `tau_squared` must be strictly positive (the engine collapses a non-positive
/// value to `insufficient_data`). When a metric is degenerate (e.g. every unit
/// converted, so `p̄(1−p̄) = 0`) the derived default would be zero; we clamp up
/// to this floor so the test still runs (it will simply be very sensitive).
pub const TAU_SQUARED_FLOOR: f64 = 1e-9;

/// Which sequential adapter applies to a metric, derived from its metric-type
/// discriminant (`"count"` / `"conversion"`, `"numeric"` / `"continuous"`,
/// `"funnel"`, `"ratio"`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SequentialMetricFamily {
    /// Count / Bernoulli proportion difference → [`sequential_count`].
    Count,
    /// Continuous numeric mean difference → [`sequential_numeric`].
    Numeric,
    /// Funnel final-step conversion-rate difference → [`sequential_funnel`].
    Funnel,
    /// Ratio (delta method) → [`sequential_ratio`].
    Ratio,
}

impl SequentialMetricFamily {
    /// Map a metric-type / kind string to a sequential family.
    ///
    /// Returns `None` for unrecognised discriminants (e.g. `"percentile"`,
    /// which has no closed-form always-valid analogue here) so the caller can
    /// skip sequential for that metric.
    #[must_use]
    pub fn from_metric_type(metric_type: &str) -> Option<Self> {
        match metric_type.trim().to_ascii_lowercase().as_str() {
            "count" | "conversion" | "binomial" | "bernoulli" => Some(Self::Count),
            "numeric" | "continuous" | "mean" | "revenue" | "duration" => Some(Self::Numeric),
            "funnel" => Some(Self::Funnel),
            "ratio" => Some(Self::Ratio),
            _ => None,
        }
    }
}

/// Resolve the per-experiment [`SequentialConfig`] from the iteration snapshot.
///
/// `enabled` gates the whole computation upstream; this helper is only about
/// turning the snapshotted `(alpha, tau_squared, min_sample_size)` into a
/// concrete config. When `tau_squared` is `None`, `default_tau` is used (the
/// caller derives it from the metric's variant stats via
/// [`default_tau_squared`]). The α and min-sample-size are taken verbatim; the
/// engine itself rejects an out-of-range α as `insufficient_data`.
#[must_use]
pub fn resolve_sequential_config(
    alpha: f64,
    tau_squared: Option<f64>,
    min_sample_size: i64,
    default_tau: f64,
) -> SequentialConfig {
    let tau = tau_squared
        .filter(|t| t.is_finite() && *t > 0.0)
        .unwrap_or(default_tau)
        .max(TAU_SQUARED_FLOOR);
    SequentialConfig {
        alpha,
        tau_squared: tau,
        min_sample_size,
    }
}

/// Unit-information-style τ² default from per-variant stats.
///
/// The mixing prior `effect ~ N(0, τ²)` is most powerful for effects of size
/// `≈ τ`, so a natural scale-free default is the *pooled variance of the effect
/// scale*:
///
/// * **Proportions** (count / funnel): the effect is a difference of rates, so
///   we use the pooled Bernoulli variance `p̄(1−p̄)` where `p̄` is the
///   sample-size-weighted pooled rate across all variants.
/// * **Numeric**: the effect is a mean difference, so we use the pooled sample
///   variance across all variants (sample-size-weighted average of each
///   variant's `variance`).
///
/// The result is clamped to [`TAU_SQUARED_FLOOR`] so it is always a valid
/// strictly-positive mixing variance. Returns [`TAU_SQUARED_FLOOR`] when no
/// usable statistics are present.
#[must_use]
pub fn default_tau_squared(family: SequentialMetricFamily, stats: &[&VariantStats]) -> f64 {
    let tau = match family {
        SequentialMetricFamily::Count | SequentialMetricFamily::Funnel => {
            // Pooled rate p̄ across variants, then p̄(1−p̄).
            let mut conv = 0.0_f64;
            let mut n = 0.0_f64;
            for s in stats {
                let rate = match family {
                    SequentialMetricFamily::Funnel => s.conversion_rate,
                    _ => s
                        .conversions
                        .map(|c| c as f64 / (s.sample_size.max(1) as f64)),
                };
                if let Some(r) = rate {
                    let ni = s.sample_size.max(0) as f64;
                    conv += r * ni;
                    n += ni;
                }
            }
            if n > 0.0 {
                let p = (conv / n).clamp(0.0, 1.0);
                p * (1.0 - p)
            } else {
                0.0
            }
        }
        SequentialMetricFamily::Numeric => {
            // Sample-size-weighted pooled variance across variants.
            let mut weighted = 0.0_f64;
            let mut n = 0.0_f64;
            for s in stats {
                if let Some(v) = s.variance.filter(|v| v.is_finite() && *v >= 0.0) {
                    let ni = s.sample_size.max(0) as f64;
                    weighted += v * ni;
                    n += ni;
                }
            }
            if n > 0.0 { weighted / n } else { 0.0 }
        }
        // Ratio's effect scale is data-dependent and the sufficient stats live
        // off `VariantStats`; fall back to the floor (a small, very-sensitive
        // mixing variance) unless the caller overrides with an explicit τ².
        SequentialMetricFamily::Ratio => 0.0,
    };
    if tau.is_finite() && tau > 0.0 {
        tau
    } else {
        TAU_SQUARED_FLOOR
    }
}

/// The baseline JSON entry written for the control variant.
fn control_entry() -> Value {
    json!({
        "always_valid_p": 1.0,
        "p_crossed": false,
        "ci_lower": 0.0,
        "ci_upper": 0.0,
        "insufficient_data": false,
        "method": "msprt",
    })
}

/// Serialise a [`SequentialResult`] into its per-variant JSON entry, mapping
/// non-finite CI bounds to JSON `null`.
fn result_to_entry(r: &SequentialResult) -> Value {
    let ci = |x: f64| -> Value { if x.is_finite() { json!(x) } else { Value::Null } };
    json!({
        "always_valid_p": r.always_valid_p,
        "p_crossed": r.p_crossed,
        "ci_lower": ci(r.ci_lower),
        "ci_upper": ci(r.ci_upper),
        "insufficient_data": r.insufficient_data,
        "method": r.method,
    })
}

/// Look up a variant's prior running-minimum `always_valid_p`, defaulting to
/// `1.0` (the seed) when absent or unparseable.
fn prev_p_for(prev: &HashMap<String, f64>, variant_key: &str) -> f64 {
    prev.get(variant_key)
        .copied()
        .filter(|p| p.is_finite())
        .unwrap_or(1.0)
}

/// Compute the `sequential_result` JSON blob for a single
/// `(metric_key, context_type)` row from per-variant [`VariantStats`].
///
/// `control_key` identifies the baseline variant; it is always present in the
/// output. Every other key in `variant_stats` is tested against control with
/// the `family` adapter at a Bonferroni-split α (`split_alpha` over the total
/// number of variants). `prev_p_by_variant` carries each variant's prior
/// running-minimum p (seed `1.0`).
///
/// Returns `None` when there is no control entry in `variant_stats` (nothing to
/// compare against), so the caller can omit `sequential_result` for the row.
#[must_use]
pub fn compute_sequential_blob(
    family: SequentialMetricFamily,
    control_key: &str,
    variant_stats: &HashMap<String, VariantStats>,
    cfg: &SequentialConfig,
    prev_p_by_variant: &HashMap<String, f64>,
) -> Option<Value> {
    let control = variant_stats.get(control_key)?;
    let num_variants = variant_stats.len() as i64;
    let split = split_alpha(cfg, num_variants);

    let mut obj = serde_json::Map::with_capacity(variant_stats.len());
    obj.insert(control_key.to_string(), control_entry());

    for (variant_key, vstats) in variant_stats {
        if variant_key == control_key {
            continue;
        }
        let prev_p = prev_p_for(prev_p_by_variant, variant_key);
        let result = match family {
            SequentialMetricFamily::Count => sequential_count(control, vstats, &split, prev_p),
            SequentialMetricFamily::Numeric => sequential_numeric(control, vstats, &split, prev_p),
            SequentialMetricFamily::Funnel => sequential_funnel(control, vstats, &split, prev_p),
            // Ratio sufficient stats are not carried on VariantStats; callers
            // with ratio metrics must use `compute_sequential_blob_ratio`.
            SequentialMetricFamily::Ratio => SequentialResult {
                always_valid_p: 1.0,
                p_crossed: false,
                ci_lower: f64::NEG_INFINITY,
                ci_upper: f64::INFINITY,
                method: "msprt".to_string(),
                insufficient_data: true,
            },
        };
        obj.insert(variant_key.clone(), result_to_entry(&result));
    }

    Some(Value::Object(obj))
}

/// Ratio variant of [`compute_sequential_blob`] taking per-group delta-method
/// sufficient statistics ([`RatioGroupStats`]) instead of [`VariantStats`].
///
/// Used when the metric family is [`SequentialMetricFamily::Ratio`] and the
/// ratio query has aggregated the required per-group sums
/// (`num_sum, den_sum, num_sq_sum, den_sq_sum, num_den_sum`). Same JSON shape
/// and Bonferroni-split semantics as the count/numeric/funnel path.
#[must_use]
pub fn compute_sequential_blob_ratio(
    control_key: &str,
    group_stats: &HashMap<String, RatioGroupStats>,
    cfg: &SequentialConfig,
    prev_p_by_variant: &HashMap<String, f64>,
) -> Option<Value> {
    let control = group_stats.get(control_key)?;
    let num_variants = group_stats.len() as i64;
    let split = split_alpha(cfg, num_variants);

    let mut obj = serde_json::Map::with_capacity(group_stats.len());
    obj.insert(control_key.to_string(), control_entry());

    for (variant_key, vstats) in group_stats {
        if variant_key == control_key {
            continue;
        }
        let prev_p = prev_p_for(prev_p_by_variant, variant_key);
        let result = sequential_ratio(control, vstats, &split, prev_p);
        obj.insert(variant_key.clone(), result_to_entry(&result));
    }

    Some(Value::Object(obj))
}

/// Parse a previously-stored `sequential_result` JSON blob into a
/// `variant_key → always_valid_p` map suitable for use as the next look's
/// `prev_p_by_variant`.
///
/// Missing / malformed entries are simply skipped — the caller defaults any
/// absent variant to the `1.0` seed, which is the correct running-minimum
/// starting point. An empty / non-object input yields an empty map.
#[must_use]
pub fn parse_prev_always_valid_p(prior_blob: &str) -> HashMap<String, f64> {
    let mut out = HashMap::new();
    if prior_blob.trim().is_empty() {
        return out;
    }
    let Ok(Value::Object(map)) = serde_json::from_str::<Value>(prior_blob) else {
        return out;
    };
    for (variant_key, entry) in map {
        if let Some(p) = entry
            .get("always_valid_p")
            .and_then(Value::as_f64)
            .filter(|p| p.is_finite())
        {
            out.insert(variant_key, p);
        }
    }
    out
}

// ── Running-minimum prior fetch (Phase 3.2) ─────────────────────────────────────

/// One column projected from the prior `experiment_results` row.
#[derive(Debug, Clone, Deserialize, clickhouse::Row)]
struct PriorSequentialRow {
    sequential_result: String,
}

/// Fetch the most-recent prior `sequential_result` JSON blob for a result
/// tuple, parsed into a `variant_key → always_valid_p` map for use as the next
/// look's `prev_p_by_variant`.
///
/// "Most-recent prior" = the row with the greatest `computed_at` **strictly
/// before** `now` for the exact `(env_id, experiment_id, iteration_id,
/// metric_key, context_type)` tuple (recall result rows store the per-variant
/// breakdown in JSON with `variant_key = ''`, so the variant key is *not* part
/// of the lookup). Passing the current tick's `computed_at` as `now` excludes
/// the row being written this tick.
///
/// Returns an empty map (→ every variant seeds at `1.0`) when there is no prior
/// row, the blob is empty, or the query / parse fails for any reason — the
/// running-minimum is seeded conservatively rather than propagating an error
/// that would abort the whole compute pass.
pub async fn fetch_prev_always_valid_p(
    client: &clickhouse::Client,
    env_id: Uuid,
    experiment_id: Uuid,
    iteration_id: Uuid,
    metric_key: &str,
    context_type: &str,
    now: DateTime<Utc>,
) -> HashMap<String, f64> {
    let now_ms = now.timestamp_millis();
    // `sequential_result != ''` so we skip historical rows written before this
    // feature existed (they carry an empty blob) and find the latest row that
    // actually has a sequential payload to seed the running minimum from.
    let sql = "SELECT sequential_result FROM experiment_results \
         WHERE env_id = ? AND experiment_id = ? AND iteration_id = ? \
           AND metric_key = ? AND context_type = ? \
           AND sequential_result != '' \
           AND computed_at < fromUnixTimestamp64Milli(?) \
         ORDER BY computed_at DESC \
         LIMIT 1";

    let fetched = client
        .query(sql)
        .bind(env_id)
        .bind(experiment_id)
        .bind(iteration_id)
        .bind(metric_key)
        .bind(context_type)
        .bind(now_ms)
        .fetch_optional::<PriorSequentialRow>()
        .await;

    match fetched {
        Ok(Some(row)) => parse_prev_always_valid_p(&row.sequential_result),
        Ok(None) => HashMap::new(),
        Err(e) => {
            tracing::warn!(
                experiment_id = %experiment_id,
                metric_key,
                context_type,
                "failed to fetch prior sequential_result for running-min seed: {e}"
            );
            HashMap::new()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn count_stats(n: i64, conversions: i64) -> VariantStats {
        VariantStats {
            sample_size: n,
            conversions: Some(conversions),
            mean: None,
            variance: None,
            conversion_rate: Some(conversions as f64 / n as f64),
            percentiles: None,
        }
    }

    fn numeric_stats(n: i64, mean: f64, variance: f64) -> VariantStats {
        VariantStats {
            sample_size: n,
            conversions: None,
            mean: Some(mean),
            variance: Some(variance),
            conversion_rate: None,
            percentiles: None,
        }
    }

    // ── family mapping ────────────────────────────────────────────────────────

    #[test]
    fn family_from_metric_type_maps_known_kinds() {
        assert_eq!(
            SequentialMetricFamily::from_metric_type("count"),
            Some(SequentialMetricFamily::Count)
        );
        assert_eq!(
            SequentialMetricFamily::from_metric_type("Conversion"),
            Some(SequentialMetricFamily::Count)
        );
        assert_eq!(
            SequentialMetricFamily::from_metric_type("numeric"),
            Some(SequentialMetricFamily::Numeric)
        );
        assert_eq!(
            SequentialMetricFamily::from_metric_type("funnel"),
            Some(SequentialMetricFamily::Funnel)
        );
        assert_eq!(
            SequentialMetricFamily::from_metric_type("ratio"),
            Some(SequentialMetricFamily::Ratio)
        );
        assert_eq!(SequentialMetricFamily::from_metric_type("percentile"), None);
        assert_eq!(SequentialMetricFamily::from_metric_type("bogus"), None);
    }

    // ── τ² default ──────────────────────────────────────────────────────────────

    #[test]
    fn default_tau_for_proportion_is_pooled_bernoulli_variance() {
        // control 100/1000 = 0.1, treatment 150/1000 = 0.15 → p̄ = 0.125.
        let c = count_stats(1000, 100);
        let t = count_stats(1000, 150);
        let tau = default_tau_squared(SequentialMetricFamily::Count, &[&c, &t]);
        let expected = 0.125 * (1.0 - 0.125);
        assert!((tau - expected).abs() < 1e-12, "got {tau}, want {expected}");
    }

    #[test]
    fn default_tau_for_numeric_is_pooled_variance() {
        let c = numeric_stats(500, 10.0, 4.0);
        let t = numeric_stats(1500, 11.0, 8.0);
        let tau = default_tau_squared(SequentialMetricFamily::Numeric, &[&c, &t]);
        // (4*500 + 8*1500) / 2000 = (2000 + 12000)/2000 = 7.0
        assert!((tau - 7.0).abs() < 1e-12, "got {tau}");
    }

    #[test]
    fn default_tau_degenerate_clamps_to_floor() {
        // Everyone converted → p̄(1−p̄) = 0 → clamp to floor.
        let c = count_stats(100, 100);
        let t = count_stats(100, 100);
        let tau = default_tau_squared(SequentialMetricFamily::Count, &[&c, &t]);
        assert_eq!(tau, TAU_SQUARED_FLOOR);
    }

    #[test]
    fn resolve_config_prefers_explicit_tau_else_default() {
        let with_explicit = resolve_sequential_config(0.05, Some(2.5), 100, 7.0);
        assert!((with_explicit.tau_squared - 2.5).abs() < 1e-12);
        let with_default = resolve_sequential_config(0.05, None, 100, 7.0);
        assert!((with_default.tau_squared - 7.0).abs() < 1e-12);
        // Non-positive explicit τ² falls back to the default.
        let bad_explicit = resolve_sequential_config(0.05, Some(-1.0), 100, 7.0);
        assert!((bad_explicit.tau_squared - 7.0).abs() < 1e-12);
    }

    // ── blob shape ────────────────────────────────────────────────────────────

    #[test]
    fn blob_includes_control_baseline_and_per_variant_entries() {
        let mut stats = HashMap::new();
        stats.insert("control".to_string(), count_stats(2000, 200));
        stats.insert("treatment".to_string(), count_stats(2000, 400));
        let cfg = SequentialConfig {
            alpha: 0.05,
            tau_squared: 0.1,
            min_sample_size: 100,
        };
        let blob = compute_sequential_blob(
            SequentialMetricFamily::Count,
            "control",
            &stats,
            &cfg,
            &HashMap::new(),
        )
        .expect("control present → Some");
        let obj = blob.as_object().unwrap();

        // Control baseline.
        assert_eq!(obj["control"]["always_valid_p"], json!(1.0));
        assert_eq!(obj["control"]["p_crossed"], json!(false));
        assert_eq!(obj["control"]["method"], json!("msprt"));

        // Treatment entry exists, method tagged, fields present.
        let tr = &obj["treatment"];
        assert_eq!(tr["method"], json!("msprt"));
        assert!(tr.get("always_valid_p").is_some());
        assert!(tr.get("ci_lower").is_some());
        assert!(tr.get("insufficient_data").is_some());
        // A 200/2000 vs 400/2000 lift on 2000 units should be detected.
        assert_eq!(tr["p_crossed"], json!(true));
        assert_eq!(tr["insufficient_data"], json!(false));
    }

    #[test]
    fn blob_returns_none_without_control() {
        let mut stats = HashMap::new();
        stats.insert("treatment".to_string(), count_stats(2000, 400));
        let cfg = SequentialConfig::default();
        assert!(
            compute_sequential_blob(
                SequentialMetricFamily::Count,
                "control",
                &stats,
                &cfg,
                &HashMap::new(),
            )
            .is_none()
        );
    }

    #[test]
    fn insufficient_data_emits_null_ci_bounds() {
        // n below min_sample_size → insufficient → CI is (-inf, +inf) → null.
        let mut stats = HashMap::new();
        stats.insert("control".to_string(), count_stats(10, 1));
        stats.insert("treatment".to_string(), count_stats(10, 2));
        let cfg = SequentialConfig {
            alpha: 0.05,
            tau_squared: 0.1,
            min_sample_size: 100,
        };
        let blob = compute_sequential_blob(
            SequentialMetricFamily::Count,
            "control",
            &stats,
            &cfg,
            &HashMap::new(),
        )
        .unwrap();
        let tr = &blob.as_object().unwrap()["treatment"];
        assert_eq!(tr["insufficient_data"], json!(true));
        assert_eq!(tr["always_valid_p"], json!(1.0));
        assert_eq!(tr["ci_lower"], Value::Null);
        assert_eq!(tr["ci_upper"], Value::Null);
    }

    // ── prev_p parse / round-trip ───────────────────────────────────────────────

    #[test]
    fn parse_prev_always_valid_p_extracts_per_variant() {
        let blob = json!({
            "control": {"always_valid_p": 1.0, "method": "msprt"},
            "treatment": {"always_valid_p": 0.03, "method": "msprt"},
        })
        .to_string();
        let prev = parse_prev_always_valid_p(&blob);
        assert!((prev["control"] - 1.0).abs() < 1e-12);
        assert!((prev["treatment"] - 0.03).abs() < 1e-12);
    }

    #[test]
    fn parse_prev_handles_empty_and_malformed() {
        assert!(parse_prev_always_valid_p("").is_empty());
        assert!(parse_prev_always_valid_p("   ").is_empty());
        assert!(parse_prev_always_valid_p("not json").is_empty());
        assert!(parse_prev_always_valid_p("[1,2,3]").is_empty());
    }

    // ── running-minimum monotonicity (Phase 3.2) ──────────────────────────────

    /// Across two successive computes with the SAME (or growing) data, the
    /// reported always-valid p is a monotone non-increasing running minimum:
    /// feeding tick-1's blob back in as `prev_p` for tick-2 must never raise
    /// any variant's `always_valid_p`.
    #[test]
    fn running_minimum_is_monotone_non_increasing_across_ticks() {
        let cfg = SequentialConfig {
            alpha: 0.05,
            tau_squared: 0.05,
            min_sample_size: 100,
        };

        // Tick 1: a modest signal.
        let mut t1 = HashMap::new();
        t1.insert("control".to_string(), count_stats(1000, 100));
        t1.insert("treatment".to_string(), count_stats(1000, 130));
        let blob1 = compute_sequential_blob(
            SequentialMetricFamily::Count,
            "control",
            &t1,
            &cfg,
            &HashMap::new(),
        )
        .unwrap();
        let p1 = parse_prev_always_valid_p(&blob1.to_string());

        // Tick 2: MORE data, same direction; carry tick-1's running-min p in.
        let mut t2 = HashMap::new();
        t2.insert("control".to_string(), count_stats(4000, 400));
        t2.insert("treatment".to_string(), count_stats(4000, 520));
        let blob2 =
            compute_sequential_blob(SequentialMetricFamily::Count, "control", &t2, &cfg, &p1)
                .unwrap();
        let p2 = parse_prev_always_valid_p(&blob2.to_string());

        // Monotone: tick-2's p must be ≤ tick-1's p for the treatment arm.
        assert!(
            p2["treatment"] <= p1["treatment"] + 1e-12,
            "tick2 p {} must be <= tick1 p {}",
            p2["treatment"],
            p1["treatment"]
        );

        // And running-min is enforced even if tick-2's *fresh* look were weaker:
        // re-run tick 2 with a deliberately tiny effect but tick-1's strong prior.
        let mut t3 = HashMap::new();
        t3.insert("control".to_string(), count_stats(4000, 400));
        t3.insert("treatment".to_string(), count_stats(4000, 401)); // ~no effect
        let strong_prior: HashMap<String, f64> =
            [("treatment".to_string(), 0.001)].into_iter().collect();
        let blob3 = compute_sequential_blob(
            SequentialMetricFamily::Count,
            "control",
            &t3,
            &cfg,
            &strong_prior,
        )
        .unwrap();
        let p3 = parse_prev_always_valid_p(&blob3.to_string());
        assert!(
            p3["treatment"] <= 0.001 + 1e-12,
            "running-min must not exceed the carried-in prior 0.001, got {}",
            p3["treatment"]
        );
    }

    // ── ratio path ──────────────────────────────────────────────────────────────

    #[test]
    fn ratio_blob_uses_delta_method_group_stats() {
        // Two clearly different ratios with enough spread to be significant.
        let control = RatioGroupStats {
            n: 1000,
            num_sum: 1000.0,
            den_sum: 2000.0, // R = 0.5
            num_sq_sum: 1500.0,
            den_sq_sum: 5000.0,
            num_den_sum: 2400.0,
        };
        let treatment = RatioGroupStats {
            n: 1000,
            num_sum: 1500.0,
            den_sum: 2000.0, // R = 0.75
            num_sq_sum: 2800.0,
            den_sq_sum: 5000.0,
            num_den_sum: 3300.0,
        };
        let mut groups = HashMap::new();
        groups.insert("control".to_string(), control);
        groups.insert("treatment".to_string(), treatment);
        let cfg = SequentialConfig {
            alpha: 0.05,
            tau_squared: 0.1,
            min_sample_size: 100,
        };
        let blob =
            compute_sequential_blob_ratio("control", &groups, &cfg, &HashMap::new()).unwrap();
        let obj = blob.as_object().unwrap();
        assert_eq!(obj["control"]["always_valid_p"], json!(1.0));
        assert_eq!(obj["treatment"]["method"], json!("msprt"));
        assert_eq!(obj["treatment"]["insufficient_data"], json!(false));
    }

    #[test]
    fn ratio_via_variant_stats_path_is_insufficient() {
        // The VariantStats path can't carry ratio sufficient stats → insufficient.
        let mut stats = HashMap::new();
        stats.insert("control".to_string(), count_stats(1000, 100));
        stats.insert("treatment".to_string(), count_stats(1000, 130));
        let cfg = SequentialConfig::default();
        let blob = compute_sequential_blob(
            SequentialMetricFamily::Ratio,
            "control",
            &stats,
            &cfg,
            &HashMap::new(),
        )
        .unwrap();
        assert_eq!(
            blob.as_object().unwrap()["treatment"]["insufficient_data"],
            json!(true)
        );
    }
}
