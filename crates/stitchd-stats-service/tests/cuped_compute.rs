//! Live-ClickHouse integration test for CUPED variance reduction in the numeric
//! compute pass.
//!
//! Seeds `experiment_assignments` + `events` for a 2-variant NUMERIC (sum)
//! experiment where each unit's pre-period covariate `X_pre` is strongly
//! correlated with its post-period value `Y`, then runs [`run_stats_compute`]
//! twice over the SAME data:
//!
//!   * once with `pre_period_days = 14` (CUPED ON), and
//!   * once with `pre_period_days = 0` (CUPED OFF).
//!
//! and asserts:
//!
//!   * the CUPED run surfaces `variant_stats["cuped"]` with
//!     `variance_reduction_pct > 0` (CUPED actually reduced variance), and
//!   * the CUPED run's frequentist confidence interval is **tighter** (narrower)
//!     than the no-CUPED run's — the downstream proof that the adjusted stats
//!     flow into the analyzers, and
//!   * the bayesian result is still produced on the adjusted stats.
//!
//! Tagged `#[ignore]` so the default `cargo test` run needs no infrastructure.
//! Run explicitly with:
//!
//! ```sh
//! DATABASE_URL=… cargo test -p stitchd-stats-service --test cuped_compute -- --ignored
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

struct Seeded {
    env_id: Uuid,
    exp_id: Uuid,
    iter_id: Uuid,
    metric: MetricDefinition,
    started_at: chrono::DateTime<Utc>,
    iter_end: chrono::DateTime<Utc>,
}

/// Seed a 2-variant numeric (sum-over-`value_double`) experiment where each
/// unit's pre-period covariate `X_pre` is strongly correlated with the
/// unit-level component of its post-period `Y`. Control and treatment differ by
/// a fixed lift so the frequentist contrast is well-defined.
///
/// CRUCIALLY, `X_pre` is a PRE-treatment covariate: it is drawn from the SAME
/// distribution for both arms (the treatment hasn't happened yet during the
/// pre-period), correlated only with the per-unit `spread_i` that also feeds
/// `Y`. Concretely:
///
/// ```text
///   Y_i     = base(arm) + spread_i        // base differs by arm (+20 lift)
///   X_pre_i = spread_i - jitter_i         // SAME baseline for both arms
/// ```
///
/// so the pooled CUPED θ ≈ 1 removes the `spread_i`-driven variance while
/// PRESERVING the between-arm `+20` difference (CUPED is mean-preserving and a
/// pre-treatment covariate carries no arm signal) → a large variance reduction
/// and a tighter CI around the true lift.
async fn seed_cuped_experiment(ch: &Client, n_per_variant: usize) -> Seeded {
    let env_id = Uuid::new_v4();
    let exp_id = Uuid::new_v4();
    let iter_id = Uuid::new_v4();
    let flag_id = Uuid::new_v4();

    let started_at = Utc.with_ymd_and_hms(2026, 5, 1, 0, 0, 0).unwrap();
    let iter_end = Utc.with_ymd_and_hms(2026, 5, 31, 0, 0, 0).unwrap();
    let assigned_at = started_at; // assignments land at iteration start
    let post_event_at = started_at + Duration::hours(1); // post-period (>= assigned_at)
    let pre_event_at = started_at - Duration::days(7); // inside the 14-day pre-window

    let metric_key = format!("revenue_{}", &exp_id.to_string()[..8]);

    // Assignments: N control + N treatment.
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
    let mut assignments = Vec::with_capacity(2 * n_per_variant);
    for i in 0..n_per_variant {
        assignments.push(mk_assignment("control", format!("c_{i}")));
        assignments.push(mk_assignment("treatment", format!("t_{i}")));
    }
    insert_assignments(ch, &assignments).await;

    // Events: one POST-period Y event and one PRE-period X_pre event per unit,
    // both carried on `value_double` (the canonical numeric column the
    // sum/avg aggregator + the CUPED fetch both read).
    let mk_event = |key: String, at: chrono::DateTime<Utc>, value: f64| EventRow {
        env_id,
        contexts: vec![("user".into(), key)],
        metric_key: metric_key.clone(),
        value_bool: None,
        value_int: None,
        value_double: Some(value),
        timestamp: at,
        ingested_at: at,
        properties: vec![],
        occurred_at: at,
    };

    let mut events = Vec::with_capacity(4 * n_per_variant);
    // A deterministic per-unit `spread` drives BOTH Y and X_pre, giving the
    // variant real variance that CUPED can strip. `base` differs by arm (+20
    // lift) but X_pre — a pre-treatment covariate — does NOT, so the lift
    // survives the pooled adjustment.
    for i in 0..n_per_variant {
        let spread = (i % 50) as f64; // 0..49 shared unit-level component
        let jitter = ((i % 7) as f64) * 0.1; // tiny decorrelation on X_pre

        // control: base 100, treatment: base 120 (a +20 lift).
        let y_c = 100.0 + spread;
        let y_t = 120.0 + spread;
        // X_pre tracks the SHARED spread only (same baseline for both arms).
        let x_pre = spread - jitter;

        events.push(mk_event(format!("c_{i}"), post_event_at, y_c));
        events.push(mk_event(format!("c_{i}"), pre_event_at, x_pre));
        events.push(mk_event(format!("t_{i}"), post_event_at, y_t));
        events.push(mk_event(format!("t_{i}"), pre_event_at, x_pre));
    }
    insert_events(ch, &events).await;

    let now = Utc::now();
    let metric = MetricDefinition {
        id: MetricId::new(),
        environment_id: EnvironmentId::from_uuid(env_id),
        key: metric_key.clone(),
        name: "revenue".into(),
        description: None,
        kind: MetricKind::Aggregation(AggregationConfig {
            event_key: metric_key, // count/sum reads event_key == seeded metric_key
            aggregator: AggregationOperator::Sum,
            // None on_field → the canonical value_double/value_int coalesce,
            // identical between Y (post) and X_pre (pre).
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
        started_at,
        iter_end,
    }
}

fn running_experiment(s: &Seeded, pre_period_days: u32) -> RunningExperiment {
    RunningExperiment {
        experiment_id: s.exp_id,
        env_id: s.env_id,
        iteration_id: s.iter_id,
        metric_ids: vec![s.metric.id.as_uuid()],
        variant_keys: vec!["control".into(), "treatment".into()],
        started_at: s.started_at,
        unit_context_types: vec!["user".into()],
        pre_period_days,
        sequential: SequentialSettings {
            enabled: false,
            alpha: 0.05,
            tau_squared: None,
            min_sample_size: 100,
        },
    }
}

/// Width of the treatment vs control frequentist confidence interval in a
/// summary's `frequentist_result` blob.
fn ci_width(summary: &stitchd_stats_service::results_writer::MetricSummary) -> f64 {
    let freq = summary
        .frequentist_result
        .as_ref()
        .expect("frequentist_result present");
    let ci = &freq["treatment"]["confidence_interval"];
    let lo = ci["lower"].as_f64().expect("ci lower");
    let hi = ci["upper"].as_f64().expect("ci upper");
    hi - lo
}

/// End-to-end: CUPED reduces variance and tightens the frequentist CI vs the
/// same compute pass with CUPED disabled.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs running clickhouse"]
async fn cuped_reduces_variance_and_tightens_ci() {
    let ch = make_client();
    stitchd_event_writer::migrations::run(&ch)
        .await
        .expect("apply CH migrations");

    let seeded = seed_cuped_experiment(&ch, 1000).await;
    let metrics: HashMap<Uuid, MetricDefinition> =
        [(seeded.metric.id.as_uuid(), seeded.metric.clone())]
            .into_iter()
            .collect();
    let reader = ClickHouseCellReader::new(Arc::new(ch.clone()));
    let now = seeded.iter_end + Duration::days(1);

    // ── CUPED ON (pre_period_days = 14) ──────────────────────────────────────
    let exp_cuped = running_experiment(&seeded, 14);
    let cuped_summaries =
        run_stats_compute(&reader, &ch, &exp_cuped, &metrics, seeded.iter_end, now)
            .await
            .expect("CUPED compute pass should succeed");
    assert_eq!(cuped_summaries.len(), 1, "one (metric, user) summary");
    let cuped = &cuped_summaries[0];
    assert_eq!(cuped.metric_type, "numeric");

    // variant_stats["cuped"] present with a positive variance_reduction_pct.
    let vstats = cuped
        .variant_stats
        .as_object()
        .expect("variant_stats object");
    let cuped_blob = vstats
        .get("cuped")
        .expect("variant_stats carries the cuped summary");
    let vrp = cuped_blob["variance_reduction_pct"]
        .as_f64()
        .expect("variance_reduction_pct");
    assert!(
        vrp > 0.0,
        "CUPED should reduce variance with a strongly correlated covariate; got {vrp}"
    );
    assert_eq!(
        cuped_blob["applied"],
        serde_json::json!(true),
        "applied should be true when variance was reduced"
    );

    // Frequentist + bayesian produced on the adjusted stats.
    assert!(cuped.frequentist_result.is_some(), "freq on adjusted stats");
    assert!(cuped.bayesian_result.is_some(), "bayes on adjusted stats");

    // ── CUPED OFF (pre_period_days = 0) ──────────────────────────────────────
    let exp_plain = running_experiment(&seeded, 0);
    let plain_summaries =
        run_stats_compute(&reader, &ch, &exp_plain, &metrics, seeded.iter_end, now)
            .await
            .expect("no-CUPED compute pass should succeed");
    let plain = &plain_summaries[0];
    // No CUPED path → no cuped blob.
    assert!(
        plain
            .variant_stats
            .as_object()
            .expect("object")
            .get("cuped")
            .is_none(),
        "no-CUPED run must not carry a cuped blob"
    );

    // ── The CUPED CI is tighter than the no-CUPED CI ─────────────────────────
    let cuped_w = ci_width(cuped);
    let plain_w = ci_width(plain);
    assert!(
        cuped_w < plain_w,
        "CUPED CI width ({cuped_w}) should be tighter than no-CUPED ({plain_w})"
    );

    // Both runs should still recover the ~+20 lift midpoint (CUPED preserves the
    // mean; it only tightens the interval).
    let mid = |s: &stitchd_stats_service::results_writer::MetricSummary| {
        let ci = &s.frequentist_result.as_ref().unwrap()["treatment"]["confidence_interval"];
        (ci["lower"].as_f64().unwrap() + ci["upper"].as_f64().unwrap()) / 2.0
    };
    assert!(
        (mid(cuped) - 20.0).abs() < 5.0,
        "CUPED lift midpoint should be ~20, got {}",
        mid(cuped)
    );

    println!(
        "cuped_compute observed: variance_reduction_pct={vrp:.2} \
         cuped_ci_width={cuped_w:.4} plain_ci_width={plain_w:.4} \
         theta={}",
        cuped_blob["theta"].as_f64().unwrap_or(f64::NAN)
    );
}
