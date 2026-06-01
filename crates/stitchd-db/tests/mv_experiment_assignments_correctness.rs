//! Correctness tests for `experiment_assignments_mv` filtering — Phase 4
//! Task 4 (rule-scoping + context-type-scoping + default-rule-bound).
//!
//! Three failure modes the live MV must reject by design (per spec §1):
//!
//!   * **Different rule on same flag.** Eval matches rule B; experiment is
//!     bound to rule A → no assignment.
//!   * **Out-of-scope context type.** Experiment's `unit_context_types =
//!     ['user']`; eval row has `context_type='account'` → no assignment.
//!   * **Default-rule-bound experiment.** `targets_default_rule=true`,
//!     `flag_rule_id IS NULL`; eval fell through with `matched_rule_id IS
//!     NULL` → assignment created.
//!
//! Lives separately from `mv_experiment_assignments.rs` so the two files
//! can run in parallel without sharing a global PG state. Each test seeds
//! its own UUIDs and cleans up.

use chrono::Utc;
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
struct AssignmentSummary {
    #[serde(with = "clickhouse::serde::uuid")]
    experiment_id: Uuid,
    #[serde(with = "clickhouse::serde::uuid")]
    iteration_id: Uuid,
    context_type: String,
    context_key: String,
    variant_key: String,
}

struct Seed {
    env_id: Uuid,
    flag_id: Uuid,
    flag_key: String,
    rule_a: Uuid,
    rule_b: Uuid,
    /// Rule-bound experiment on rule_a, unit_context_types=['user'].
    rule_bound_exp: Uuid,
    rule_bound_iter: Uuid,
    /// Default-rule-bound experiment (separate flag), unit_context_types=['user'].
    default_rule_exp: Uuid,
    default_rule_iter: Uuid,
    default_rule_flag_id: Uuid,
    default_rule_flag_key: String,
    org_id: Uuid,
    project_id: Uuid,
}

async fn seed(pool: &PgPool) -> Seed {
    let org_id = Uuid::new_v4();
    let project_id = Uuid::new_v4();
    let env_id = Uuid::new_v4();
    let flag_id = Uuid::new_v4();
    let flag_key = format!("flag-corr-{}", &flag_id.to_string()[..8]);
    let default_rule_flag_id = Uuid::new_v4();
    let default_rule_flag_key = format!("flag-corr-dr-{}", &default_rule_flag_id.to_string()[..8]);
    let rule_a = Uuid::new_v4();
    let rule_b = Uuid::new_v4();

    sqlx::query("INSERT INTO organisations (id, name) VALUES ($1, $2)")
        .bind(org_id)
        .bind(format!("org-corr-{org_id}"))
        .execute(pool)
        .await
        .expect("seed org");
    sqlx::query("INSERT INTO projects (id, organisation_id, name) VALUES ($1, $2, $3)")
        .bind(project_id)
        .bind(org_id)
        .bind(format!("proj-corr-{project_id}"))
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

    for (fid, key) in [
        (flag_id, flag_key.clone()),
        (default_rule_flag_id, default_rule_flag_key.clone()),
    ] {
        sqlx::query(
            "INSERT INTO feature_flags (id, project_id, key, value_type, enabled) \
             VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(fid)
        .bind(project_id)
        .bind(key)
        .bind("bool")
        .bind(true)
        .execute(pool)
        .await
        .expect("seed flag");
    }

    for (i, rid) in [rule_a, rule_b].iter().enumerate() {
        sqlx::query(
            "INSERT INTO feature_flag_rules (id, flag_id, rule_index, rule_def) \
             VALUES ($1, $2, $3, $4::jsonb)",
        )
        .bind(rid)
        .bind(flag_id)
        .bind(i32::try_from(i).unwrap())
        .bind("{}")
        .execute(pool)
        .await
        .expect("seed rule");
    }

    // Rule-bound experiment on rule_a, ['user'] only.
    let rule_bound_exp = Uuid::new_v4();
    let rule_bound_iter = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO experiments \
            (id, env_id, flag_id, flag_rule_id, name, status, traffic_allocation, \
             targets_default_rule, unit_context_types) \
         VALUES ($1, $2, $3, $4, $5, 'running', 100.0, false, '{user}')",
    )
    .bind(rule_bound_exp)
    .bind(env_id)
    .bind(flag_id)
    .bind(rule_a)
    .bind(format!("corr-rb-{rule_bound_exp}"))
    .execute(pool)
    .await
    .expect("seed rule-bound experiment");
    sqlx::query(
        "INSERT INTO experiment_iterations \
            (id, experiment_id, flag_id, iteration_number, started_at, traffic_allocation, \
             targets_default_rule, unit_context_types) \
         VALUES ($1, $2, $3, 1, $4, 100.0, false, '{user}')",
    )
    .bind(rule_bound_iter)
    .bind(rule_bound_exp)
    .bind(flag_id)
    .bind(Utc::now())
    .execute(pool)
    .await
    .expect("seed rule-bound iteration");

    // Default-rule-bound experiment on a different flag, ['user'] only.
    let default_rule_exp = Uuid::new_v4();
    let default_rule_iter = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO experiments \
            (id, env_id, flag_id, flag_rule_id, name, status, traffic_allocation, \
             targets_default_rule, unit_context_types) \
         VALUES ($1, $2, $3, NULL, $4, 'running', 100.0, true, '{user}')",
    )
    .bind(default_rule_exp)
    .bind(env_id)
    .bind(default_rule_flag_id)
    .bind(format!("corr-dr-{default_rule_exp}"))
    .execute(pool)
    .await
    .expect("seed default-rule experiment");
    sqlx::query(
        "INSERT INTO experiment_iterations \
            (id, experiment_id, flag_id, iteration_number, started_at, traffic_allocation, \
             targets_default_rule, unit_context_types) \
         VALUES ($1, $2, $3, 1, $4, 100.0, true, '{user}')",
    )
    .bind(default_rule_iter)
    .bind(default_rule_exp)
    .bind(default_rule_flag_id)
    .bind(Utc::now())
    .execute(pool)
    .await
    .expect("seed default-rule iteration");

    Seed {
        env_id,
        flag_id,
        flag_key,
        rule_a,
        rule_b,
        rule_bound_exp,
        rule_bound_iter,
        default_rule_exp,
        default_rule_iter,
        default_rule_flag_id,
        default_rule_flag_key,
        org_id,
        project_id,
    }
}

async fn cleanup(pool: &PgPool, seed: &Seed) {
    let _ = sqlx::query("DELETE FROM experiment_iterations WHERE id IN ($1, $2)")
        .bind(seed.rule_bound_iter)
        .bind(seed.default_rule_iter)
        .execute(pool)
        .await;
    let _ = sqlx::query("DELETE FROM experiments WHERE id IN ($1, $2)")
        .bind(seed.rule_bound_exp)
        .bind(seed.default_rule_exp)
        .execute(pool)
        .await;
    let _ = sqlx::query("DELETE FROM feature_flag_rules WHERE flag_id = $1")
        .bind(seed.flag_id)
        .execute(pool)
        .await;
    let _ = sqlx::query("DELETE FROM feature_flags WHERE id IN ($1, $2)")
        .bind(seed.flag_id)
        .bind(seed.default_rule_flag_id)
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

async fn settle_and_optimize(client: &clickhouse::Client) {
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    client
        .query("OPTIMIZE TABLE experiment_assignments FINAL")
        .execute()
        .await
        .expect("optimize");
}

async fn fetch_for_experiment(
    client: &clickhouse::Client,
    experiment_id: Uuid,
) -> Vec<AssignmentSummary> {
    client
        .query(
            "SELECT experiment_id, iteration_id, context_type, context_key, variant_key \
             FROM experiment_assignments FINAL \
             WHERE experiment_id = ? \
             ORDER BY context_key",
        )
        .bind(experiment_id.to_string())
        .fetch_all()
        .await
        .expect("fetch")
}

/// Case 1: Eval matches rule_b on the rule_a-bound experiment's flag. The
/// dictionary key (env_id, flag_id, rule_b, 'user') has no entry → no
/// assignment row is produced.
#[tokio::test]
async fn different_rule_on_same_flag_does_not_attribute() {
    let Some(client) = ch_client() else {
        eprintln!("STITCHD_CLICKHOUSE_URL not set — skipping");
        return;
    };
    let Some(pool) = pg_pool().await else {
        return;
    };
    let seed = seed(&pool).await;
    prime_ch(&client).await;

    let row = EvalLogRow {
        env_id: seed.env_id,
        flag_id: seed.flag_id,
        flag_key: seed.flag_key.clone(),
        variant_key: "control".to_string(),
        targeting_on: true,
        evaluated_at: Utc::now(),
        context_type: "user".to_string(),
        context_key: "alice".to_string(),
        params_json: "{}".to_string(),
        matched_rule_id: Some(seed.rule_b), // wrong rule
        evaluation_id: uuid::Uuid::new_v4(),
    };
    insert_eval_log_rows(&client, &[row])
        .await
        .expect("insert eval log");
    settle_and_optimize(&client).await;

    let rows = fetch_for_experiment(&client, seed.rule_bound_exp).await;
    cleanup(&pool, &seed).await;
    assert!(
        rows.is_empty(),
        "rule_b eval must not attribute to a rule_a-bound experiment; got {rows:?}"
    );
}

/// Case 2: Eval `context_type='account'` against an experiment with
/// `unit_context_types = ['user']`. The dict key tuple includes
/// context_type, so 'account' is not in the view → dictHas=0 → no row.
#[tokio::test]
async fn out_of_scope_context_type_does_not_attribute() {
    let Some(client) = ch_client() else {
        eprintln!("STITCHD_CLICKHOUSE_URL not set — skipping");
        return;
    };
    let Some(pool) = pg_pool().await else {
        return;
    };
    let seed = seed(&pool).await;
    prime_ch(&client).await;

    let row = EvalLogRow {
        env_id: seed.env_id,
        flag_id: seed.flag_id,
        flag_key: seed.flag_key.clone(),
        variant_key: "treatment".to_string(),
        targeting_on: true,
        evaluated_at: Utc::now(),
        context_type: "account".to_string(),
        context_key: "acme".to_string(),
        params_json: "{}".to_string(),
        matched_rule_id: Some(seed.rule_a),
        evaluation_id: uuid::Uuid::new_v4(),
    };
    insert_eval_log_rows(&client, &[row])
        .await
        .expect("insert eval log");
    settle_and_optimize(&client).await;

    let rows = fetch_for_experiment(&client, seed.rule_bound_exp).await;
    cleanup(&pool, &seed).await;
    assert!(
        rows.is_empty(),
        "'account' is not in unit_context_types=['user']; must not attribute. Got {rows:?}"
    );
}

/// Case 3: Default-rule-bound experiment, eval row has `matched_rule_id IS
/// NULL` (fell through to the default rule). The dictionary key
/// `(env_id, default_rule_flag_id, NULL, 'user')` resolves → assignment row.
#[tokio::test]
async fn default_rule_bound_experiment_attributes_on_null_matched_rule() {
    let Some(client) = ch_client() else {
        eprintln!("STITCHD_CLICKHOUSE_URL not set — skipping");
        return;
    };
    let Some(pool) = pg_pool().await else {
        return;
    };
    let seed = seed(&pool).await;
    prime_ch(&client).await;

    let row = EvalLogRow {
        env_id: seed.env_id,
        flag_id: seed.default_rule_flag_id,
        flag_key: seed.default_rule_flag_key.clone(),
        variant_key: "treatment".to_string(),
        targeting_on: true,
        evaluated_at: Utc::now(),
        context_type: "user".to_string(),
        context_key: "dave".to_string(),
        params_json: "{}".to_string(),
        matched_rule_id: None, // default-rule fallthrough
        evaluation_id: uuid::Uuid::new_v4(),
    };
    insert_eval_log_rows(&client, &[row])
        .await
        .expect("insert eval log");
    settle_and_optimize(&client).await;

    let rows = fetch_for_experiment(&client, seed.default_rule_exp).await;
    cleanup(&pool, &seed).await;
    assert_eq!(
        rows.len(),
        1,
        "default-rule-bound experiment must attribute NULL matched_rule_id evals; got {rows:?}"
    );
    assert_eq!(rows[0].context_type, "user");
    assert_eq!(rows[0].context_key, "dave");
    assert_eq!(rows[0].variant_key, "treatment");
    assert_eq!(rows[0].iteration_id, seed.default_rule_iter);
}
