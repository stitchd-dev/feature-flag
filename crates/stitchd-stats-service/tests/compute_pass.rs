//! Live-ClickHouse integration test for the per-metric compute pass.
//!
//! Seeds `experiment_assignments` + `events` for a 2-variant experiment with a
//! KNOWN conversion effect, runs [`run_stats_compute`] for a single count
//! metric on the `user` context, and asserts the produced [`MetricSummary`]
//! carries:
//!
//!   * a non-empty `frequentist_result` with a sane (significant) p-value for
//!     the treatment vs control comparison,
//!   * a `bayesian_result` with a high `prob_best` for treatment, and
//!   * (sequential enabled) a `sequential_result` blob keyed by both variants
//!     with `treatment.p_crossed == true`.
//!
//! This is the end-to-end proof that the sufficient-stats SQL + VariantStats
//! construction + frequentist/bayesian/sequential engines wire together against
//! the real database.
//!
//! Tagged `#[ignore]` so the default `cargo test` run needs no infrastructure.
//! Run explicitly with:
//!
//! ```sh
//! DATABASE_URL=… cargo test -p stitchd-stats-service --test compute_pass -- --ignored
//! ```

#![allow(clippy::expect_used)]

use std::collections::HashMap;
use std::sync::Arc;

use chrono::{Duration, TimeZone, Utc};
use clickhouse::Client;
use stitchd_core::id::{EnvironmentId, MetricId};
use stitchd_core::metric::{
    AggregationConfig, AggregationOperator, GoalDirection, MetricDefinition, MetricKind,
};
use stitchd_db::clickhouse::SeedAssignmentRow;
use stitchd_event_writer::SeedEventRow;
use stitchd_stats_service::compute::{ClickHouseCellReader, run_stats_compute};
use stitchd_stats_service::scheduler::{RunningExperiment, SequentialSettings};
use uuid::Uuid;

type AssignmentRow = SeedAssignmentRow;
type EventRow = SeedEventRow;

fn make_client() -> Client {
    let url = std::env::var("STITCHD_CLICKHOUSE_URL")
        .unwrap_or_else(|_| "http://localhost:8123".to_string());
    let db = std::env::var("STITCHD_CLICKHOUSE_DB").unwrap_or_else(|_| "stitchd".to_string());
    let user = std::env::var("STITCHD_CLICKHOUSE_USER").unwrap_or_else(|_| "stitchd".to_string());
    let password =
        std::env::var("STITCHD_CLICKHOUSE_PASSWORD").unwrap_or_else(|_| "stitchd".to_string());
    Client::default()
        .with_url(url)
        .with_database(db)
        .with_user(user)
        .with_password(password)
}

async fn insert_assignments(ch: &Client, rows: &[AssignmentRow]) {
    let mut insert = ch
        .insert::<AssignmentRow>("experiment_assignments")
        .await
        .expect("prepare assignments insert");
    for row in rows {
        insert.write(row).await.expect("write assignment row");
    }
    insert.end().await.expect("finalize assignments insert");
}

async fn insert_events(ch: &Client, rows: &[EventRow]) {
    let mut insert = ch
        .insert::<EventRow>("events")
        .await
        .expect("prepare events insert");
    for row in rows {
        insert.write(row).await.expect("write event row");
    }
    insert.end().await.expect("finalize events insert");
}

/// Build a deterministic `(env, exp, iter)` and seed N units per variant, with a
/// per-variant conversion count. Returns the IDs + metric definition.
struct Seeded {
    env_id: Uuid,
    exp_id: Uuid,
    iter_id: Uuid,
    metric: MetricDefinition,
    iter_end: chrono::DateTime<Utc>,
}

async fn seed_count_experiment(
    ch: &Client,
    n_per_variant: usize,
    control_conversions: usize,
    treatment_conversions: usize,
) -> Seeded {
    let env_id = Uuid::new_v4();
    let exp_id = Uuid::new_v4();
    let iter_id = Uuid::new_v4();
    let flag_id = Uuid::new_v4();
    let assigned_at = Utc.with_ymd_and_hms(2026, 5, 1, 0, 0, 0).unwrap();
    let iter_end = Utc.with_ymd_and_hms(2026, 5, 31, 0, 0, 0).unwrap();
    let event_at = assigned_at + Duration::hours(1);
    let metric_key = format!("checkout_completed_{}", &exp_id.to_string()[..8]);

    // Assignments: N control + N treatment, unique context keys.
    let mut assignments = Vec::with_capacity(2 * n_per_variant);
    let mk_assignment = |variant: &str, key: String| AssignmentRow {
        experiment_id: exp_id,
        iteration_id: iter_id,
        env_id,
        flag_id,
        matched_rule_id: None,
        context_type: "user".into(),
        context_key: key,
        variant_key: variant.into(),
        assigned_at,
        version: -assigned_at.timestamp_millis(),
    };
    for i in 0..n_per_variant {
        assignments.push(mk_assignment("control", format!("c_{i}")));
        assignments.push(mk_assignment("treatment", format!("t_{i}")));
    }
    insert_assignments(ch, &assignments).await;

    // Events: one conversion per converting unit (post-exposure, in-window).
    let mut events = Vec::new();
    let mk_event = |key: String| EventRow {
        env_id,
        contexts: vec![("user".into(), key)],
        metric_key: metric_key.clone(),
        value_bool: None,
        value_int: None,
        value_double: None,
        timestamp: event_at,
        ingested_at: event_at,
        properties: vec![],
        occurred_at: event_at,
    };
    for i in 0..control_conversions {
        events.push(mk_event(format!("c_{i}")));
    }
    for i in 0..treatment_conversions {
        events.push(mk_event(format!("t_{i}")));
    }
    insert_events(ch, &events).await;

    let now = Utc::now();
    let metric = MetricDefinition {
        id: MetricId::new(),
        environment_id: EnvironmentId::from_uuid(env_id),
        key: metric_key,
        name: "checkout".into(),
        description: None,
        kind: MetricKind::Aggregation(AggregationConfig {
            event_key: "x".into(), // overwritten below to match seeded metric_key
            aggregator: AggregationOperator::Count,
            on_field: None,
            where_clause: None,
        }),
        goal_direction: GoalDirection::Increase,
        version: 1,
        created_at: now,
        updated_at: now,
        deleted_at: None,
    };
    Seeded {
        env_id,
        exp_id,
        iter_id,
        metric,
        iter_end,
    }
}

fn running_experiment(s: &Seeded, sequential_enabled: bool) -> RunningExperiment {
    RunningExperiment {
        experiment_id: s.exp_id,
        env_id: s.env_id,
        iteration_id: s.iter_id,
        metric_ids: vec![s.metric.id.as_uuid()],
        variant_keys: vec!["control".into(), "treatment".into()],
        started_at: Utc::now(),
        unit_context_types: vec!["user".into()],
        pre_period_days: 0,
        sequential: SequentialSettings {
            enabled: sequential_enabled,
            alpha: 0.05,
            tau_squared: None,
            min_sample_size: 100,
        },
        // Default: no designed split → uniform SRM fallback. The weighted-SRM
        // integration test below overrides this on the returned struct.
        variant_expected_bp: HashMap::new(),
        experiment_mode: stitchd_core::experimentation::bandit::ExperimentMode::Fixed,
        bandit_config: None,
        bandit_campaign_id: None,
    }
}

/// Align the metric's `event_key` with the seeded metric_key (the count
/// aggregator reads `event_key`, which IS the seeded metric_key).
fn align_event_key(metric: &mut MetricDefinition, key: &str) {
    if let MetricKind::Aggregation(cfg) = &mut metric.kind {
        cfg.event_key = key.to_string();
    }
}

/// Full end-to-end: seed a clear count effect, run the compute pass, and assert
/// the frequentist + bayesian + sequential blobs are produced and sane.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs running clickhouse"]
async fn compute_pass_produces_frequentist_bayesian_sequential() {
    let ch = make_client();
    stitchd_event_writer::migrations::run(&ch)
        .await
        .expect("apply CH migrations");

    // 1000 units/variant; control 100/1000 (10 %), treatment 200/1000 (20 %) —
    // a large, clearly-significant lift.
    let mut seeded = seed_count_experiment(&ch, 1000, 100, 200).await;
    let metric_key = seeded.metric.key.clone();
    align_event_key(&mut seeded.metric, &metric_key);

    let exp = running_experiment(&seeded, true);
    let metrics: HashMap<Uuid, MetricDefinition> =
        [(seeded.metric.id.as_uuid(), seeded.metric.clone())]
            .into_iter()
            .collect();

    let reader = ClickHouseCellReader::new(Arc::new(ch.clone()));
    let now = seeded.iter_end + Duration::days(1);
    let summaries = run_stats_compute(&reader, &ch, &exp, &metrics, seeded.iter_end, now)
        .await
        .expect("compute pass should succeed");

    // One summary: (metric_key, "user").
    assert_eq!(summaries.len(), 1, "expected one (metric, context) summary");
    let s = &summaries[0];
    assert_eq!(s.context_type, "user");
    assert_eq!(s.metric_key, metric_key);
    assert_eq!(s.metric_type, "count", "count metric type threaded through");

    // variant_stats carries the per-variant point conversion rates.
    let vstats = s.variant_stats.as_object().expect("variant_stats object");
    let control_rate = vstats["control"].as_f64().expect("control rate");
    let treatment_rate = vstats["treatment"].as_f64().expect("treatment rate");
    assert!(
        (control_rate - 0.10).abs() < 1e-6,
        "control rate should be 0.10, got {control_rate}"
    );
    assert!(
        (treatment_rate - 0.20).abs() < 1e-6,
        "treatment rate should be 0.20, got {treatment_rate}"
    );

    // Frequentist: treatment entry with a small, significant p-value.
    let freq = s
        .frequentist_result
        .as_ref()
        .expect("frequentist_result present");
    let tr_freq = &freq["treatment"];
    let p = tr_freq["p_value"].as_f64().expect("p_value");
    assert!(
        p < 0.001,
        "10%→20% lift on n=1000 should be highly significant; got p={p}"
    );
    assert_eq!(
        tr_freq["significant"],
        serde_json::json!(true),
        "treatment should be flagged significant"
    );
    // Bonferroni-corrected p present (K-1 = 1 comparison → equals raw p here).
    assert!(
        tr_freq.get("p_value_corrected").is_some(),
        "corrected p should be attached, got {tr_freq}"
    );

    // Bayesian: treatment near-certain to beat control.
    let bayes = s.bayesian_result.as_ref().expect("bayesian_result present");
    let prob_best = bayes["treatment"]["prob_best"].as_f64().expect("prob_best");
    assert!(
        prob_best > 0.95,
        "treatment prob_best should be high; got {prob_best}"
    );

    // Sequential: both variants present; treatment crossed.
    let seq = s
        .sequential_result
        .as_ref()
        .expect("sequential_result present (sequential enabled)");
    let seq_obj = seq.as_object().expect("sequential object");
    assert!(seq_obj.contains_key("control"), "control baseline present");
    assert!(seq_obj.contains_key("treatment"), "treatment present");
    assert_eq!(
        seq_obj["control"]["always_valid_p"],
        serde_json::json!(1.0),
        "control is the baseline"
    );
    assert_eq!(
        seq_obj["treatment"]["method"],
        serde_json::json!("msprt"),
        "treatment tagged msprt"
    );
    assert_eq!(
        seq_obj["treatment"]["p_crossed"],
        serde_json::json!(true),
        "a 10%→20% lift on 1000 units should cross the always-valid threshold"
    );
    assert_eq!(
        seq_obj["treatment"]["insufficient_data"],
        serde_json::json!(false)
    );

    // Print the observed numbers so the test run records the actual p / blob.
    println!(
        "compute_pass observed: freq.p_value={p} bayes.prob_best={prob_best} sequential={}",
        serde_json::to_string(seq).unwrap()
    );
}

/// Sequential disabled → no `sequential_result` blob, but frequentist +
/// bayesian still produced.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs running clickhouse"]
async fn compute_pass_omits_sequential_when_disabled() {
    let ch = make_client();
    stitchd_event_writer::migrations::run(&ch)
        .await
        .expect("apply CH migrations");

    let mut seeded = seed_count_experiment(&ch, 500, 50, 90).await;
    let metric_key = seeded.metric.key.clone();
    align_event_key(&mut seeded.metric, &metric_key);

    let exp = running_experiment(&seeded, false);
    let metrics: HashMap<Uuid, MetricDefinition> =
        [(seeded.metric.id.as_uuid(), seeded.metric.clone())]
            .into_iter()
            .collect();

    let reader = ClickHouseCellReader::new(Arc::new(ch.clone()));
    let now = seeded.iter_end + Duration::days(1);
    let summaries = run_stats_compute(&reader, &ch, &exp, &metrics, seeded.iter_end, now)
        .await
        .expect("compute pass should succeed");

    assert_eq!(summaries.len(), 1);
    let s = &summaries[0];
    assert!(
        s.frequentist_result.is_some(),
        "frequentist still produced when sequential disabled"
    );
    assert!(s.bayesian_result.is_some(), "bayesian still produced");
    assert!(
        s.sequential_result.is_none(),
        "sequential blob must be absent when disabled"
    );
}

/// ITT sample-size: a control unit with NO events still counts toward the
/// denominator (the conversion rate uses the assigned count, not the firing
/// count).
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs running clickhouse"]
async fn compute_pass_itt_denominator_counts_non_firing_units() {
    let ch = make_client();
    stitchd_event_writer::migrations::run(&ch)
        .await
        .expect("apply CH migrations");

    // 200 units/variant, but only 20 control + 60 treatment convert.
    let mut seeded = seed_count_experiment(&ch, 200, 20, 60).await;
    let metric_key = seeded.metric.key.clone();
    align_event_key(&mut seeded.metric, &metric_key);

    let exp = running_experiment(&seeded, false);
    let metrics: HashMap<Uuid, MetricDefinition> =
        [(seeded.metric.id.as_uuid(), seeded.metric.clone())]
            .into_iter()
            .collect();

    let reader = ClickHouseCellReader::new(Arc::new(ch.clone()));
    let now = seeded.iter_end + Duration::days(1);
    let summaries = run_stats_compute(&reader, &ch, &exp, &metrics, seeded.iter_end, now)
        .await
        .expect("compute pass should succeed");

    let s = &summaries[0];
    let vstats = s.variant_stats.as_object().unwrap();
    // 20/200 = 0.10, 60/200 = 0.30 — denominators are the ASSIGNED counts.
    assert!(
        (vstats["control"].as_f64().unwrap() - 0.10).abs() < 1e-6,
        "control ITT rate 20/200=0.10, got {}",
        vstats["control"]
    );
    assert!(
        (vstats["treatment"].as_f64().unwrap() - 0.30).abs() < 1e-6,
        "treatment ITT rate 60/200=0.30, got {}",
        vstats["treatment"]
    );
}

/// Seed an experiment whose two arms receive a NON-uniform assignment count
/// (`control_n` / `treatment_n` units), each with one conversion, so the SRM
/// observed split matches a designed canary ratio. Returns the IDs + metric.
async fn seed_assignment_split(ch: &Client, control_n: usize, treatment_n: usize) -> Seeded {
    let env_id = Uuid::new_v4();
    let exp_id = Uuid::new_v4();
    let iter_id = Uuid::new_v4();
    let flag_id = Uuid::new_v4();
    let assigned_at = Utc.with_ymd_and_hms(2026, 5, 1, 0, 0, 0).unwrap();
    let iter_end = Utc.with_ymd_and_hms(2026, 5, 31, 0, 0, 0).unwrap();
    let event_at = assigned_at + Duration::hours(1);
    let metric_key = format!("canary_metric_{}", &exp_id.to_string()[..8]);

    let mk_assignment = |variant: &str, key: String| AssignmentRow {
        experiment_id: exp_id,
        iteration_id: iter_id,
        env_id,
        flag_id,
        matched_rule_id: None,
        context_type: "user".into(),
        context_key: key,
        variant_key: variant.into(),
        assigned_at,
        version: -assigned_at.timestamp_millis(),
    };
    let mut assignments = Vec::with_capacity(control_n + treatment_n);
    for i in 0..control_n {
        assignments.push(mk_assignment("control", format!("c_{i}")));
    }
    for i in 0..treatment_n {
        assignments.push(mk_assignment("treatment", format!("t_{i}")));
    }
    insert_assignments(ch, &assignments).await;

    // One conversion per unit (keeps the metric well-defined; SRM uses the
    // ASSIGNMENT counts, not events).
    let mk_event = |key: String| EventRow {
        env_id,
        contexts: vec![("user".into(), key)],
        metric_key: metric_key.clone(),
        value_bool: None,
        value_int: None,
        value_double: None,
        timestamp: event_at,
        ingested_at: event_at,
        properties: vec![],
        occurred_at: event_at,
    };
    let mut events = Vec::new();
    for i in 0..control_n {
        events.push(mk_event(format!("c_{i}")));
    }
    for i in 0..treatment_n {
        events.push(mk_event(format!("t_{i}")));
    }
    insert_events(ch, &events).await;

    let now = Utc::now();
    let metric = MetricDefinition {
        id: MetricId::new(),
        environment_id: EnvironmentId::from_uuid(env_id),
        key: metric_key.clone(),
        name: "canary".into(),
        description: None,
        kind: MetricKind::Aggregation(AggregationConfig {
            event_key: metric_key,
            aggregator: AggregationOperator::Count,
            on_field: None,
            where_clause: None,
        }),
        goal_direction: GoalDirection::Increase,
        version: 1,
        created_at: now,
        updated_at: now,
        deleted_at: None,
    };
    Seeded {
        env_id,
        exp_id,
        iter_id,
        metric,
        iter_end,
    }
}

/// Read the SRM blob from the (single) metric summary's `variant_stats["srm"]`.
fn srm_blob(
    summaries: &[stitchd_stats_service::results_writer::MetricSummary],
) -> serde_json::Value {
    summaries[0]
        .variant_stats
        .as_object()
        .expect("variant_stats object")
        .get("srm")
        .expect("srm blob attached")
        .clone()
}

/// WEIGHTED SRM end-to-end: a 90/10 design with an observed 900/100 assignment
/// split is HEALTHY when `variant_expected_bp` carries the 90/10 design, but the
/// SAME observed split is a RED mismatch under the uniform `total/K` baseline
/// (expected 500/500). Proves the designed split flows from the proto field
/// through `run_stats_compute` into the SRM verdict against the real database.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs running clickhouse"]
async fn compute_pass_weighted_srm_canary_matches_design_is_green() {
    let ch = make_client();
    stitchd_event_writer::migrations::run(&ch)
        .await
        .expect("apply CH migrations");

    // 900 control + 100 treatment assignments → a 90/10 observed split.
    let seeded = seed_assignment_split(&ch, 900, 100).await;
    let metrics: HashMap<Uuid, MetricDefinition> =
        [(seeded.metric.id.as_uuid(), seeded.metric.clone())]
            .into_iter()
            .collect();
    let reader = ClickHouseCellReader::new(Arc::new(ch.clone()));
    let now = seeded.iter_end + Duration::days(1);

    // (1) WITH the designed 90/10 weights → SRM healthy (green).
    let mut exp_weighted = running_experiment(&seeded, false);
    exp_weighted.variant_expected_bp = HashMap::from([
        ("control".to_string(), 9000),
        ("treatment".to_string(), 1000),
    ]);
    let summaries = run_stats_compute(&reader, &ch, &exp_weighted, &metrics, seeded.iter_end, now)
        .await
        .expect("weighted compute pass should succeed");
    let srm = srm_blob(&summaries);
    assert_eq!(
        srm["health"], "green",
        "a 900/100 split under a 90/10 design must be healthy; got {srm}"
    );
    assert!(
        srm["overall_chi_sq"].as_f64().unwrap() < 1e-6,
        "observed == weighted-expected → χ² ≈ 0, got {}",
        srm["overall_chi_sq"]
    );
    // The weighted expected split is 900/100, not the uniform 500/500.
    let pv = srm["per_variant"].as_array().unwrap();
    let ctrl = pv.iter().find(|r| r["variant_key"] == "control").unwrap();
    assert!(
        (ctrl["expected"].as_f64().unwrap() - 900.0).abs() < 1e-6,
        "control expected should be the weighted 900, got {}",
        ctrl["expected"]
    );

    // (2) Same data, EMPTY weights → uniform baseline (500/500) → RED mismatch.
    let exp_uniform = running_experiment(&seeded, false); // variant_expected_bp empty
    let summaries_u = run_stats_compute(&reader, &ch, &exp_uniform, &metrics, seeded.iter_end, now)
        .await
        .expect("uniform compute pass should succeed");
    let srm_u = srm_blob(&summaries_u);
    assert_eq!(
        srm_u["health"], "red",
        "without the design weights the 90/10 split is a uniform-baseline SRM violation; got {srm_u}"
    );
    println!(
        "weighted SRM: weighted.health={} chi_sq={} | uniform.health={} chi_sq={}",
        srm["health"], srm["overall_chi_sq"], srm_u["health"], srm_u["overall_chi_sq"]
    );
}
