//! Stats domain types for statistical analysis of experiment results.
//!
//! This module provides all types needed to represent the results of a
//! statistical analysis run on an [`super::ExperimentIteration`].

pub mod bayesian;
pub mod bootstrap;
pub mod frequentist;
pub mod recommendation;
pub mod srm;

use std::collections::HashMap;
use std::fmt;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

// ── AnalysisType ─────────────────────────────────────────────────────────────

/// The statistical methodology used to analyse experiment results.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, sqlx::Type)]
#[serde(rename_all = "snake_case")]
#[sqlx(type_name = "text", rename_all = "snake_case")]
pub enum AnalysisType {
    /// Classical frequentist hypothesis testing (p-values, confidence intervals).
    Frequentist,
    /// Bayesian inference (posterior probabilities, credible intervals).
    Bayesian,
}

// ── MetricType ───────────────────────────────────────────────────────────────

/// The kind of metric being analysed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MetricType {
    /// A simple event count metric.
    Count,
    /// A continuous numeric metric (e.g. revenue, latency).
    Numeric,
    /// A percentile metric (e.g. p50/p95/p99 latency).
    Percentile,
    /// A funnel (multi-step conversion) metric.
    Funnel,
}

// ── Percentiles ──────────────────────────────────────────────────────────────

/// Percentile summary statistics for a variant.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct Percentiles {
    /// 50th percentile (median).
    pub p50: f64,
    /// 95th percentile.
    pub p95: f64,
    /// 99th percentile.
    pub p99: f64,
}

// ── VariantStats ─────────────────────────────────────────────────────────────

/// Per-variant raw statistics collected during the experiment.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct VariantStats {
    /// Number of unique units (users/sessions) in this variant.
    pub sample_size: i64,
    /// Number of conversion events observed (for conversion/funnel metrics).
    pub conversions: Option<i64>,
    /// Arithmetic mean of the metric values.
    pub mean: Option<f64>,
    /// Sample variance of the metric values.
    pub variance: Option<f64>,
    /// Observed conversion rate (`conversions / sample_size`).
    pub conversion_rate: Option<f64>,
    /// Percentile breakdown (populated for `Percentile` metric type).
    pub percentiles: Option<Percentiles>,
}

// ── ConfidenceInterval / CredibleInterval ────────────────────────────────────

/// A symmetric interval with lower and upper bounds.
///
/// Used both as a frequentist confidence interval and a Bayesian credible interval.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ConfidenceInterval {
    /// Lower bound of the interval.
    pub lower: f64,
    /// Upper bound of the interval.
    pub upper: f64,
}

// ── FrequentistResult ────────────────────────────────────────────────────────

/// Results from frequentist hypothesis testing.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct FrequentistResult {
    /// Two-tailed p-value for the null hypothesis of no difference.
    pub p_value: f64,
    /// Multi-comparison-corrected p-value (Bonferroni). `None` when only one
    /// comparison was run (i.e. a 2-variant experiment with K=1) or when
    /// callers have not applied a correction. When `Some`, this is the value
    /// callers should use for significance decisions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub p_value_corrected: Option<f64>,
    /// Confidence interval for the effect size.
    pub confidence_interval: ConfidenceInterval,
    /// Whether the result is statistically significant at the configured alpha level.
    pub significant: bool,
}

// ── BayesianResult ───────────────────────────────────────────────────────────

/// Results from Bayesian inference.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct BayesianResult {
    /// Posterior probability that this variant is the best.
    pub prob_best: f64,
    /// Credible interval for the effect size.
    pub credible_interval: ConfidenceInterval,
    /// Expected loss from choosing this variant over the current best.
    pub expected_loss: f64,
}

// ── Recommendation ───────────────────────────────────────────────────────────

/// The recommended action based on the statistical analysis results.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Recommendation {
    /// A specific variant has won and should be promoted.
    VariantWins(String),
    /// The control group has won; roll back / keep the default.
    ControlWins,
    /// The result is inconclusive (insufficient signal, not significant).
    Inconclusive,
    /// More data is needed before a conclusion can be drawn.
    NeedsMoreData,
}

impl Recommendation {
    /// Returns `true` if this recommendation is actionable (i.e. a winner is clear).
    pub fn is_actionable(&self) -> bool {
        matches!(
            self,
            Recommendation::VariantWins(_) | Recommendation::ControlWins
        )
    }

    /// Numeric priority for ordering recommendations from most to least decisive.
    ///
    /// Lower number = more decisive:
    /// 1. `VariantWins` — clear winner
    /// 2. `ControlWins` — control wins
    /// 3. `Inconclusive` — no signal
    /// 4. `NeedsMoreData` — still collecting
    fn decisiveness_rank(&self) -> u8 {
        match self {
            Recommendation::VariantWins(_) => 1,
            Recommendation::ControlWins => 2,
            Recommendation::Inconclusive => 3,
            Recommendation::NeedsMoreData => 4,
        }
    }
}

impl PartialOrd for Recommendation {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Recommendation {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.decisiveness_rank().cmp(&other.decisiveness_rank())
    }
}

impl fmt::Display for Recommendation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Recommendation::VariantWins(variant) => write!(f, "variant_wins:{variant}"),
            Recommendation::ControlWins => write!(f, "control_wins"),
            Recommendation::Inconclusive => write!(f, "inconclusive"),
            Recommendation::NeedsMoreData => write!(f, "needs_more_data"),
        }
    }
}

// ── MetricResult ─────────────────────────────────────────────────────────────

/// The full analysis result for a single metric within an iteration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct MetricResult {
    /// The metric definition key this result is for.
    pub metric_key: String,
    /// The type of metric analysed.
    pub metric_type: MetricType,
    /// Per-variant raw statistics, keyed by variant key.
    pub variant_stats: HashMap<String, VariantStats>,
    /// Frequentist analysis result (if frequentist analysis was run).
    pub frequentist_result: Option<FrequentistResult>,
    /// Bayesian analysis result (if Bayesian analysis was run).
    pub bayesian_result: Option<BayesianResult>,
    /// The recommended action based on the analysis.
    pub recommendation: Recommendation,
}

// ── IterationResults ─────────────────────────────────────────────────────────

/// The complete statistical analysis results for one experiment iteration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct IterationResults {
    /// The experiment these results belong to.
    pub experiment_id: Uuid,
    /// The specific iteration these results belong to.
    pub iteration_id: Uuid,
    /// The 1-based iteration number within the experiment.
    pub iteration_number: i32,
    /// When this analysis was computed.
    pub computed_at: DateTime<Utc>,
    /// Per-metric analysis results.
    pub metrics: Vec<MetricResult>,
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── AnalysisType ─────────────────────────────────────────────────────────

    #[test]
    fn analysis_type_serializes_to_snake_case() {
        let freq = serde_json::to_string(&AnalysisType::Frequentist).unwrap();
        assert_eq!(freq, "\"frequentist\"");

        let bayes = serde_json::to_string(&AnalysisType::Bayesian).unwrap();
        assert_eq!(bayes, "\"bayesian\"");
    }

    #[test]
    fn analysis_type_deserializes_from_snake_case() {
        let freq: AnalysisType = serde_json::from_str("\"frequentist\"").unwrap();
        assert_eq!(freq, AnalysisType::Frequentist);

        let bayes: AnalysisType = serde_json::from_str("\"bayesian\"").unwrap();
        assert_eq!(bayes, AnalysisType::Bayesian);
    }

    #[test]
    fn analysis_type_unknown_string_deserializes_to_err() {
        let result: Result<AnalysisType, _> = serde_json::from_str("\"unknown\"");
        assert!(result.is_err());
    }

    #[test]
    fn analysis_type_is_clone_and_copy() {
        let a = AnalysisType::Frequentist;
        let b = a; // Copy
        let c = a;
        assert_eq!(a, b);
        assert_eq!(a, c);
    }

    // ── MetricType ────────────────────────────────────────────────────────────

    #[test]
    fn metric_type_all_variants_serialize() {
        assert_eq!(
            serde_json::to_string(&MetricType::Count).unwrap(),
            "\"count\""
        );
        assert_eq!(
            serde_json::to_string(&MetricType::Numeric).unwrap(),
            "\"numeric\""
        );
        assert_eq!(
            serde_json::to_string(&MetricType::Percentile).unwrap(),
            "\"percentile\""
        );
        assert_eq!(
            serde_json::to_string(&MetricType::Funnel).unwrap(),
            "\"funnel\""
        );
    }

    #[test]
    fn metric_type_all_variants_deserialize() {
        assert_eq!(
            serde_json::from_str::<MetricType>("\"count\"").unwrap(),
            MetricType::Count
        );
        assert_eq!(
            serde_json::from_str::<MetricType>("\"numeric\"").unwrap(),
            MetricType::Numeric
        );
        assert_eq!(
            serde_json::from_str::<MetricType>("\"percentile\"").unwrap(),
            MetricType::Percentile
        );
        assert_eq!(
            serde_json::from_str::<MetricType>("\"funnel\"").unwrap(),
            MetricType::Funnel
        );
    }

    // ── Recommendation ordering ───────────────────────────────────────────────

    #[test]
    fn recommendation_variant_wins_is_most_decisive() {
        let vw = Recommendation::VariantWins("variant_b".to_string());
        let cw = Recommendation::ControlWins;
        let inc = Recommendation::Inconclusive;
        let nmd = Recommendation::NeedsMoreData;

        assert!(vw < cw);
        assert!(vw < inc);
        assert!(vw < nmd);
    }

    #[test]
    fn recommendation_control_wins_beats_inconclusive() {
        assert!(Recommendation::ControlWins < Recommendation::Inconclusive);
    }

    #[test]
    fn recommendation_inconclusive_beats_needs_more_data() {
        assert!(Recommendation::Inconclusive < Recommendation::NeedsMoreData);
    }

    #[test]
    fn recommendation_needs_more_data_is_least_decisive() {
        assert!(Recommendation::NeedsMoreData > Recommendation::VariantWins("x".to_string()));
        assert!(Recommendation::NeedsMoreData > Recommendation::ControlWins);
        assert!(Recommendation::NeedsMoreData > Recommendation::Inconclusive);
    }

    #[test]
    fn recommendation_is_actionable() {
        assert!(Recommendation::VariantWins("v".to_string()).is_actionable());
        assert!(Recommendation::ControlWins.is_actionable());
        assert!(!Recommendation::Inconclusive.is_actionable());
        assert!(!Recommendation::NeedsMoreData.is_actionable());
    }

    #[test]
    fn recommendation_display() {
        assert_eq!(
            Recommendation::VariantWins("variant_b".to_string()).to_string(),
            "variant_wins:variant_b"
        );
        assert_eq!(Recommendation::ControlWins.to_string(), "control_wins");
        assert_eq!(Recommendation::Inconclusive.to_string(), "inconclusive");
        assert_eq!(Recommendation::NeedsMoreData.to_string(), "needs_more_data");
    }

    #[test]
    fn recommendation_serializes_to_snake_case() {
        let json = serde_json::to_string(&Recommendation::ControlWins).unwrap();
        assert_eq!(json, "\"control_wins\"");

        let json = serde_json::to_string(&Recommendation::Inconclusive).unwrap();
        assert_eq!(json, "\"inconclusive\"");

        let json = serde_json::to_string(&Recommendation::NeedsMoreData).unwrap();
        assert_eq!(json, "\"needs_more_data\"");
    }

    #[test]
    fn recommendation_variant_wins_serializes_with_payload() {
        let rec = Recommendation::VariantWins("variant_b".to_string());
        let json = serde_json::to_value(&rec).unwrap();
        // serde default for newtype variant: {"variant_wins": "variant_b"}
        assert!(json.get("variant_wins").is_some());
        assert_eq!(json["variant_wins"], "variant_b");
    }

    // ── Percentiles ──────────────────────────────────────────────────────────

    #[test]
    fn percentiles_struct_can_be_constructed_and_cloned() {
        let p = Percentiles {
            p50: 100.0,
            p95: 250.0,
            p99: 400.0,
        };
        let p2 = p.clone();
        assert_eq!(p, p2);
    }

    #[test]
    fn percentiles_serializes_to_snake_case_keys() {
        let p = Percentiles {
            p50: 1.0,
            p95: 2.0,
            p99: 3.0,
        };
        let v = serde_json::to_value(&p).unwrap();
        assert!(v.get("p50").is_some());
        assert!(v.get("p95").is_some());
        assert!(v.get("p99").is_some());
    }

    // ── VariantStats ─────────────────────────────────────────────────────────

    #[test]
    fn variant_stats_all_none_options() {
        let vs = VariantStats {
            sample_size: 1000,
            conversions: None,
            mean: None,
            variance: None,
            conversion_rate: None,
            percentiles: None,
        };
        assert_eq!(vs.sample_size, 1000);
        assert!(vs.conversions.is_none());
        assert!(vs.mean.is_none());
    }

    #[test]
    fn variant_stats_with_all_fields() {
        let vs = VariantStats {
            sample_size: 500,
            conversions: Some(50),
            mean: Some(0.1),
            variance: Some(0.09),
            conversion_rate: Some(0.1),
            percentiles: Some(Percentiles {
                p50: 100.0,
                p95: 250.0,
                p99: 400.0,
            }),
        };
        assert_eq!(vs.sample_size, 500);
        assert_eq!(vs.conversions, Some(50));
        assert_eq!(vs.conversion_rate, Some(0.1));
        assert!(vs.percentiles.is_some());
    }

    // ── ConfidenceInterval ────────────────────────────────────────────────────

    #[test]
    fn confidence_interval_can_be_constructed_and_cloned() {
        let ci = ConfidenceInterval {
            lower: -0.05,
            upper: 0.15,
        };
        let ci2 = ci.clone();
        assert_eq!(ci, ci2);
    }

    // ── FrequentistResult ─────────────────────────────────────────────────────

    #[test]
    fn frequentist_result_significant_when_p_value_low() {
        let result = FrequentistResult {
            p_value: 0.01,
            p_value_corrected: None,
            confidence_interval: ConfidenceInterval {
                lower: 0.02,
                upper: 0.18,
            },
            significant: true,
        };
        assert!(result.significant);
        assert!(result.p_value < 0.05);
    }

    #[test]
    fn frequentist_result_not_significant_when_p_value_high() {
        let result = FrequentistResult {
            p_value: 0.42,
            p_value_corrected: None,
            confidence_interval: ConfidenceInterval {
                lower: -0.05,
                upper: 0.15,
            },
            significant: false,
        };
        assert!(!result.significant);
    }

    #[test]
    fn frequentist_result_with_bonferroni_corrected_p_value() {
        let result = FrequentistResult {
            p_value: 0.02,
            p_value_corrected: Some(0.06), // Bonferroni K=3
            confidence_interval: ConfidenceInterval {
                lower: 0.01,
                upper: 0.15,
            },
            significant: false,
        };
        assert!(result.p_value_corrected.is_some());
        // After correction, the variant is no longer significant.
        assert!(!result.significant);
    }

    #[test]
    fn frequentist_result_corrected_field_skipped_when_none() {
        let result = FrequentistResult {
            p_value: 0.01,
            p_value_corrected: None,
            confidence_interval: ConfidenceInterval {
                lower: 0.02,
                upper: 0.18,
            },
            significant: true,
        };
        let json = serde_json::to_value(&result).unwrap();
        assert!(
            json.get("p_value_corrected").is_none(),
            "None should be skipped, got {}",
            json
        );
    }

    // ── BayesianResult ────────────────────────────────────────────────────────

    #[test]
    fn bayesian_result_can_be_constructed_and_cloned() {
        let result = BayesianResult {
            prob_best: 0.95,
            credible_interval: ConfidenceInterval {
                lower: 0.02,
                upper: 0.18,
            },
            expected_loss: 0.001,
        };
        let result2 = result.clone();
        assert_eq!(result.prob_best, result2.prob_best);
        assert_eq!(result.expected_loss, result2.expected_loss);
    }

    // ── MetricResult ──────────────────────────────────────────────────────────

    #[test]
    fn metric_result_with_variant_stats_hashmap() {
        let mut variant_stats = HashMap::new();
        variant_stats.insert(
            "control".to_string(),
            VariantStats {
                sample_size: 1000,
                conversions: Some(100),
                mean: Some(0.1),
                variance: Some(0.09),
                conversion_rate: Some(0.1),
                percentiles: None,
            },
        );
        variant_stats.insert(
            "variant_b".to_string(),
            VariantStats {
                sample_size: 1000,
                conversions: Some(120),
                mean: Some(0.12),
                variance: Some(0.10),
                conversion_rate: Some(0.12),
                percentiles: None,
            },
        );

        let mr = MetricResult {
            metric_key: "checkout_completed".to_string(),
            metric_type: MetricType::Count,
            variant_stats,
            frequentist_result: Some(FrequentistResult {
                p_value: 0.03,
                p_value_corrected: None,
                confidence_interval: ConfidenceInterval {
                    lower: 0.005,
                    upper: 0.035,
                },
                significant: true,
            }),
            bayesian_result: None,
            recommendation: Recommendation::VariantWins("variant_b".to_string()),
        };

        assert_eq!(mr.metric_key, "checkout_completed");
        assert_eq!(mr.variant_stats.len(), 2);
        assert!(mr.variant_stats.contains_key("control"));
        assert!(mr.variant_stats.contains_key("variant_b"));
        assert!(mr.frequentist_result.is_some());
        assert!(mr.bayesian_result.is_none());
    }

    // ── IterationResults ──────────────────────────────────────────────────────

    #[test]
    fn iteration_results_can_be_constructed_and_cloned() {
        let experiment_id = Uuid::new_v4();
        let iteration_id = Uuid::new_v4();

        let ir = IterationResults {
            experiment_id,
            iteration_id,
            iteration_number: 1,
            computed_at: Utc::now(),
            metrics: vec![],
        };

        let ir2 = ir.clone();
        assert_eq!(ir.experiment_id, ir2.experiment_id);
        assert_eq!(ir.iteration_number, ir2.iteration_number);
        assert_eq!(ir.metrics.len(), 0);
    }

    #[test]
    fn iteration_results_serializes_and_deserializes() {
        let ir = IterationResults {
            experiment_id: Uuid::new_v4(),
            iteration_id: Uuid::new_v4(),
            iteration_number: 2,
            computed_at: Utc::now(),
            metrics: vec![],
        };

        let json = serde_json::to_string(&ir).unwrap();
        let ir2: IterationResults = serde_json::from_str(&json).unwrap();
        assert_eq!(ir.experiment_id, ir2.experiment_id);
        assert_eq!(ir.iteration_number, ir2.iteration_number);
    }

    #[test]
    fn iteration_results_serialized_keys_are_snake_case() {
        let ir = IterationResults {
            experiment_id: Uuid::new_v4(),
            iteration_id: Uuid::new_v4(),
            iteration_number: 1,
            computed_at: Utc::now(),
            metrics: vec![],
        };
        let v = serde_json::to_value(&ir).unwrap();
        assert!(v.get("experiment_id").is_some());
        assert!(v.get("iteration_id").is_some());
        assert!(v.get("iteration_number").is_some());
        assert!(v.get("computed_at").is_some());
        assert!(v.get("metrics").is_some());
    }
}
