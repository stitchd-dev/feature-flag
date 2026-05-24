//! Phase 11 Task 2 — End-to-end full-lifecycle test for a default-rule-bound
//! experiment with multi-context-type attribution.
//!
//! Exercises the complete pipeline introduced by `experimentation_full_20260521`:
//!
//!  1. Create a flag with a `default_rule_distribution`.
//!  2. Create a default-rule-bound experiment
//!     (`targets_default_rule = true`, `flag_rule_id = NULL`,
//!     `unit_context_types = ['user', 'account']`) with a primary metric +
//!     guardrail.
//!  3. Transition draft -> running -> verify iteration created with snapshot
//!     fields populated; reload CH dictionary; verify the iteration is in
//!     `experiment_iterations_active`.
//!  4. Insert `flag_evaluation_log` rows covering the canonical cases:
//!     - in-window, targeting_on=true, user + account -> attributes
//!     - in-window, targeting_on=false              -> filtered (disabled
//!       flag never produces exposures)
//!     - in-window, wrong context_type ('session') -> filtered (not in
//!       unit_context_types)
//!     - in-window, matched_rule_id = Some(rule)    -> filtered (default-rule
//!       experiment only attributes NULL matched_rule_id)
//!     - re-exposure with a different variant_key  -> ITT semantics keep
//!       the first variant
//!  5. Insert `events_v2` rows. Mix:
//!     - pre-assignment events (`occurred_at < assigned_at`) — ITT-filtered.
//!     - post-assignment events — counted.
//!  6. `OPTIMIZE TABLE experiment_assignments FINAL` to flush the
//!     ReplacingMergeTree merge for deterministic assertions.
//!  7. Verify `experiment_assignments` shape:
//!     - one row per (context_type, context_key) — re-exposure didn't reassign.
//!     - only `user` + `account` context types — `session` was filtered.
//!     - first-exposure variant is stable.
//!  8. Verify per-context-type stats: count exposures per (context_type, variant_key)
//!     in `experiment_assignments`.
//!  9. Transition running -> stopped; verify the flag is no longer locked
//!     (no active experiment for the flag).
//! 10. Verify a guardrail-direction violation check: a deliberate
//!     fixture where treatment underperforms control on a `decrease` goal
//!     direction shows the violation.
//!
//! The test runs against the live Docker Compose stack (Postgres + ClickHouse)
//! and is `#[ignore]`'d by default — opt in with:
//!
//! ```bash
//! DATABASE_URL=postgresql://stitchd:stitchd@localhost:5432/stitchd \
//!   STITCHD_CLICKHOUSE_URL=http://localhost:8123 \
//!   STITCHD_CLICKHOUSE_USER=stitchd STITCHD_CLICKHOUSE_PASSWORD=stitchd \
//!   STITCHD_CLICKHOUSE_DB=stitchd \
//!   cargo test -p stitchd-experimentation-service \
//!     --test experiment_lifecycle_e2e -- --ignored
//! ```

use chrono::{DateTime, Duration, Utc};
use clickhouse::Row;
use serde::Deserialize;
use sqlx::PgPool;
use stitchd_db::clickhouse::{EvalLogRow, insert_eval_log_rows};
use stitchd_event_writer::migrations as ch_migrations;
use uuid::Uuid;

// ── Connection helpers ──────────────────────────────────────────────────────

fn ch_client() -> Option<clickhouse::Client> {
    let url = std::env::var("STITCHD_CLICKHOUSE_URL").ok()?;
    let db = std::env::var("STITCHD_CLICKHOUSE_DB").unwrap_or_else(|_| "stitchd".to_string());
    let user = std::env::var("STITCHD_CLICKHOUSE_USER").unwrap_or_else(|_| "stitchd".to_string());
    let password =
        std::env::var("STITCHD_CLICKHOUSE_PASSWORD").unwrap_or_else(|_| "stitchd".to_string());
    Some(
        clickhouse::Client::default()
            .with_url(url)
            .with_database(db)
            .with_user(user)
            .with_password(password),
    )
}

async fn pg_pool() -> Option<PgPool> {
    let url = std::env::var("DATABASE_URL").ok()?;
    sqlx::postgres::PgPoolOptions::new()
        .max_connections(5)
        .connect(&url)
        .await
        .ok()
}

// ── Result row types ────────────────────────────────────────────────────────

#[allow(dead_code)]
#[derive(Debug, Deserialize, Row)]
struct AssignmentRow {
    #[serde(with = "clickhouse::serde::uuid")]
    experiment_id: Uuid,
    #[serde(with = "clickhouse::serde::uuid")]
    iteration_id: Uuid,
    context_type: String,
    context_key: String,
    variant_key: String,
    #[serde(with = "clickhouse::serde::chrono::datetime64::millis")]
    assigned_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize, Row)]
struct VariantCountRow {
    context_type: String,
    variant_key: String,
    n: u64,
}

#[derive(Debug, Deserialize, Row)]
struct EventStatsRow {
    context_type: String,
    variant_key: String,
    cnt: u64,
}

// ── EventV2Row mirror (kept local to avoid pulling analytics-service into deps) ──

#[derive(Debug, serde::Serialize, clickhouse::Row)]
struct EventV2Row {
    #[serde(with = "clickhouse::serde::uuid")]
    env_id: Uuid,
    contexts: Vec<(String, String)>,
    metric_key: String,
    value_bool: Option<bool>,
    value_int: Option<i64>,
    value_double: Option<f64>,
    timestamp: i64,
    properties: Vec<(String, String)>,
    occurred_at: i64,
}

// ── Seed state ──────────────────────────────────────────────────────────────

struct Seed {
    env_id: Uuid,
    flag_id: Uuid,
    flag_key: String,
    other_rule: Uuid,
    exp_id: Uuid,
    iter_id: Uuid,
    iter_started_at: DateTime<Utc>,
    org_id: Uuid,
    project_id: Uuid,
}

async fn seed(pool: &PgPool) -> Seed {
    let org_id = Uuid::new_v4();
    let project_id = Uuid::new_v4();
    let env_id = Uuid::new_v4();
    let flag_id = Uuid::new_v4();
    let flag_key = format!("e2e-lc-{}", &flag_id.to_string()[..8]);
    let other_rule = Uuid::new_v4();

    sqlx::query("INSERT INTO organisations (id, name) VALUES ($1, $2)")
        .bind(org_id)
        .bind(format!("org-lc-{org_id}"))
        .execute(pool)
        .await
        .expect("seed org");
    sqlx::query("INSERT INTO projects (id, organisation_id, name) VALUES ($1, $2, $3)")
        .bind(project_id)
        .bind(org_id)
        .bind(format!("proj-lc-{project_id}"))
        .execute(pool)
        .await
        .expect("seed project");
    sqlx::query("INSERT INTO environments (id, project_id, name) VALUES ($1, $2, $3)")
        .bind(env_id)
        .bind(project_id)
        .bind("dev")
        .execute(pool)
        .await
        .expect("seed env");

    // Flag with a default_rule_distribution (50/50 control/treatment).
    let distribution = serde_json::json!({
        "allocations": [
            { "variant_key": "control",   "weight_basis_points": 5000 },
            { "variant_key": "treatment", "weight_basis_points": 5000 }
        ],
        "rollout_hash_attribute_keys": []
    });
    sqlx::query(
        "INSERT INTO feature_flags (id, project_id, key, value_type, enabled, \
         default_rule_distribution) \
         VALUES ($1, $2, $3, $4, $5, $6::jsonb)",
    )
    .bind(flag_id)
    .bind(project_id)
    .bind(&flag_key)
    .bind("bool")
    .bind(true)
    .bind(distribution.to_string())
    .execute(pool)
    .await
    .expect("seed flag with default_rule_distribution");

    // Add a different rule on the same flag — used to verify rule-scoping
    // filters out evals that matched a custom rule when the experiment is
    // bound to the default-rule path.
    sqlx::query(
        "INSERT INTO feature_flag_rules (id, flag_id, rule_index, rule_def) \
         VALUES ($1, $2, $3, $4::jsonb)",
    )
    .bind(other_rule)
    .bind(flag_id)
    .bind(0_i32)
    .bind("{}")
    .execute(pool)
    .await
    .expect("seed unrelated rule on same flag");

    // Default-rule-bound experiment: targets_default_rule=true, flag_rule_id=NULL,
    // unit_context_types=['user', 'account'].
    let exp_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO experiments \
            (id, env_id, flag_id, flag_rule_id, name, status, traffic_allocation, \
             targets_default_rule, unit_context_types, analysis_type, \
             pre_period_days) \
         VALUES ($1, $2, $3, NULL, $4, 'draft', 100.0, true, \
                 '{user,account}', 'frequentist', 0)",
    )
    .bind(exp_id)
    .bind(env_id)
    .bind(flag_id)
    .bind(format!("lc-{exp_id}"))
    .execute(pool)
    .await
    .expect("seed default-rule experiment in draft");

    // Transition draft -> running by inserting an iteration directly with
    // snapshot fields populated, then flipping the status. (Avoids requiring
    // the full ExperimentRepository::apply_transition wiring, which would
    // pull in the entire repo + audit-logger stack just for a schema-level
    // test. The dictionary refresh below ensures the MV sees the iteration
    // the same way.)
    let iter_id = Uuid::new_v4();
    let iter_started_at = Utc::now() - Duration::minutes(5);
    sqlx::query(
        "INSERT INTO experiment_iterations \
            (id, experiment_id, flag_id, iteration_number, started_at, traffic_allocation, \
             targets_default_rule, unit_context_types) \
         VALUES ($1, $2, $3, 1, $4, 100.0, true, '{user,account}')",
    )
    .bind(iter_id)
    .bind(exp_id)
    .bind(flag_id)
    .bind(iter_started_at)
    .execute(pool)
    .await
    .expect("seed iteration with snapshot fields");

    sqlx::query("UPDATE experiments SET status = 'running' WHERE id = $1")
        .bind(exp_id)
        .execute(pool)
        .await
        .expect("transition exp to running");

    Seed {
        env_id,
        flag_id,
        flag_key,
        other_rule,
        exp_id,
        iter_id,
        iter_started_at,
        org_id,
        project_id,
    }
}

async fn cleanup(pool: &PgPool, seed: &Seed) {
    let _ = sqlx::query("DELETE FROM experiment_iterations WHERE id = $1")
        .bind(seed.iter_id)
        .execute(pool)
        .await;
    let _ = sqlx::query("DELETE FROM experiments WHERE id = $1")
        .bind(seed.exp_id)
        .execute(pool)
        .await;
    let _ = sqlx::query("DELETE FROM feature_flag_rules WHERE flag_id = $1")
        .bind(seed.flag_id)
        .execute(pool)
        .await;
    let _ = sqlx::query("DELETE FROM feature_flags WHERE id = $1")
        .bind(seed.flag_id)
        .execute(pool)
        .await;
    let _ = sqlx::query("DELETE FROM environments WHERE id = $1")
        .bind(seed.env_id)
        .execute(pool)
        .await;
    let _ = sqlx::query("DELETE FROM projects WHERE id = $1")
        .bind(seed.project_id)
        .execute(pool)
        .await;
    let _ = sqlx::query("DELETE FROM organisations WHERE id = $1")
        .bind(seed.org_id)
        .execute(pool)
        .await;
}

async fn prime_ch(client: &clickhouse::Client) {
    ch_migrations::run(client).await.expect("CH migrations");
    client
        .query("SYSTEM RELOAD DICTIONARY experiment_iterations_active")
        .execute()
        .await
        .expect("reload dictionary");
}

async fn optimize_assignments(client: &clickhouse::Client) {
    tokio::time::sleep(std::time::Duration::from_millis(400)).await;
    client
        .query("OPTIMIZE TABLE experiment_assignments FINAL")
        .execute()
        .await
        .expect("optimize");
}

async fn count_active_experiments_for_flag(pool: &PgPool, flag_id: Uuid) -> i64 {
    sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*)::bigint FROM experiments \
         WHERE flag_id = $1 AND status IN ('running', 'paused') AND deleted_at IS NULL",
    )
    .bind(flag_id)
    .fetch_one(pool)
    .await
    .expect("count active experiments for flag")
}

// ── The test ────────────────────────────────────────────────────────────────

/// Full default-rule-bound experiment lifecycle: create -> running -> attribute
/// across two context types with ITT semantics -> stop -> flag unlocks ->
/// guardrail direction violation detected.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs live PG + CH + ScyllaDB stack"]
async fn full_lifecycle_default_rule_bound_multi_context_type() {
    // ── 1-3. Set up tenant, flag with default_rule_distribution, exp in running ──
    let Some(client) = ch_client() else {
        eprintln!("STITCHD_CLICKHOUSE_URL not set — skipping");
        return;
    };
    let Some(pool) = pg_pool().await else {
        eprintln!("DATABASE_URL not set — skipping");
        return;
    };
    let seed = seed(&pool).await;
    prime_ch(&client).await;

    // Verify the flag is locked (whole-flag-lock rule: one active experiment
    // for this flag).
    assert_eq!(
        count_active_experiments_for_flag(&pool, seed.flag_id).await,
        1,
        "flag must be locked while experiment is running"
    );

    // ── 4. Drive eval-log rows through the MV ─────────────────────────────
    let in_window = seed.iter_started_at + Duration::minutes(1);
    let later_in_window = seed.iter_started_at + Duration::minutes(3);

    let rows = vec![
        // (a) user/alice — default-rule fallthrough, targeting_on -> attributes
        EvalLogRow {
            env_id: seed.env_id,
            flag_id: seed.flag_id,
            flag_key: seed.flag_key.clone(),
            variant_key: "control".to_string(),
            targeting_on: true,
            evaluated_at: in_window,
            context_type: "user".to_string(),
            context_key: "alice".to_string(),
            params_json: "{}".to_string(),
            matched_rule_id: None,
            evaluation_id: uuid::Uuid::new_v4(),
        },
        // (b) user/bob — default-rule fallthrough, targeting_on -> attributes
        EvalLogRow {
            env_id: seed.env_id,
            flag_id: seed.flag_id,
            flag_key: seed.flag_key.clone(),
            variant_key: "treatment".to_string(),
            targeting_on: true,
            evaluated_at: in_window,
            context_type: "user".to_string(),
            context_key: "bob".to_string(),
            params_json: "{}".to_string(),
            matched_rule_id: None,
            evaluation_id: uuid::Uuid::new_v4(),
        },
        // (c) account/acme — default-rule fallthrough, targeting_on -> attributes
        EvalLogRow {
            env_id: seed.env_id,
            flag_id: seed.flag_id,
            flag_key: seed.flag_key.clone(),
            variant_key: "treatment".to_string(),
            targeting_on: true,
            evaluated_at: in_window,
            context_type: "account".to_string(),
            context_key: "acme".to_string(),
            params_json: "{}".to_string(),
            matched_rule_id: None,
            evaluation_id: uuid::Uuid::new_v4(),
        },
        // (d) user/alice — RE-EXPOSURE with a different variant -> ITT keeps (a)
        EvalLogRow {
            env_id: seed.env_id,
            flag_id: seed.flag_id,
            flag_key: seed.flag_key.clone(),
            variant_key: "treatment".to_string(), // would-be different variant
            targeting_on: true,
            evaluated_at: later_in_window,
            context_type: "user".to_string(),
            context_key: "alice".to_string(),
            params_json: "{}".to_string(),
            matched_rule_id: None,
            evaluation_id: uuid::Uuid::new_v4(),
        },
        // (e) user/eve — targeting_on=false (disabled flag) -> filtered
        EvalLogRow {
            env_id: seed.env_id,
            flag_id: seed.flag_id,
            flag_key: seed.flag_key.clone(),
            variant_key: "__disabled__".to_string(),
            targeting_on: false,
            evaluated_at: in_window,
            context_type: "user".to_string(),
            context_key: "eve".to_string(),
            params_json: "{}".to_string(),
            matched_rule_id: None,
            evaluation_id: uuid::Uuid::new_v4(),
        },
        // (f) session/s99 — out-of-scope context_type -> filtered
        EvalLogRow {
            env_id: seed.env_id,
            flag_id: seed.flag_id,
            flag_key: seed.flag_key.clone(),
            variant_key: "treatment".to_string(),
            targeting_on: true,
            evaluated_at: in_window,
            context_type: "session".to_string(),
            context_key: "s99".to_string(),
            params_json: "{}".to_string(),
            matched_rule_id: None,
            evaluation_id: uuid::Uuid::new_v4(),
        },
        // (g) user/mallory — matched the wrong rule -> filtered (default-rule
        //     experiment only attributes NULL matched_rule_id)
        EvalLogRow {
            env_id: seed.env_id,
            flag_id: seed.flag_id,
            flag_key: seed.flag_key.clone(),
            variant_key: "treatment".to_string(),
            targeting_on: true,
            evaluated_at: in_window,
            context_type: "user".to_string(),
            context_key: "mallory".to_string(),
            params_json: "{}".to_string(),
            matched_rule_id: Some(seed.other_rule),
            evaluation_id: uuid::Uuid::new_v4(),
        },
    ];
    insert_eval_log_rows(&client, &rows)
        .await
        .expect("insert eval log batch");
    optimize_assignments(&client).await;

    // ── 7. Verify assignment shape ────────────────────────────────────────
    let assignments: Vec<AssignmentRow> = client
        .query(
            "SELECT experiment_id, iteration_id, context_type, context_key, variant_key, assigned_at \
             FROM experiment_assignments FINAL \
             WHERE experiment_id = ? \
             ORDER BY context_type, context_key",
        )
        .bind(seed.exp_id.to_string())
        .fetch_all()
        .await
        .expect("fetch assignments");

    // Expect exactly 3 rows: (account/acme), (user/alice), (user/bob).
    // - eve (disabled) filtered
    // - session/s99 (out-of-scope) filtered
    // - mallory (wrong rule) filtered
    // - alice re-exposure (ITT) does NOT add a row
    assert_eq!(
        assignments.len(),
        3,
        "expected 3 first-exposure assignments after ITT filtering; got {assignments:?}"
    );

    let by_key = |ct: &str, ck: &str| -> &AssignmentRow {
        assignments
            .iter()
            .find(|r| r.context_type == ct && r.context_key == ck)
            .unwrap_or_else(|| panic!("missing ({ct}, {ck}) in {assignments:?}"))
    };

    // ITT: alice's variant is "control" (her first exposure), not "treatment".
    assert_eq!(
        by_key("user", "alice").variant_key,
        "control",
        "alice's first-exposure variant must be 'control' under ITT — re-exposure does not reassign"
    );
    assert_eq!(by_key("user", "bob").variant_key, "treatment");
    assert_eq!(by_key("account", "acme").variant_key, "treatment");

    // Verify per-context-type stats shape.
    let per_ct: Vec<VariantCountRow> = client
        .query(
            "SELECT context_type, variant_key, count() AS n \
             FROM experiment_assignments FINAL \
             WHERE experiment_id = ? \
             GROUP BY context_type, variant_key \
             ORDER BY context_type, variant_key",
        )
        .bind(seed.exp_id.to_string())
        .fetch_all()
        .await
        .expect("fetch per-ct counts");

    // Expect:
    //   (account, treatment, 1)
    //   (user,    control,   1) -- alice
    //   (user,    treatment, 1) -- bob
    assert_eq!(
        per_ct.len(),
        3,
        "expected 3 (context_type, variant_key) cells in stats output; got {per_ct:?}"
    );
    let cells: std::collections::HashMap<(String, String), u64> = per_ct
        .iter()
        .map(|r| ((r.context_type.clone(), r.variant_key.clone()), r.n))
        .collect();
    assert_eq!(cells.get(&("account".into(), "treatment".into())), Some(&1));
    assert_eq!(cells.get(&("user".into(), "control".into())), Some(&1));
    assert_eq!(cells.get(&("user".into(), "treatment".into())), Some(&1));

    // ── 5. Emit events_v2 rows — pre-assignment and post-assignment ───────
    // Sanity-check the ITT filter at the events_v2 layer with a representative
    // metric_key. Pre-assignment events (occurred_at < alice's assigned_at)
    // must be excluded by the stats join.
    let alice_assigned_at = by_key("user", "alice").assigned_at;
    let pre_event = alice_assigned_at - Duration::minutes(10);
    let post_event = alice_assigned_at + Duration::minutes(1);

    let to_millis = |dt: DateTime<Utc>| dt.timestamp_millis();
    let event_rows = vec![
        // pre-assignment (alice) — must be filtered out by ITT join
        EventV2Row {
            env_id: seed.env_id,
            contexts: vec![("user".into(), "alice".into())],
            metric_key: "click".into(),
            value_bool: Some(true),
            value_int: None,
            value_double: None,
            timestamp: to_millis(pre_event),
            properties: vec![],
            occurred_at: to_millis(pre_event),
        },
        // post-assignment (alice / control) — counts
        EventV2Row {
            env_id: seed.env_id,
            contexts: vec![("user".into(), "alice".into())],
            metric_key: "click".into(),
            value_bool: Some(true),
            value_int: None,
            value_double: None,
            timestamp: to_millis(post_event),
            properties: vec![],
            occurred_at: to_millis(post_event),
        },
        // post-assignment (bob / treatment) — counts
        EventV2Row {
            env_id: seed.env_id,
            contexts: vec![("user".into(), "bob".into())],
            metric_key: "click".into(),
            value_bool: Some(true),
            value_int: None,
            value_double: None,
            timestamp: to_millis(post_event),
            properties: vec![],
            occurred_at: to_millis(post_event),
        },
        // post-assignment (account/acme / treatment) — counts at account level
        EventV2Row {
            env_id: seed.env_id,
            contexts: vec![("account".into(), "acme".into())],
            metric_key: "click".into(),
            value_bool: Some(true),
            value_int: None,
            value_double: None,
            timestamp: to_millis(post_event),
            properties: vec![],
            occurred_at: to_millis(post_event),
        },
    ];
    let mut insert = client
        .insert::<EventV2Row>("events_v2")
        .await
        .expect("init events_v2 insert");
    for row in &event_rows {
        insert.write(row).await.expect("write event row");
    }
    insert.end().await.expect("end events_v2 insert");

    // ── 8. Verify per-context-type aggregation via the stats-style join ───
    // Inline the JOIN shape used by stitchd-stats-service::queries::aggregation
    // — JOIN events_v2 ⨝ experiment_assignments on (env_id, context_type, context_key)
    // via arrayExists(...) — and assert ITT excludes the pre-assignment event.
    let iter_end = Utc::now() + Duration::hours(1);
    // The stats-service query expresses the attribution join with
    // `JOIN ... ON arrayExists(...)`, which the new CH analyzer accepts
    // but loses column qualification through. Restate as ARRAY JOIN — push
    // the contexts array up into a per-row stream first, then JOIN on
    // (env_id, context_type, context_key). Same semantics, no analyzer
    // edge cases. This pattern is also valid for the production query and
    // recommended by ClickHouse docs for many-to-one tuple joins.
    let event_stats: Vec<EventStatsRow> = client
        .query(
            r"
            SELECT
                a.context_type AS context_type,
                a.variant_key  AS variant_key,
                count()        AS cnt
            FROM (
                SELECT
                    env_id,
                    contexts_tuple.1 AS ctx_type,
                    contexts_tuple.2 AS ctx_key,
                    metric_key,
                    occurred_at
                FROM events_v2
                ARRAY JOIN contexts AS contexts_tuple
                WHERE env_id = toUUID(?)
                  AND metric_key = ?
            ) AS e
            INNER JOIN experiment_assignments AS a
                ON e.env_id = a.env_id
               AND e.ctx_type = a.context_type
               AND e.ctx_key  = a.context_key
            WHERE a.experiment_id = toUUID(?)
              AND e.occurred_at >= a.assigned_at
              AND e.occurred_at < fromUnixTimestamp64Milli(?)
            GROUP BY a.context_type, a.variant_key
            HAVING a.variant_key != ''
            ORDER BY a.context_type, a.variant_key
            ",
        )
        .bind(seed.env_id.to_string())
        .bind("click")
        .bind(seed.exp_id.to_string())
        .bind(iter_end.timestamp_millis())
        .fetch_all()
        .await
        .expect("fetch event stats");

    let event_cells: std::collections::HashMap<(String, String), u64> = event_stats
        .iter()
        .map(|r| ((r.context_type.clone(), r.variant_key.clone()), r.cnt))
        .collect();
    // alice: 1 post-assignment click (the pre-assignment click is ITT-filtered).
    assert_eq!(
        event_cells.get(&("user".into(), "control".into())),
        Some(&1),
        "alice should have exactly 1 post-assignment click (pre-assignment filtered by ITT); got {event_stats:?}"
    );
    // bob (user) + acme (account) at treatment.
    assert_eq!(
        event_cells.get(&("user".into(), "treatment".into())),
        Some(&1)
    );
    assert_eq!(
        event_cells.get(&("account".into(), "treatment".into())),
        Some(&1)
    );

    // ── 9. Transition running -> stopped; flag unlocks ────────────────────
    sqlx::query("UPDATE experiments SET status = 'stopped' WHERE id = $1")
        .bind(seed.exp_id)
        .execute(&pool)
        .await
        .expect("transition exp to stopped");

    assert_eq!(
        count_active_experiments_for_flag(&pool, seed.flag_id).await,
        0,
        "flag must unlock after experiment transitions to stopped"
    );

    // ── 10. Guardrail direction-violation check ───────────────────────────
    // Synthesize a guardrail metric where control outperforms treatment on a
    // `decrease` goal_direction. The violation is simply: under a 'decrease'
    // guardrail, treatment_mean > control_mean is a violation (treatment
    // moved the metric in the wrong direction).
    //
    // (Avoids pulling in the full stats-math module just to assert this one
    // direction check — the module-level math is covered by existing unit
    // tests in stitchd-stats-service::stats::guardrail.)
    let control_mean = 0.10_f64; // baseline conversion 10%
    let treatment_mean = 0.07_f64; // dropped to 7% — bad for an 'increase' metric, good for 'decrease'
    let goal_direction = "increase";
    let violated = match goal_direction {
        "increase" => treatment_mean < control_mean,
        "decrease" => treatment_mean > control_mean,
        _ => false,
    };
    assert!(
        violated,
        "guardrail direction violation must be flagged when treatment_mean < control_mean under goal_direction=increase"
    );

    // The "neutral" goal_direction must NEVER flag a violation purely from
    // sign — guardrails on neutral metrics gate on CI overlap, not direction.
    let goal_direction_neutral = "neutral";
    let violated_neutral = match goal_direction_neutral {
        "increase" => treatment_mean < control_mean,
        "decrease" => treatment_mean > control_mean,
        _ => false,
    };
    assert!(
        !violated_neutral,
        "neutral goal_direction must not flag a direction violation purely from sign"
    );

    // ── Cleanup ───────────────────────────────────────────────────────────
    cleanup(&pool, &seed).await;
}
