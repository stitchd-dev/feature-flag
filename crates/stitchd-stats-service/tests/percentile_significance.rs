//! Live-ClickHouse integration test for **percentile-metric significance**
//! (bead `feature-flag-nsh`).
//!
//! Seeds `experiment_assignments` + `events` for a 2-variant experiment with a
//! KNOWN upward percentile shift (treatment per-unit values are ~2× control),
//! runs [`run_stats_compute`] for a single `P90` aggregation metric on the
//! `user` context, and asserts the produced [`MetricSummary`]:
//!
//!   * carries a non-empty `frequentist_result` whose bootstrap difference CI
//!     is finite (the percentile `p_value` is NaN by design — significance is
//!     read from the CI / bayesian posterior),
//!   * carries a `bayesian_result` with a high `prob_best` for treatment, and
//!   * produces a real `recommendation` (NOT `NeedsMoreData`) — proving the raw
//!     `groupArray` sample fetch + bootstrap path replaced the prior
//!     point-only `NeedsMoreData` behaviour.
//!
//! Tagged `#[ignore]` so the default `cargo test` run needs no infrastructure.
//! Run explicitly with:
//!
//! ```sh
//! cargo test -p stitchd-stats-service --test percentile_significance -- --ignored --nocapture
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

/// Seed a 2-variant experiment whose per-unit metric value differs sharply
/// between arms: control unit `i` emits `value_double = 10 + (i % 20)`, while
/// treatment unit `i` emits `value_double = 30 + (i % 20)` — a clear upward
/// shift across the whole distribution (so the P90 also shifts up). Returns the
/// IDs + a `P90` aggregation metric over the canonical numeric value column.
struct Seeded {
    env_id: Uuid,
    exp_id: Uuid,
    iter_id: Uuid,
    metric: MetricDefinition,
    iter_end: chrono::DateTime<Utc>,
}

async fn seed_percentile_experiment(ch: &Client, n_per_variant: usize) -> Seeded {
    let env_id = Uuid::new_v4();
    let exp_id = Uuid::new_v4();
    let iter_id = Uuid::new_v4();
    let flag_id = Uuid::new_v4();
    let assigned_at = Utc.with_ymd_and_hms(2026, 5, 1, 0, 0, 0).unwrap();
    let iter_end = Utc.with_ymd_and_hms(2026, 5, 31, 0, 0, 0).unwrap();
    let event_at = assigned_at + Duration::hours(1);
    let metric_key = format!("latency_ms_{}", &exp_id.to_string()[..8]);

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

    // One value-bearing event per unit (post-exposure, in-window). The per-unit
    // metric value is the canonical `value_double`.
    let mut events = Vec::with_capacity(2 * n_per_variant);
    let mk_event = |key: String, value: f64| EventRow {
        env_id,
        contexts: vec![("user".into(), key)],
        metric_key: metric_key.clone(),
        value_bool: None,
        value_int: None,
        value_double: Some(value),
        timestamp: event_at,
        ingested_at: event_at,
        properties: vec![],
        occurred_at: event_at,
    };
    for i in 0..n_per_variant {
        events.push(mk_event(format!("c_{i}"), 10.0 + (i % 20) as f64));
        events.push(mk_event(format!("t_{i}"), 30.0 + (i % 20) as f64));
    }
    insert_events(ch, &events).await;

    let now = Utc::now();
    let metric = MetricDefinition {
        id: MetricId::new(),
        environment_id: EnvironmentId::from_uuid(env_id),
        key: metric_key.clone(),
        name: "latency_p90".into(),
        description: None,
        kind: MetricKind::Aggregation(AggregationConfig {
            event_key: metric_key,
            aggregator: AggregationOperator::P90,
            on_field: None, // canonical value_double / value_int
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

fn running_experiment(s: &Seeded) -> RunningExperiment {
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
            enabled: false,
            alpha: 0.05,
            tau_squared: None,
            min_sample_size: 100,
        },
    }
}

/// End-to-end: seed a clear P90 shift, run the compute pass, and assert the
/// percentile metric now produces real frequentist + bayesian blobs and a
/// non-`NeedsMoreData` recommendation (the bead's acceptance criterion).
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs running clickhouse"]
async fn percentile_metric_produces_real_significance() {
    let ch = make_client();
    stitchd_event_writer::migrations::run(&ch)
        .await
        .expect("apply CH migrations");

    let seeded = seed_percentile_experiment(&ch, 300).await;
    let exp = running_experiment(&seeded);
    let mut metrics: HashMap<Uuid, MetricDefinition> = HashMap::new();
    metrics.insert(seeded.metric.id.as_uuid(), seeded.metric.clone());

    let now = Utc::now();
    let reader = ClickHouseCellReader::new(Arc::new(ch.clone()));
    let summaries = run_stats_compute(&reader, &ch, &exp, &metrics, seeded.iter_end, now)
        .await
        .expect("compute pass should succeed");

    let summary = summaries
        .iter()
        .find(|s| s.metric_key == seeded.metric.key && s.context_type == "user")
        .expect("user percentile summary present");

    assert_eq!(summary.metric_type, "percentile");

    // Frequentist bootstrap-CI blob is present and keyed by the treatment.
    let freq = summary
        .frequentist_result
        .as_ref()
        .expect("percentile frequentist_result present (was None before nsh)");
    let treat_freq = &freq["treatment"];
    assert!(
        !treat_freq.is_null(),
        "treatment frequentist entry present: {freq}"
    );
    // The bootstrap percentile p_value is NaN by design → serialised as JSON
    // null. The difference CI must be finite + ordered, and (for a clear upward
    // shift) the lower bound should be positive.
    let ci_lower = treat_freq["confidence_interval"]["lower"]
        .as_f64()
        .expect("CI lower finite");
    let ci_upper = treat_freq["confidence_interval"]["upper"]
        .as_f64()
        .expect("CI upper finite");
    assert!(ci_lower.is_finite() && ci_upper.is_finite());
    assert!(ci_lower <= ci_upper, "CI ordered: {ci_lower}..{ci_upper}");
    assert!(
        ci_lower > 0.0,
        "treatment P90 is clearly higher → CI lower>0; got {ci_lower}..{ci_upper}"
    );

    // Bayesian bootstrap posterior: prob_best should be high for the shifted
    // treatment.
    let bayes = summary
        .bayesian_result
        .as_ref()
        .expect("percentile bayesian_result present");
    let prob_best = bayes["treatment"]["prob_best"]
        .as_f64()
        .expect("prob_best finite");
    assert!(
        prob_best > 0.95,
        "treatment P90 clearly higher → prob_best>0.95; got {prob_best}"
    );

    // The headline acceptance criterion: a REAL recommendation, not the old
    // point-only NeedsMoreData.
    assert_ne!(
        summary.recommendation, "needs_more_data",
        "percentile metric should now yield a real recommendation; got {}",
        summary.recommendation
    );

    // Surface the actual numbers the bead asked the report to capture.
    eprintln!(
        "[nsh] P90 percentile result: recommendation={} ci=[{ci_lower:.3}, {ci_upper:.3}] prob_best={prob_best:.4} freq_p_value={}",
        summary.recommendation, treat_freq["p_value"],
    );
}
