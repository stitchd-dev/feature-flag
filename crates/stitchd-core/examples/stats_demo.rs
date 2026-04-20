/// Quick demo of the statistical analysis engine with sample data.
///
/// Run with:
///   cargo run -p stitchd-core --example stats_demo
use stitchd_core::experimentation::stats::{
    AnalysisType, VariantStats, bayesian, frequentist,
    recommendation::{RecommendationInput, pick_winner, recommend},
};

fn hr(label: &str) {
    println!("\n{}", "─".repeat(60));
    println!("  {label}");
    println!("{}", "─".repeat(60));
}

fn main() {
    // ── Scenario 1: Count metric — clear winner ───────────────────────────────
    hr("Scenario 1 · Count metric — CLEAR WINNER (variant converts 15% vs 10%)");

    let control = VariantStats {
        sample_size: 1_000,
        conversions: Some(100), // 10% conversion
        mean: None,
        variance: None,
        conversion_rate: Some(0.10),
        percentiles: None,
    };
    let variant_a = VariantStats {
        sample_size: 1_000,
        conversions: Some(150), // 15% conversion
        mean: None,
        variance: None,
        conversion_rate: Some(0.15),
        percentiles: None,
    };

    let freq = frequentist::analyze_count(&control, &variant_a);
    println!("[Frequentist]");
    println!("  p_value            = {:.6}", freq.p_value);
    println!(
        "  CI (lift)          = [{:.4}, {:.4}]",
        freq.confidence_interval.lower, freq.confidence_interval.upper
    );
    println!("  significant        = {}", freq.significant);

    let bayes = bayesian::analyze_count(&control, &variant_a);
    println!("[Bayesian]");
    println!("  prob_best (variant)= {:.4}", bayes.prob_best);
    println!(
        "  credible interval  = [{:.4}, {:.4}]",
        bayes.credible_interval.lower, bayes.credible_interval.upper
    );
    println!("  expected_loss      = {:.6}", bayes.expected_loss);

    let rec_freq = recommend(&RecommendationInput {
        variant_key: "variant_a".into(),
        is_control: false,
        frequentist: Some(freq.clone()),
        bayesian: None,
        analysis_type: AnalysisType::Frequentist,
        sample_size: 1_000,
        min_sample_size: Some(200),
    });
    let rec_bayes = recommend(&RecommendationInput {
        variant_key: "variant_a".into(),
        is_control: false,
        frequentist: None,
        bayesian: Some(bayes.clone()),
        analysis_type: AnalysisType::Bayesian,
        sample_size: 1_000,
        min_sample_size: Some(200),
    });
    println!("[Recommendations]");
    println!("  Frequentist: {:?}", rec_freq);
    println!("  Bayesian:    {:?}", rec_bayes);

    // ── Scenario 2: Count metric — inconclusive ───────────────────────────────
    hr("Scenario 2 · Count metric — INCONCLUSIVE (10.5% vs 10.0%, n=200 each)");

    let ctrl2 = VariantStats {
        sample_size: 200,
        conversions: Some(20), // 10.0%
        mean: None,
        variance: None,
        conversion_rate: Some(0.10),
        percentiles: None,
    };
    let var2 = VariantStats {
        sample_size: 200,
        conversions: Some(21), // 10.5%
        mean: None,
        variance: None,
        conversion_rate: Some(0.105),
        percentiles: None,
    };

    let freq2 = frequentist::analyze_count(&ctrl2, &var2);
    let bayes2 = bayesian::analyze_count(&ctrl2, &var2);
    println!(
        "[Frequentist]  p={:.4}  significant={}",
        freq2.p_value, freq2.significant
    );
    println!("[Bayesian]     prob_best={:.4}", bayes2.prob_best);
    println!("[Recommendations]");
    println!(
        "  Frequentist: {:?}",
        recommend(&RecommendationInput {
            variant_key: "variant_b".into(),
            is_control: false,
            frequentist: Some(freq2),
            bayesian: None,
            analysis_type: AnalysisType::Frequentist,
            sample_size: 200,
            min_sample_size: None,
        })
    );
    println!(
        "  Bayesian:    {:?}",
        recommend(&RecommendationInput {
            variant_key: "variant_b".into(),
            is_control: false,
            frequentist: None,
            bayesian: Some(bayes2),
            analysis_type: AnalysisType::Bayesian,
            sample_size: 200,
            min_sample_size: None,
        })
    );

    // ── Scenario 3: Needs more data ───────────────────────────────────────────
    hr("Scenario 3 · Count metric — NEEDS MORE DATA (n=50, min=200)");

    let ctrl3 = VariantStats {
        sample_size: 50,
        conversions: Some(5),
        mean: None,
        variance: None,
        conversion_rate: Some(0.10),
        percentiles: None,
    };
    let var3 = VariantStats {
        sample_size: 50,
        conversions: Some(12),
        mean: None,
        variance: None,
        conversion_rate: Some(0.24),
        percentiles: None,
    };
    let freq3 = frequentist::analyze_count(&ctrl3, &var3);
    println!(
        "[Frequentist]  p={:.4}  significant={}",
        freq3.p_value, freq3.significant
    );
    println!("[Recommendation with min_sample_size=200]");
    println!(
        "  {:?}",
        recommend(&RecommendationInput {
            variant_key: "variant_c".into(),
            is_control: false,
            frequentist: Some(freq3),
            bayesian: None,
            analysis_type: AnalysisType::Frequentist,
            sample_size: 50,
            min_sample_size: Some(200),
        })
    );

    // ── Scenario 4: Numeric metric (revenue per user) ─────────────────────────
    hr("Scenario 4 · Numeric metric — revenue/user ($12.50 vs $10.00, n=500)");

    let ctrl4 = VariantStats {
        sample_size: 500,
        conversions: None,
        mean: Some(10.00),
        variance: Some(25.0),
        conversion_rate: None,
        percentiles: None,
    };
    let var4 = VariantStats {
        sample_size: 500,
        conversions: None,
        mean: Some(12.50),
        variance: Some(28.0),
        conversion_rate: None,
        percentiles: None,
    };
    let freq4 = frequentist::analyze_numeric(&ctrl4, &var4);
    let bayes4 = bayesian::analyze_numeric(&ctrl4, &var4);
    println!(
        "[Frequentist]  p={:.6}  CI=[{:.2}, {:.2}]  significant={}",
        freq4.p_value,
        freq4.confidence_interval.lower,
        freq4.confidence_interval.upper,
        freq4.significant
    );
    println!(
        "[Bayesian]     prob_best={:.4}  CI=[{:.2}, {:.2}]",
        bayes4.prob_best, bayes4.credible_interval.lower, bayes4.credible_interval.upper
    );

    // ── Scenario 5: Percentile metric (p95 latency, ms) ──────────────────────
    hr("Scenario 5 · Percentile metric — p95 latency (variant faster)");

    // Control: roughly normal around 200ms
    let control_latency: Vec<f64> = (0..500)
        .map(|i| 200.0 + ((i as f64 * 1.618) % 100.0) - 50.0)
        .collect();
    // Variant: roughly normal around 160ms (faster)
    let variant_latency: Vec<f64> = (0..500)
        .map(|i| 160.0 + ((i as f64 * 1.618) % 80.0) - 40.0)
        .collect();

    let freq5 = frequentist::analyze_percentile(&control_latency, &variant_latency, 95.0);
    println!("[Frequentist bootstrap p95]");
    println!(
        "  CI of (variant_p95 - control_p95) = [{:.1}ms, {:.1}ms]",
        freq5.confidence_interval.lower, freq5.confidence_interval.upper
    );
    println!("  (negative = variant is faster)");

    // ── Scenario 6: pick_winner multi-variant ─────────────────────────────────
    hr("Scenario 6 · Multi-variant — pick_winner");

    let recs = vec![
        (
            "control".to_string(),
            stitchd_core::experimentation::stats::Recommendation::ControlWins,
        ),
        (
            "variant_a".to_string(),
            stitchd_core::experimentation::stats::Recommendation::Inconclusive,
        ),
        (
            "variant_b".to_string(),
            stitchd_core::experimentation::stats::Recommendation::VariantWins("variant_b".into()),
        ),
    ];
    println!("  Input: control=ControlWins, variant_a=Inconclusive, variant_b=VariantWins");
    println!("  pick_winner → {:?}", pick_winner(&recs));

    println!("\n{}", "─".repeat(60));
    println!("  All scenarios complete.");
    println!("{}", "─".repeat(60));
}
