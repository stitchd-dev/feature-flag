//! Full lifecycle integration tests for the experiment results analysis pipeline.
//!
//! These tests exercise the complete analysis flow with controlled data sets,
//! verifying correctness end-to-end without requiring a live ClickHouse instance.
//! They call the stats functions directly with pre-built `VariantStats` and
//! upsert results via `PgExperimentResultsRepository`.

use std::sync::Arc;

use chrono::{Duration, Utc};
use stitchd_core::{
    experimentation::stats::{
        AnalysisType, BayesianResult, FrequentistResult, VariantStats, bayesian, frequentist,
        recommendation, recommendation::RecommendationInput,
    },
    experimentation::{Experiment, ExperimentStatus},
    flag::{FlagRecord, FlagRule, FlagValueType},
    id::{
        EnvironmentId, ExperimentId, FlagId, FlagKey, OrganisationId, ProjectId, RuleId, VariantId,
    },
    rule_engine::types::{ConditionExpr, Rule, RuleOutput},
    tenant::{Environment, Organisation, Project},
};
use stitchd_db::{
    EnvironmentRepository, ExperimentRepository, FlagRepository, OrganisationRepository,
    ProjectRepository, UpsertResultRow,
    experiment_results::{ExperimentResultsRepository, PgExperimentResultsRepository},
    repository::pg::{
        PgAuditLogger, PgEnvironmentRepository, PgExperimentRepository, PgFlagRepository,
        PgOrganisationRepository, PgProjectRepository,
    },
};

// ── helpers ───────────────────────────────────────────────────────────────────

/// Seed the DB with all entities required before an experiment can be created.
async fn setup_deps(pool: sqlx::PgPool) -> (EnvironmentId, RuleId) {
    let audit = Arc::new(PgAuditLogger::new(pool.clone()));
    let org_repo = PgOrganisationRepository::new(pool.clone(), audit.clone());
    let proj_repo = PgProjectRepository::new(pool.clone(), audit.clone());
    let env_repo = PgEnvironmentRepository::new(pool.clone(), audit.clone());
    let flag_repo = PgFlagRepository::new(pool.clone(), audit.clone());

    let org = Organisation {
        id: OrganisationId::new(),
        name: "Lifecycle Org".into(),
        created_at: Utc::now(),
        updated_at: Utc::now(),
        deleted_at: None,
        version: 1,
    };
    org_repo.create(&org).await.unwrap();

    let project = Project {
        id: ProjectId::new(),
        organisation_id: org.id,
        name: "Lifecycle Project".into(),
        created_at: Utc::now(),
        updated_at: Utc::now(),
        deleted_at: None,
        version: 1,
    };
    proj_repo.create(&project).await.unwrap();

    let env = Environment {
        id: EnvironmentId::new(),
        project_id: project.id,
        name: "Lifecycle Env".into(),
        created_at: Utc::now(),
        updated_at: Utc::now(),
        deleted_at: None,
        version: 1,
    };
    env_repo.create(&env).await.unwrap();

    let flag = FlagRecord {
        id: FlagId::new(),
        project_id: project.id,
        key: FlagKey::new("lifecycle-flag").unwrap(),
        value_type: FlagValueType::Bool,
        enabled: true,
        default_variant_id: None,
        created_at: Utc::now(),
        updated_at: Utc::now(),
        deleted_at: None,
        version: 1,
    };
    flag_repo.create(&flag).await.unwrap();

    let rules = vec![FlagRule {
        flag_id: flag.id,
        rule_index: 0,
        rule: Rule {
            id: RuleId::new(),
            condition: ConditionExpr::And(vec![]),
            output: RuleOutput::Variant(VariantId::new()),
        },
    }];
    flag_repo.upsert_rules(flag.id, &rules).await.unwrap();

    let rule_uuid: uuid::Uuid = sqlx::query_scalar!(
        "SELECT id FROM feature_flag_rules WHERE flag_id = $1 LIMIT 1",
        flag.id.as_uuid()
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let flag_rule_id = RuleId::from_uuid(rule_uuid);

    (env.id, flag_rule_id)
}

/// Build an `Experiment` value (not yet persisted).
fn make_experiment(env_id: EnvironmentId, flag_rule_id: RuleId) -> Experiment {
    Experiment {
        id: ExperimentId::new(),
        environment_id: env_id,
        flag_rule_id,
        name: "Lifecycle Test Experiment".into(),
        description: None,
        hypothesis: None,
        metric_keys: vec!["conversion".into()],
        traffic_allocation: 100.0,
        min_sample_size: None,
        scheduled_start_at: None,
        scheduled_end_at: None,
        status: ExperimentStatus::Draft,
        created_at: Utc::now(),
        updated_at: Utc::now(),
        deleted_at: None,
        version: 1,
    }
}

/// Build a `VariantStats` for a count metric.
fn count_stats(sample_size: i64, conversions: i64) -> VariantStats {
    let rate = if sample_size > 0 {
        Some(conversions as f64 / sample_size as f64)
    } else {
        None
    };
    VariantStats {
        sample_size,
        conversions: Some(conversions),
        mean: rate,
        variance: None,
        conversion_rate: rate,
        percentiles: None,
    }
}

/// Build a `VariantStats` for a funnel metric (final step / first step).
fn funnel_stats(first_step: i64, final_step: i64) -> VariantStats {
    let rate = if first_step > 0 {
        Some(final_step as f64 / first_step as f64)
    } else {
        None
    };
    VariantStats {
        sample_size: first_step,
        conversions: Some(final_step),
        mean: rate,
        variance: None,
        conversion_rate: rate,
        percentiles: None,
    }
}

// ── Scenario 1: Frequentist — significant result confirmed ────────────────────

/// Control 100/1000, variant 150/1000.
/// After frequentist analysis: significant = true, p_value < 0.05,
/// recommendation starts with "variant_wins:".
#[sqlx::test(migrations = "../stitchd-db/migrations")]
async fn test_frequentist_significant_result(pool: sqlx::PgPool) {
    let (env_id, rule_id) = setup_deps(pool.clone()).await;
    let audit = Arc::new(PgAuditLogger::new(pool.clone()));
    let exp_repo = PgExperimentRepository::new(pool.clone(), audit);
    let results_repo = PgExperimentResultsRepository::new(pool.clone());

    let exp = make_experiment(env_id, rule_id);
    exp_repo.create(&exp).await.unwrap();
    exp_repo
        .apply_transition(exp.id, ExperimentStatus::Running, None)
        .await
        .unwrap();

    let iters = exp_repo.list_iterations(exp.id).await.unwrap();
    let iter = &iters[0];

    let control = count_stats(1000, 100);
    let variant = count_stats(1000, 150);

    let freq_result = frequentist::analyze_count(&control, &variant);

    // Core assertions on the analysis result itself
    assert!(freq_result.significant, "result should be significant");
    assert!(
        freq_result.p_value < 0.05,
        "p_value should be < 0.05, got {}",
        freq_result.p_value
    );
    // CI lower > 0 means variant is better
    assert!(
        freq_result.confidence_interval.lower > 0.0,
        "CI lower should be > 0 (variant wins)"
    );

    // Build recommendation
    let rec_input = RecommendationInput {
        variant_key: "variant_a".to_string(),
        is_control: false,
        frequentist: Some(freq_result.clone()),
        bayesian: None,
        analysis_type: AnalysisType::Frequentist,
        sample_size: variant.sample_size,
        min_sample_size: None,
    };
    let rec = recommendation::recommend(&rec_input);
    let rec_str = rec.to_string();
    assert!(
        rec_str.starts_with("variant_wins:"),
        "recommendation should be variant_wins:…, got {rec_str}"
    );

    // Persist and verify round-trip
    let row = UpsertResultRow {
        experiment_id: exp.id.as_uuid(),
        iteration_id: iter.id.as_uuid(),
        metric_key: "conversion".to_string(),
        metric_type: "count".to_string(),
        variant_stats: serde_json::json!({ "control": control, "variant_a": variant }),
        frequentist_result: Some(serde_json::to_value(&freq_result).unwrap()),
        bayesian_result: None,
        recommendation: rec_str.clone(),
        computed_at: Utc::now(),
    };
    let returned = results_repo.upsert(&row).await.unwrap();

    assert_eq!(returned.recommendation, rec_str);
    let stored_freq: FrequentistResult =
        serde_json::from_value(returned.frequentist_result.unwrap()).unwrap();
    assert!(stored_freq.significant);
    assert!(stored_freq.p_value < 0.05);
}

// ── Scenario 2: Bayesian — P(best) > 0.95 for clear winner ───────────────────

/// Same data (100/1000 vs 150/1000): Bayesian prob_best should exceed 0.95.
#[sqlx::test(migrations = "../stitchd-db/migrations")]
async fn test_bayesian_prob_best_clear_winner(pool: sqlx::PgPool) {
    let (env_id, rule_id) = setup_deps(pool.clone()).await;
    let audit = Arc::new(PgAuditLogger::new(pool.clone()));
    let exp_repo = PgExperimentRepository::new(pool.clone(), audit);
    let results_repo = PgExperimentResultsRepository::new(pool.clone());

    let exp = make_experiment(env_id, rule_id);
    exp_repo.create(&exp).await.unwrap();
    exp_repo
        .apply_transition(exp.id, ExperimentStatus::Running, None)
        .await
        .unwrap();

    let iters = exp_repo.list_iterations(exp.id).await.unwrap();
    let iter = &iters[0];

    let control = count_stats(1000, 100);
    let variant = count_stats(1000, 150);

    let bayes_result = bayesian::analyze_count(&control, &variant);

    assert!(
        bayes_result.prob_best > 0.95,
        "prob_best should exceed 0.95 for clear winner, got {}",
        bayes_result.prob_best
    );

    // Recommendation
    let rec_input = RecommendationInput {
        variant_key: "variant_a".to_string(),
        is_control: false,
        frequentist: None,
        bayesian: Some(bayes_result.clone()),
        analysis_type: AnalysisType::Bayesian,
        sample_size: variant.sample_size,
        min_sample_size: None,
    };
    let rec = recommendation::recommend(&rec_input);
    assert!(
        rec.to_string().starts_with("variant_wins:"),
        "expected variant_wins:…, got {}",
        rec
    );

    // Persist
    let row = UpsertResultRow {
        experiment_id: exp.id.as_uuid(),
        iteration_id: iter.id.as_uuid(),
        metric_key: "conversion".to_string(),
        metric_type: "count".to_string(),
        variant_stats: serde_json::json!({ "control": control, "variant_a": variant }),
        frequentist_result: None,
        bayesian_result: Some(serde_json::to_value(&bayes_result).unwrap()),
        recommendation: rec.to_string(),
        computed_at: Utc::now(),
    };
    let returned = results_repo.upsert(&row).await.unwrap();

    let stored_bayes: BayesianResult =
        serde_json::from_value(returned.bayesian_result.unwrap()).unwrap();
    assert!(stored_bayes.prob_best > 0.95);
}

// ── Scenario 3: Inconclusive — close metrics ─────────────────────────────────

/// Control 100/1000, variant 102/1000: result should be inconclusive.
#[sqlx::test(migrations = "../stitchd-db/migrations")]
async fn test_frequentist_inconclusive_close_metrics(pool: sqlx::PgPool) {
    let (env_id, rule_id) = setup_deps(pool.clone()).await;
    let audit = Arc::new(PgAuditLogger::new(pool.clone()));
    let exp_repo = PgExperimentRepository::new(pool.clone(), audit);
    let results_repo = PgExperimentResultsRepository::new(pool.clone());

    let exp = make_experiment(env_id, rule_id);
    exp_repo.create(&exp).await.unwrap();
    exp_repo
        .apply_transition(exp.id, ExperimentStatus::Running, None)
        .await
        .unwrap();

    let iters = exp_repo.list_iterations(exp.id).await.unwrap();
    let iter = &iters[0];

    let control = count_stats(1000, 100);
    let variant = count_stats(1000, 102);

    let freq_result = frequentist::analyze_count(&control, &variant);

    // Tiny difference — should not be significant
    assert!(
        !freq_result.significant,
        "close metrics should not yield a significant result, p_value={}",
        freq_result.p_value
    );

    let rec_input = RecommendationInput {
        variant_key: "variant_a".to_string(),
        is_control: false,
        frequentist: Some(freq_result.clone()),
        bayesian: None,
        analysis_type: AnalysisType::Frequentist,
        sample_size: variant.sample_size,
        min_sample_size: None,
    };
    let rec = recommendation::recommend(&rec_input);
    assert_eq!(rec.to_string(), "inconclusive");

    // Persist and verify
    let row = UpsertResultRow {
        experiment_id: exp.id.as_uuid(),
        iteration_id: iter.id.as_uuid(),
        metric_key: "conversion".to_string(),
        metric_type: "count".to_string(),
        variant_stats: serde_json::json!({ "control": control, "variant_a": variant }),
        frequentist_result: Some(serde_json::to_value(&freq_result).unwrap()),
        bayesian_result: None,
        recommendation: rec.to_string(),
        computed_at: Utc::now(),
    };
    let returned = results_repo.upsert(&row).await.unwrap();
    assert_eq!(returned.recommendation, "inconclusive");
}

// ── Scenario 4: NeedsMoreData guardrail ──────────────────────────────────────

/// Control 5/50, variant 8/50, min_sample_size = 200.
/// Guardrail fires before stats are evaluated.
#[sqlx::test(migrations = "../stitchd-db/migrations")]
async fn test_needs_more_data_guardrail(pool: sqlx::PgPool) {
    let (env_id, rule_id) = setup_deps(pool.clone()).await;
    let audit = Arc::new(PgAuditLogger::new(pool.clone()));
    let exp_repo = PgExperimentRepository::new(pool.clone(), audit);
    let results_repo = PgExperimentResultsRepository::new(pool.clone());

    let exp = make_experiment(env_id, rule_id);
    exp_repo.create(&exp).await.unwrap();
    exp_repo
        .apply_transition(exp.id, ExperimentStatus::Running, None)
        .await
        .unwrap();

    let iters = exp_repo.list_iterations(exp.id).await.unwrap();
    let iter = &iters[0];

    let control = count_stats(50, 5);
    let variant = count_stats(50, 8);
    let min_sample_size: i64 = 200;

    // Stats analysis (if it were run, it might look significant due to proportions)
    let freq_result = frequentist::analyze_count(&control, &variant);

    let rec_input = RecommendationInput {
        variant_key: "variant_a".to_string(),
        is_control: false,
        frequentist: Some(freq_result.clone()),
        bayesian: None,
        analysis_type: AnalysisType::Frequentist,
        sample_size: variant.sample_size,
        min_sample_size: Some(min_sample_size),
    };
    let rec = recommendation::recommend(&rec_input);
    assert_eq!(
        rec.to_string(),
        "needs_more_data",
        "guardrail should fire because sample_size ({}) < min_sample_size ({})",
        variant.sample_size,
        min_sample_size
    );

    // Persist and verify
    let row = UpsertResultRow {
        experiment_id: exp.id.as_uuid(),
        iteration_id: iter.id.as_uuid(),
        metric_key: "conversion".to_string(),
        metric_type: "count".to_string(),
        variant_stats: serde_json::json!({ "control": control, "variant_a": variant }),
        frequentist_result: Some(serde_json::to_value(&freq_result).unwrap()),
        bayesian_result: None,
        recommendation: rec.to_string(),
        computed_at: Utc::now(),
    };
    let returned = results_repo.upsert(&row).await.unwrap();
    assert_eq!(returned.recommendation, "needs_more_data");
}

// ── Scenario 5: Per-iteration isolation ──────────────────────────────────────

/// Insert results for iteration 1 and iteration 2 with different data.
/// Fetch each independently and verify they don't bleed into each other.
#[sqlx::test(migrations = "../stitchd-db/migrations")]
async fn test_per_iteration_isolation(pool: sqlx::PgPool) {
    let (env_id, rule_id) = setup_deps(pool.clone()).await;
    let audit = Arc::new(PgAuditLogger::new(pool.clone()));
    let exp_repo = PgExperimentRepository::new(pool.clone(), audit);
    let results_repo = PgExperimentResultsRepository::new(pool.clone());

    let exp = make_experiment(env_id, rule_id);
    exp_repo.create(&exp).await.unwrap();

    // Iteration 1: draft → running
    exp_repo
        .apply_transition(exp.id, ExperimentStatus::Running, None)
        .await
        .unwrap();
    let iters1 = exp_repo.list_iterations(exp.id).await.unwrap();
    assert_eq!(iters1.len(), 1);
    let iter1_id = iters1[0].id.as_uuid();

    let control1 = count_stats(1000, 100);
    let variant1 = count_stats(1000, 150);
    let freq1 = frequentist::analyze_count(&control1, &variant1);

    let row1 = UpsertResultRow {
        experiment_id: exp.id.as_uuid(),
        iteration_id: iter1_id,
        metric_key: "conversion".to_string(),
        metric_type: "count".to_string(),
        variant_stats: serde_json::json!({ "control": control1, "variant_a": variant1 }),
        frequentist_result: Some(serde_json::to_value(&freq1).unwrap()),
        bayesian_result: None,
        recommendation: "variant_wins:variant_a".to_string(),
        computed_at: Utc::now(),
    };
    results_repo.upsert(&row1).await.unwrap();

    // Iteration 2: running → paused → running
    exp_repo
        .apply_transition(exp.id, ExperimentStatus::Paused, None)
        .await
        .unwrap();
    exp_repo
        .apply_transition(exp.id, ExperimentStatus::Running, None)
        .await
        .unwrap();
    let iters2 = exp_repo.list_iterations(exp.id).await.unwrap();
    assert_eq!(iters2.len(), 2);
    let iter2_id = iters2[1].id.as_uuid();

    let control2 = count_stats(500, 50); // Different data
    let variant2 = count_stats(500, 51);
    let freq2 = frequentist::analyze_count(&control2, &variant2);

    let row2 = UpsertResultRow {
        experiment_id: exp.id.as_uuid(),
        iteration_id: iter2_id,
        metric_key: "conversion".to_string(),
        metric_type: "count".to_string(),
        variant_stats: serde_json::json!({ "control": control2, "variant_a": variant2 }),
        frequentist_result: Some(serde_json::to_value(&freq2).unwrap()),
        bayesian_result: None,
        recommendation: "inconclusive".to_string(),
        computed_at: Utc::now(),
    };
    results_repo.upsert(&row2).await.unwrap();

    // Fetch each iteration independently
    let rows_iter1 = results_repo
        .fetch_by_iteration(exp.id.as_uuid(), iter1_id)
        .await
        .unwrap();
    let rows_iter2 = results_repo
        .fetch_by_iteration(exp.id.as_uuid(), iter2_id)
        .await
        .unwrap();

    assert_eq!(rows_iter1.len(), 1);
    assert_eq!(rows_iter2.len(), 1);

    // Results should differ between iterations
    assert_ne!(
        rows_iter1[0].recommendation, rows_iter2[0].recommendation,
        "iteration 1 and 2 should have different recommendations"
    );
    assert_eq!(rows_iter1[0].iteration_id, iter1_id);
    assert_eq!(rows_iter2[0].iteration_id, iter2_id);

    // Verify variant_stats are distinct
    assert_ne!(
        rows_iter1[0].variant_stats, rows_iter2[0].variant_stats,
        "variant stats should differ between iterations"
    );
}

// ── Scenario 6: Recompute → fresh data ───────────────────────────────────────

/// Insert a stale result (computed_at in the past before the iteration started).
/// Confirm `is_stale` returns true, then recompute and confirm `computed_at` updated.
#[sqlx::test(migrations = "../stitchd-db/migrations")]
async fn test_recompute_updates_computed_at(pool: sqlx::PgPool) {
    let (env_id, rule_id) = setup_deps(pool.clone()).await;
    let audit = Arc::new(PgAuditLogger::new(pool.clone()));
    let exp_repo = PgExperimentRepository::new(pool.clone(), audit);
    let results_repo = PgExperimentResultsRepository::new(pool.clone());

    let exp = make_experiment(env_id, rule_id);
    exp_repo.create(&exp).await.unwrap();
    exp_repo
        .apply_transition(exp.id, ExperimentStatus::Running, None)
        .await
        .unwrap();

    let iters = exp_repo.list_iterations(exp.id).await.unwrap();
    let iter = &iters[0];

    // Insert with computed_at in the past (before started_at)
    let stale_computed_at = iter.started_at - Duration::hours(1);

    let control = count_stats(1000, 100);
    let variant = count_stats(1000, 150);
    let freq_result = frequentist::analyze_count(&control, &variant);

    let stale_row = UpsertResultRow {
        experiment_id: exp.id.as_uuid(),
        iteration_id: iter.id.as_uuid(),
        metric_key: "conversion".to_string(),
        metric_type: "count".to_string(),
        variant_stats: serde_json::json!({ "control": control, "variant_a": variant }),
        frequentist_result: Some(serde_json::to_value(&freq_result).unwrap()),
        bayesian_result: None,
        recommendation: "variant_wins:variant_a".to_string(),
        computed_at: stale_computed_at,
    };
    results_repo.upsert(&stale_row).await.unwrap();

    // Confirm is_stale returns true
    let stale = results_repo
        .is_stale(exp.id.as_uuid(), iter.id.as_uuid())
        .await
        .unwrap();
    assert!(
        stale,
        "result should be stale when computed_at is before started_at"
    );

    // Recompute: upsert with fresh computed_at (now + 1s to ensure > started_at)
    let fresh_computed_at = Utc::now() + Duration::seconds(1);
    let fresh_row = UpsertResultRow {
        computed_at: fresh_computed_at,
        ..stale_row
    };
    let returned = results_repo.upsert(&fresh_row).await.unwrap();

    assert!(
        returned.computed_at >= fresh_computed_at,
        "computed_at should be updated after recompute"
    );

    // is_stale should now return false
    let stale_after = results_repo
        .is_stale(exp.id.as_uuid(), iter.id.as_uuid())
        .await
        .unwrap();
    assert!(
        !stale_after,
        "result should no longer be stale after recompute"
    );
}

// ── Scenario 7: Funnel metric — correct chained conversion rate ───────────────

/// Funnel with 3 steps:
///   control:  step0=1000, step1=500, step2=200  → conversion_rate = 200/1000 = 0.20
///   variant:  step0=1000, step1=600, step2=350  → conversion_rate = 350/1000 = 0.35
#[sqlx::test(migrations = "../stitchd-db/migrations")]
async fn test_funnel_metric_correct_conversion_rate(pool: sqlx::PgPool) {
    let (env_id, rule_id) = setup_deps(pool.clone()).await;
    let audit = Arc::new(PgAuditLogger::new(pool.clone()));
    let exp_repo = PgExperimentRepository::new(pool.clone(), audit);
    let results_repo = PgExperimentResultsRepository::new(pool.clone());

    let exp = make_experiment(env_id, rule_id);
    exp_repo.create(&exp).await.unwrap();
    exp_repo
        .apply_transition(exp.id, ExperimentStatus::Running, None)
        .await
        .unwrap();

    let iters = exp_repo.list_iterations(exp.id).await.unwrap();
    let iter = &iters[0];

    // Build funnel stats: final_step / first_step
    let control = funnel_stats(1000, 200); // 20%
    let variant = funnel_stats(1000, 350); // 35%

    // Verify conversion_rate calculation
    let ctrl_rate = control.conversion_rate.unwrap();
    let var_rate = variant.conversion_rate.unwrap();
    assert!(
        (ctrl_rate - 0.20).abs() < 1e-9,
        "control conversion_rate should be 0.20, got {ctrl_rate}"
    );
    assert!(
        (var_rate - 0.35).abs() < 1e-9,
        "variant conversion_rate should be 0.35, got {var_rate}"
    );

    // Run funnel analysis
    let freq_result = frequentist::analyze_funnel(&control, &variant);
    assert!(
        freq_result.significant,
        "funnel result should be significant for 20% vs 35% at n=1000"
    );
    assert!(
        freq_result.p_value < 0.05,
        "p_value should be < 0.05, got {}",
        freq_result.p_value
    );
    assert!(
        freq_result.confidence_interval.lower > 0.0,
        "CI lower should be > 0 (variant wins)"
    );

    // Recommendation
    let rec_input = RecommendationInput {
        variant_key: "variant_a".to_string(),
        is_control: false,
        frequentist: Some(freq_result.clone()),
        bayesian: None,
        analysis_type: AnalysisType::Frequentist,
        sample_size: variant.sample_size,
        min_sample_size: None,
    };
    let rec = recommendation::recommend(&rec_input);
    assert!(
        rec.to_string().starts_with("variant_wins:"),
        "funnel recommendation should be variant_wins:…, got {}",
        rec
    );

    // Persist and verify round-trip
    let row = UpsertResultRow {
        experiment_id: exp.id.as_uuid(),
        iteration_id: iter.id.as_uuid(),
        metric_key: "signup_funnel".to_string(),
        metric_type: "funnel".to_string(),
        variant_stats: serde_json::json!({
            "control": {
                "sample_size": 1000,
                "conversions": 200,
                "conversion_rate": 0.20,
                "mean": 0.20,
                "variance": null,
                "percentiles": null
            },
            "variant_a": {
                "sample_size": 1000,
                "conversions": 350,
                "conversion_rate": 0.35,
                "mean": 0.35,
                "variance": null,
                "percentiles": null
            }
        }),
        frequentist_result: Some(serde_json::to_value(&freq_result).unwrap()),
        bayesian_result: None,
        recommendation: rec.to_string(),
        computed_at: Utc::now(),
    };
    let returned = results_repo.upsert(&row).await.unwrap();

    assert_eq!(returned.metric_type, "funnel");
    assert!(returned.recommendation.starts_with("variant_wins:"));

    // Verify stored variant_stats conversion rates
    let stored_stats = &returned.variant_stats;
    let stored_ctrl_rate = stored_stats["control"]["conversion_rate"]
        .as_f64()
        .expect("control conversion_rate should be a number");
    let stored_var_rate = stored_stats["variant_a"]["conversion_rate"]
        .as_f64()
        .expect("variant_a conversion_rate should be a number");

    assert!(
        (stored_ctrl_rate - 0.20).abs() < 1e-9,
        "stored control conversion_rate should be 0.20, got {stored_ctrl_rate}"
    );
    assert!(
        (stored_var_rate - 0.35).abs() < 1e-9,
        "stored variant_a conversion_rate should be 0.35, got {stored_var_rate}"
    );
}
