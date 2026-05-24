//! Integration test for `experiment_assignments_mv` — Phase 4 Task 2.
//!
//! Drives the materialized view end-to-end:
//!   1. Seed PG: env + flag + rule + running rule-bound experiment + iteration.
//!   2. Reload `experiment_iterations_active` dictionary so it sees the seed.
//!   3. Insert eval-log rows into `flag_evaluation_log` (via the production
//!      `insert_eval_log_rows` writer).
//!   4. `OPTIMIZE TABLE experiment_assignments FINAL` to collapse the
//!      ReplacingMergeTree, then SELECT to assert the right
//!      `(experiment_id, iteration_id, context_type, context_key, variant_key,
//!        assigned_at)` rows appear.
//!
//! Coverage:
//!   * Happy path — eval-log row matching the running iteration produces
//!     one assignment row.
//!   * First-exposure dedup — re-exposure with a DIFFERENT variant does NOT
//!     reassign (the _version inversion keeps the earliest row).
//!   * `targeting_on = false` rows are filtered out by the MV.
//!   * Eval rows targeted at an unrelated env/flag/rule do not produce
//!     assignments (dictHas-filter side).

use chrono::{DateTime, Utc};
use clickhouse::Row;
use serde::Deserialize;
use sqlx::PgPool;
use stitchd_db::clickhouse::{EvalLogRow, insert_eval_log_rows};
use stitchd_event_writer::migrations as ch_migrations;
use uuid::Uuid;

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

#[allow(dead_code)]
#[derive(Debug, Deserialize, Row)]
pub struct AssignmentRowRead {
    #[serde(with = "clickhouse::serde::uuid")]
    pub experiment_id: Uuid,
    #[serde(with = "clickhouse::serde::uuid")]
    pub iteration_id: Uuid,
    pub context_type: String,
    pub context_key: String,
    pub variant_key: String,
}

pub struct Seed {
    pub env_id: Uuid,
    pub flag_id: Uuid,
    pub rule_a: Uuid,
    pub experiment_id: Uuid,
    pub iteration_id: Uuid,
    pub org_id: Uuid,
    pub project_id: Uuid,
}

pub async fn seed_rule_bound(pool: &PgPool, ucts: &[&str]) -> Seed {
    let org_id = Uuid::new_v4();
    let project_id = Uuid::new_v4();
    let env_id = Uuid::new_v4();
    let flag_id = Uuid::new_v4();
    let rule_a = Uuid::new_v4();

    sqlx::query("INSERT INTO organisations (id, name) VALUES ($1, $2)")
        .bind(org_id)
        .bind(format!("org-mv-{org_id}"))
        .execute(pool)
        .await
        .expect("seed org");
    sqlx::query("INSERT INTO projects (id, organisation_id, name) VALUES ($1, $2, $3)")
        .bind(project_id)
        .bind(org_id)
        .bind(format!("proj-mv-{project_id}"))
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
    sqlx::query(
        "INSERT INTO feature_flags (id, project_id, key, value_type, enabled) \
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(flag_id)
    .bind(project_id)
    .bind(format!("flag-mv-{}", &flag_id.to_string()[..8]))
    .bind("bool")
    .bind(true)
    .execute(pool)
    .await
    .expect("seed flag");
    sqlx::query(
        "INSERT INTO feature_flag_rules (id, flag_id, rule_index, rule_def) \
         VALUES ($1, $2, $3, $4::jsonb)",
    )
    .bind(rule_a)
    .bind(flag_id)
    .bind(0_i32)
    .bind("{}")
    .execute(pool)
    .await
    .expect("seed rule");

    let experiment_id = Uuid::new_v4();
    let iteration_id = Uuid::new_v4();
    let ucts_owned: Vec<String> = ucts.iter().map(|s| (*s).to_string()).collect();

    sqlx::query(
        "INSERT INTO experiments \
            (id, env_id, flag_id, flag_rule_id, name, status, traffic_allocation, \
             targets_default_rule, unit_context_types, analysis_type) \
         VALUES ($1, $2, $3, $4, $5, 'running', 100.0, false, $6, 'frequentist')",
    )
    .bind(experiment_id)
    .bind(env_id)
    .bind(flag_id)
    .bind(rule_a)
    .bind(format!("mv-exp-{experiment_id}"))
    .bind(&ucts_owned)
    .execute(pool)
    .await
    .expect("seed experiment");

    sqlx::query(
        "INSERT INTO experiment_iterations \
            (id, experiment_id, flag_id, iteration_number, started_at, traffic_allocation, \
             targets_default_rule, unit_context_types) \
         VALUES ($1, $2, $3, 1, $4, 100.0, false, $5)",
    )
    .bind(iteration_id)
    .bind(experiment_id)
    .bind(flag_id)
    .bind(Utc::now())
    .bind(&ucts_owned)
    .execute(pool)
    .await
    .expect("seed iteration");

    Seed {
        env_id,
        flag_id,
        rule_a,
        experiment_id,
        iteration_id,
        org_id,
        project_id,
    }
}

pub async fn cleanup(pool: &PgPool, seed: &Seed) {
    let _ = sqlx::query("DELETE FROM experiment_iterations WHERE id = $1")
        .bind(seed.iteration_id)
        .execute(pool)
        .await;
    let _ = sqlx::query("DELETE FROM experiments WHERE id = $1")
        .bind(seed.experiment_id)
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

pub async fn prime_ch(client: &clickhouse::Client) {
    ch_migrations::run(client).await.expect("CH migrations");
    client
        .query("SYSTEM RELOAD DICTIONARY experiment_iterations_active")
        .execute()
        .await
        .expect("reload dictionary");
}

pub async fn settle_and_optimize(client: &clickhouse::Client) {
    // Give the MV a moment to flush.
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    client
        .query("OPTIMIZE TABLE experiment_assignments FINAL")
        .execute()
        .await
        .expect("optimize assignments");
}

pub async fn fetch_assignments(
    client: &clickhouse::Client,
    experiment_id: Uuid,
) -> Vec<AssignmentRowRead> {
    client
        .query(
            "SELECT experiment_id, iteration_id, context_type, context_key, variant_key \
             FROM experiment_assignments \
             FINAL \
             WHERE experiment_id = ? \
             ORDER BY context_key, variant_key",
        )
        .bind(experiment_id.to_string())
        .fetch_all()
        .await
        .expect("fetch assignments")
}

fn eval_row(
    seed: &Seed,
    context_key: &str,
    variant: &str,
    targeting_on: bool,
    matched_rule_id: Option<Uuid>,
    when: DateTime<Utc>,
) -> EvalLogRow {
    EvalLogRow {
        env_id: seed.env_id,
        flag_id: seed.flag_id,
        flag_key: format!("flag-mv-{}", &seed.flag_id.to_string()[..8]),
        variant_key: variant.to_string(),
        targeting_on,
        evaluated_at: when,
        context_type: "user".to_string(),
        context_key: context_key.to_string(),
        params_json: "{}".to_string(),
        matched_rule_id,
        evaluation_id: uuid::Uuid::new_v4(),
    }
}

/// Happy path: an in-scope eval row creates an assignment row.
#[tokio::test]
async fn happy_path_eval_row_becomes_assignment() {
    let Some(client) = ch_client() else {
        eprintln!("STITCHD_CLICKHOUSE_URL not set — skipping");
        return;
    };
    let Some(pool) = pg_pool().await else {
        return;
    };
    let seed = seed_rule_bound(&pool, &["user"]).await;
    prime_ch(&client).await;

    let t0 = Utc::now();
    let rows = vec![eval_row(
        &seed,
        "alice",
        "treatment",
        true,
        Some(seed.rule_a),
        t0,
    )];
    insert_eval_log_rows(&client, &rows)
        .await
        .expect("insert eval log");
    settle_and_optimize(&client).await;

    let assignments = fetch_assignments(&client, seed.experiment_id).await;
    cleanup(&pool, &seed).await;

    assert_eq!(assignments.len(), 1, "expected exactly one assignment row");
    assert_eq!(assignments[0].experiment_id, seed.experiment_id);
    assert_eq!(assignments[0].iteration_id, seed.iteration_id);
    assert_eq!(assignments[0].context_type, "user");
    assert_eq!(assignments[0].context_key, "alice");
    assert_eq!(assignments[0].variant_key, "treatment");
}

/// First-exposure semantics: a later eval with a DIFFERENT variant does
/// NOT reassign the context within the iteration. The ReplacingMergeTree
/// `_version = -toUnixTimestamp64Milli(evaluated_at)` keeps the earliest.
#[tokio::test]
async fn first_exposure_not_reassigned_by_later_variant_change() {
    let Some(client) = ch_client() else {
        eprintln!("STITCHD_CLICKHOUSE_URL not set — skipping");
        return;
    };
    let Some(pool) = pg_pool().await else {
        return;
    };
    let seed = seed_rule_bound(&pool, &["user"]).await;
    prime_ch(&client).await;

    let t0 = Utc::now();
    let t1 = t0 + chrono::Duration::milliseconds(500);
    let rows = vec![
        eval_row(&seed, "bob", "control", true, Some(seed.rule_a), t0),
        eval_row(&seed, "bob", "treatment", true, Some(seed.rule_a), t1),
    ];
    insert_eval_log_rows(&client, &rows)
        .await
        .expect("insert eval log");
    settle_and_optimize(&client).await;

    let assignments = fetch_assignments(&client, seed.experiment_id).await;
    cleanup(&pool, &seed).await;

    assert_eq!(
        assignments.len(),
        1,
        "ReplacingMergeTree FINAL must collapse to one row per (exp, iter, ctype, ckey); got {assignments:?}"
    );
    assert_eq!(
        assignments[0].variant_key, "control",
        "first-exposure semantics: earliest variant wins, not the later 'treatment'"
    );
}

/// `targeting_on = false` rows are filtered out by the MV's WHERE clause.
#[tokio::test]
async fn targeting_off_rows_are_filtered() {
    let Some(client) = ch_client() else {
        eprintln!("STITCHD_CLICKHOUSE_URL not set — skipping");
        return;
    };
    let Some(pool) = pg_pool().await else {
        return;
    };
    let seed = seed_rule_bound(&pool, &["user"]).await;
    prime_ch(&client).await;

    let t0 = Utc::now();
    let rows = vec![eval_row(
        &seed,
        "carol",
        "treatment",
        false, // targeting_on = false
        Some(seed.rule_a),
        t0,
    )];
    insert_eval_log_rows(&client, &rows)
        .await
        .expect("insert eval log");
    settle_and_optimize(&client).await;

    let assignments = fetch_assignments(&client, seed.experiment_id).await;
    cleanup(&pool, &seed).await;

    assert_eq!(
        assignments.len(),
        0,
        "targeting_on=false rows must not become assignments"
    );
}

/// Eval rows on an unrelated env/flag (no matching active iteration) do
/// not create assignments. Exercises the `dictHas` filter.
#[tokio::test]
async fn unrelated_eval_rows_filtered_by_dict_has() {
    let Some(client) = ch_client() else {
        eprintln!("STITCHD_CLICKHOUSE_URL not set — skipping");
        return;
    };
    let Some(pool) = pg_pool().await else {
        return;
    };
    let seed = seed_rule_bound(&pool, &["user"]).await;
    prime_ch(&client).await;

    let t0 = Utc::now();
    // Use a totally unrelated env_id + flag_id + rule_id triple — dictHas
    // returns 0 so the MV WHERE drops the row before it tries to dictGet
    // (which would panic on a missing key).
    let stray = EvalLogRow {
        env_id: Uuid::new_v4(),
        flag_id: Uuid::new_v4(),
        flag_key: "unrelated".to_string(),
        variant_key: "stray".to_string(),
        targeting_on: true,
        evaluated_at: t0,
        context_type: "user".to_string(),
        context_key: "ghost".to_string(),
        params_json: "{}".to_string(),
        matched_rule_id: Some(Uuid::new_v4()),
        evaluation_id: Uuid::new_v4(),
    };
    insert_eval_log_rows(&client, &[stray])
        .await
        .expect("insert eval log");
    settle_and_optimize(&client).await;

    let assignments = fetch_assignments(&client, seed.experiment_id).await;
    cleanup(&pool, &seed).await;

    assert_eq!(
        assignments.len(),
        0,
        "unrelated eval rows must not pollute the assignment table"
    );
}
