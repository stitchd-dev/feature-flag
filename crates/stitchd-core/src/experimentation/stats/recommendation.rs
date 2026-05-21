//! Recommendation engine — translate statistical results into actionable recommendations.
//!
//! Provides [`recommend`] to evaluate a single variant-vs-control comparison and
//! [`pick_winner`] to determine the overall winner across multiple variants.

use super::{AnalysisType, BayesianResult, FrequentistResult, Recommendation};

// ── RecommendationInput ───────────────────────────────────────────────────────

/// Input for a single variant-vs-control recommendation computation.
pub struct RecommendationInput {
    /// The key identifying this variant.
    pub variant_key: String,
    /// Whether this variant is the control group (used as a label hint).
    pub is_control: bool,
    /// Frequentist analysis result, if available.
    ///
    /// The `confidence_interval` encodes the difference `variant − control`:
    /// positive lower bound means variant is higher; negative upper bound means
    /// control is higher.
    pub frequentist: Option<FrequentistResult>,
    /// Bayesian analysis result, if available.
    pub bayesian: Option<BayesianResult>,
    /// Which analysis methodology was used.
    pub analysis_type: AnalysisType,
    /// Observed sample size for this variant.
    pub sample_size: i64,
    /// Minimum sample size required before drawing any conclusion.
    pub min_sample_size: Option<i64>,
}

// ── recommend ─────────────────────────────────────────────────────────────────

/// Determine the recommendation for a single variant vs control comparison.
///
/// # Decision order
/// 1. **NeedsMoreData guard** — checked before any statistical test.
/// 2. **Frequentist** rule when `analysis_type == Frequentist`.
/// 3. **Bayesian** rule when `analysis_type == Bayesian`.
pub fn recommend(input: &RecommendationInput) -> Recommendation {
    // 1. NeedsMoreData guard (must be checked FIRST)
    if let Some(min) = input.min_sample_size
        && input.sample_size < min
    {
        return Recommendation::NeedsMoreData;
    }

    match input.analysis_type {
        AnalysisType::Frequentist => {
            if let Some(freq) = &input.frequentist
                && freq.significant
            {
                // CI encodes (variant - control). Lower > 0 ⟹ variant wins.
                if freq.confidence_interval.lower > 0.0 {
                    return Recommendation::VariantWins(input.variant_key.clone());
                }
                return Recommendation::ControlWins;
            }
            Recommendation::Inconclusive
        }

        AnalysisType::Bayesian => {
            if let Some(bayes) = &input.bayesian {
                if bayes.prob_best > 0.95 {
                    return Recommendation::VariantWins(input.variant_key.clone());
                }
                if (1.0 - bayes.prob_best) > 0.95 {
                    return Recommendation::ControlWins;
                }
            }
            Recommendation::Inconclusive
        }
    }
}

// ── pick_winner ───────────────────────────────────────────────────────────────

/// Given recommendations for all variants, pick the overall winner.
///
/// # Selection rules (in order)
/// 1. Exactly one `VariantWins` → return that `VariantWins`.
/// 2. `ControlWins` is present → `ControlWins`.
/// 3. Any `NeedsMoreData` present → `NeedsMoreData`.
/// 4. Otherwise → `Inconclusive`.
pub fn pick_winner(recommendations: &[(String, Recommendation)]) -> Recommendation {
    let variant_wins: Vec<&String> = recommendations
        .iter()
        .filter_map(|(key, rec)| {
            if let Recommendation::VariantWins(_) = rec {
                Some(key)
            } else {
                None
            }
        })
        .collect();

    if variant_wins.len() == 1 {
        return Recommendation::VariantWins(variant_wins[0].clone());
    }

    let has_control_wins = recommendations
        .iter()
        .any(|(_, rec)| matches!(rec, Recommendation::ControlWins));

    if has_control_wins {
        return Recommendation::ControlWins;
    }

    let has_needs_more_data = recommendations
        .iter()
        .any(|(_, rec)| matches!(rec, Recommendation::NeedsMoreData));

    if has_needs_more_data {
        return Recommendation::NeedsMoreData;
    }

    Recommendation::Inconclusive
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::experimentation::stats::{BayesianResult, ConfidenceInterval, FrequentistResult};

    fn make_frequentist(significant: bool, ci_lower: f64, ci_upper: f64) -> FrequentistResult {
        FrequentistResult {
            p_value: if significant { 0.03 } else { 0.42 },
            p_value_corrected: None,
            confidence_interval: ConfidenceInterval {
                lower: ci_lower,
                upper: ci_upper,
            },
            significant,
        }
    }

    fn make_bayesian(prob_best: f64) -> BayesianResult {
        BayesianResult {
            prob_best,
            credible_interval: ConfidenceInterval {
                lower: 0.01,
                upper: 0.15,
            },
            expected_loss: 0.001,
        }
    }

    // ── NeedsMoreData guard ───────────────────────────────────────────────────

    #[test]
    fn needs_more_data_when_sample_size_below_minimum() {
        let input = RecommendationInput {
            variant_key: "variant_b".to_string(),
            is_control: false,
            frequentist: Some(make_frequentist(true, 0.05, 0.20)),
            bayesian: None,
            analysis_type: AnalysisType::Frequentist,
            sample_size: 50,
            min_sample_size: Some(100),
        };
        assert_eq!(recommend(&input), Recommendation::NeedsMoreData);
    }

    #[test]
    fn needs_more_data_guard_precedes_statistical_tests() {
        // Even with a highly significant frequentist result, NeedsMoreData wins.
        let input = RecommendationInput {
            variant_key: "variant_b".to_string(),
            is_control: false,
            frequentist: Some(make_frequentist(true, 0.10, 0.30)),
            bayesian: None,
            analysis_type: AnalysisType::Frequentist,
            sample_size: 10,
            min_sample_size: Some(1000),
        };
        assert_eq!(recommend(&input), Recommendation::NeedsMoreData);
    }

    #[test]
    fn no_needs_more_data_when_sample_meets_minimum() {
        let input = RecommendationInput {
            variant_key: "variant_b".to_string(),
            is_control: false,
            frequentist: Some(make_frequentist(true, 0.05, 0.20)),
            bayesian: None,
            analysis_type: AnalysisType::Frequentist,
            sample_size: 100,
            min_sample_size: Some(100),
        };
        // Should NOT be NeedsMoreData (exactly at minimum is sufficient)
        assert_ne!(recommend(&input), Recommendation::NeedsMoreData);
    }

    // ── Frequentist rules ─────────────────────────────────────────────────────

    #[test]
    fn frequentist_significant_variant_higher_yields_variant_wins() {
        let input = RecommendationInput {
            variant_key: "variant_b".to_string(),
            is_control: false,
            frequentist: Some(make_frequentist(true, 0.02, 0.18)),
            bayesian: None,
            analysis_type: AnalysisType::Frequentist,
            sample_size: 500,
            min_sample_size: None,
        };
        assert_eq!(
            recommend(&input),
            Recommendation::VariantWins("variant_b".to_string())
        );
    }

    #[test]
    fn frequentist_significant_control_higher_yields_control_wins() {
        let input = RecommendationInput {
            variant_key: "variant_b".to_string(),
            is_control: false,
            // Negative CI ⟹ control is higher
            frequentist: Some(make_frequentist(true, -0.20, -0.02)),
            bayesian: None,
            analysis_type: AnalysisType::Frequentist,
            sample_size: 500,
            min_sample_size: None,
        };
        assert_eq!(recommend(&input), Recommendation::ControlWins);
    }

    #[test]
    fn frequentist_not_significant_yields_inconclusive() {
        let input = RecommendationInput {
            variant_key: "variant_b".to_string(),
            is_control: false,
            frequentist: Some(make_frequentist(false, -0.05, 0.15)),
            bayesian: None,
            analysis_type: AnalysisType::Frequentist,
            sample_size: 500,
            min_sample_size: None,
        };
        assert_eq!(recommend(&input), Recommendation::Inconclusive);
    }

    #[test]
    fn frequentist_missing_result_yields_inconclusive() {
        let input = RecommendationInput {
            variant_key: "variant_b".to_string(),
            is_control: false,
            frequentist: None,
            bayesian: None,
            analysis_type: AnalysisType::Frequentist,
            sample_size: 500,
            min_sample_size: None,
        };
        assert_eq!(recommend(&input), Recommendation::Inconclusive);
    }

    // ── Bayesian rules ────────────────────────────────────────────────────────

    #[test]
    fn bayesian_prob_best_above_threshold_yields_variant_wins() {
        let input = RecommendationInput {
            variant_key: "variant_b".to_string(),
            is_control: false,
            frequentist: None,
            bayesian: Some(make_bayesian(0.97)),
            analysis_type: AnalysisType::Bayesian,
            sample_size: 500,
            min_sample_size: None,
        };
        assert_eq!(
            recommend(&input),
            Recommendation::VariantWins("variant_b".to_string())
        );
    }

    #[test]
    fn bayesian_prob_best_at_boundary_yields_variant_wins() {
        // prob_best = 0.951 — just above threshold
        let input = RecommendationInput {
            variant_key: "variant_c".to_string(),
            is_control: false,
            frequentist: None,
            bayesian: Some(make_bayesian(0.951)),
            analysis_type: AnalysisType::Bayesian,
            sample_size: 500,
            min_sample_size: None,
        };
        assert_eq!(
            recommend(&input),
            Recommendation::VariantWins("variant_c".to_string())
        );
    }

    #[test]
    fn bayesian_prob_best_at_exactly_0_95_yields_inconclusive() {
        // Threshold is strictly > 0.95
        let input = RecommendationInput {
            variant_key: "variant_b".to_string(),
            is_control: false,
            frequentist: None,
            bayesian: Some(make_bayesian(0.95)),
            analysis_type: AnalysisType::Bayesian,
            sample_size: 500,
            min_sample_size: None,
        };
        assert_eq!(recommend(&input), Recommendation::Inconclusive);
    }

    #[test]
    fn bayesian_prob_best_low_yields_inconclusive() {
        let input = RecommendationInput {
            variant_key: "variant_b".to_string(),
            is_control: false,
            frequentist: None,
            bayesian: Some(make_bayesian(0.60)),
            analysis_type: AnalysisType::Bayesian,
            sample_size: 500,
            min_sample_size: None,
        };
        assert_eq!(recommend(&input), Recommendation::Inconclusive);
    }

    #[test]
    fn bayesian_control_wins_when_prob_best_very_low() {
        // 1.0 - 0.02 = 0.98 > 0.95 ⟹ control wins
        let input = RecommendationInput {
            variant_key: "variant_b".to_string(),
            is_control: false,
            frequentist: None,
            bayesian: Some(make_bayesian(0.02)),
            analysis_type: AnalysisType::Bayesian,
            sample_size: 500,
            min_sample_size: None,
        };
        assert_eq!(recommend(&input), Recommendation::ControlWins);
    }

    #[test]
    fn bayesian_missing_result_yields_inconclusive() {
        let input = RecommendationInput {
            variant_key: "variant_b".to_string(),
            is_control: false,
            frequentist: None,
            bayesian: None,
            analysis_type: AnalysisType::Bayesian,
            sample_size: 500,
            min_sample_size: None,
        };
        assert_eq!(recommend(&input), Recommendation::Inconclusive);
    }

    // ── pick_winner ───────────────────────────────────────────────────────────

    #[test]
    fn pick_winner_single_variant_wins() {
        let recommendations = vec![
            (
                "variant_b".to_string(),
                Recommendation::VariantWins("variant_b".to_string()),
            ),
            ("variant_c".to_string(), Recommendation::Inconclusive),
        ];
        assert_eq!(
            pick_winner(&recommendations),
            Recommendation::VariantWins("variant_b".to_string())
        );
    }

    #[test]
    fn pick_winner_two_variant_wins_is_not_single_winner() {
        // Two winners — not exactly one, so falls through to next rules
        let recommendations = vec![
            (
                "variant_b".to_string(),
                Recommendation::VariantWins("variant_b".to_string()),
            ),
            (
                "variant_c".to_string(),
                Recommendation::VariantWins("variant_c".to_string()),
            ),
        ];
        // No single winner, no ControlWins, no NeedsMoreData → Inconclusive
        assert_eq!(pick_winner(&recommendations), Recommendation::Inconclusive);
    }

    #[test]
    fn pick_winner_control_wins_when_present() {
        let recommendations = vec![
            ("variant_b".to_string(), Recommendation::Inconclusive),
            ("variant_c".to_string(), Recommendation::ControlWins),
        ];
        assert_eq!(pick_winner(&recommendations), Recommendation::ControlWins);
    }

    #[test]
    fn pick_winner_needs_more_data_when_no_winner_no_control() {
        let recommendations = vec![
            ("variant_b".to_string(), Recommendation::Inconclusive),
            ("variant_c".to_string(), Recommendation::NeedsMoreData),
        ];
        assert_eq!(pick_winner(&recommendations), Recommendation::NeedsMoreData);
    }

    #[test]
    fn pick_winner_inconclusive_when_all_inconclusive() {
        let recommendations = vec![
            ("variant_b".to_string(), Recommendation::Inconclusive),
            ("variant_c".to_string(), Recommendation::Inconclusive),
        ];
        assert_eq!(pick_winner(&recommendations), Recommendation::Inconclusive);
    }

    #[test]
    fn pick_winner_empty_slice_is_inconclusive() {
        assert_eq!(pick_winner(&[]), Recommendation::Inconclusive);
    }

    #[test]
    fn pick_winner_single_variant_wins_over_multiple_variants() {
        let recommendations = vec![
            (
                "variant_b".to_string(),
                Recommendation::VariantWins("variant_b".to_string()),
            ),
            ("variant_c".to_string(), Recommendation::NeedsMoreData),
            ("variant_d".to_string(), Recommendation::Inconclusive),
        ];
        // Exactly one VariantWins → that wins, even though NeedsMoreData is present
        assert_eq!(
            pick_winner(&recommendations),
            Recommendation::VariantWins("variant_b".to_string())
        );
    }
}
