//! Integration test for the one-shot 90-day backfill migration
//! (`20260521000004_backfill_experiment_assignments`).
//!
//! Scenario:
//!   1. Migration order at first apply: eval-log INSERTs land BEFORE the
//!      iteration starts → MV's `dictHas` check fails → those rows do NOT
//!      become assignments at insert time.
//!   2. Operator starts the experiment iteration in PG; the dictionary
//!      refresh picks it up.
//!   3. The backfill migration's SELECT-INSERT scans the last 90 days of
//!      `flag_evaluation_log` and emits assignment rows for the now-active
//!      iteration.
//!
//! The test simulates this end-to-end against live PG + CH.

use chrono::Utc;
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

struct Seed {
    env_id: Uuid,
    flag_id: Uuid,
    flag_key: String,
    rule_a: Uuid,
    experiment_id: Uuid,
    iteration_id: Uuid,
    org_id: Uuid,
    project_id: Uuid,
}

async fn seed_pg_base(pool: &PgPool) -> Seed {
    let org_id = Uuid::new_v4();
    let project_id = Uuid::new_v4();
    let env_id = Uuid::new_v4();
    let flag_id = Uuid::new_v4();
    let flag_key = format!("flag-bf-{}", &flag_id.to_string()[..8]);
    let rule_a = Uuid::new_v4();

    sqlx::query("INSERT INTO organisations (id, name) VALUES ($1, $2)")
        .bind(org_id)
        .bind(format!("org-bf-{org_id}"))
        .execute(pool)
        .await
        .expect("seed org");
    sqlx::query("INSERT INTO projects (id, organisation_id, name) VALUES ($1, $2, $3)")
        .bind(project_id)
        .bind(org_id)
        .bind(format!("proj-bf-{project_id}"))
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
    .bind(flag_key.clone())
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

    Seed {
        env_id,
        flag_id,
        flag_key,
        rule_a,
        experiment_id: Uuid::nil(),
        iteration_id: Uuid::nil(),
        org_id,
        project_id,
    }
}

async fn start_iteration(pool: &PgPool, seed: &mut Seed) {
    seed.experiment_id = Uuid::new_v4();
    seed.iteration_id = Uuid::new_v4();

    sqlx::query(
        "INSERT INTO experiments \
            (id, env_id, flag_id, flag_rule_id, name, status, traffic_allocation, \
             targets_default_rule, unit_context_types, analysis_type) \
         VALUES ($1, $2, $3, $4, $5, 'running', 100.0, false, '{user}', 'frequentist')",
    )
    .bind(seed.experiment_id)
    .bind(seed.env_id)
    .bind(seed.flag_id)
    .bind(seed.rule_a)
    .bind(format!("bf-exp-{}", seed.experiment_id))
    .execute(pool)
    .await
    .expect("create experiment");

    sqlx::query(
        "INSERT INTO experiment_iterations \
            (id, experiment_id, flag_id, iteration_number, started_at, traffic_allocation, \
             targets_default_rule, unit_context_types) \
         VALUES ($1, $2, $3, 1, $4, 100.0, false, '{user}')",
    )
    .bind(seed.iteration_id)
    .bind(seed.experiment_id)
    .bind(seed.flag_id)
    .bind(Utc::now() - chrono::Duration::hours(1)) // started in the past
    .execute(pool)
    .await
    .expect("create iteration");
}

async fn cleanup(pool: &PgPool, seed: &Seed) {
    if !seed.iteration_id.is_nil() {
        let _ = sqlx::query("DELETE FROM experiment_iterations WHERE id = $1")
            .bind(seed.iteration_id)
            .execute(pool)
            .await;
    }
    if !seed.experiment_id.is_nil() {
        let _ = sqlx::query("DELETE FROM experiments WHERE id = $1")
            .bind(seed.experiment_id)
            .execute(pool)
            .await;
    }
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

async fn reload_dict(client: &clickhouse::Client) {
    client
        .query("SYSTEM RELOAD DICTIONARY experiment_iterations_active")
        .execute()
        .await
        .expect("reload dictionary");
}

fn eval_row(
    seed: &Seed,
    context_key: &str,
    variant: &str,
    matched_rule_id: Option<Uuid>,
    when: chrono::DateTime<Utc>,
) -> EvalLogRow {
    EvalLogRow {
        env_id: seed.env_id,
        flag_id: seed.flag_id,
        flag_key: seed.flag_key.clone(),
        variant_key: variant.to_string(),
        targeting_on: true,
        evaluated_at: when,
        context_type: "user".to_string(),
        context_key: context_key.to_string(),
        params_json: "{}".to_string(),
        matched_rule_id,
    }
}

/// Run the backfill SELECT-INSERT statement directly (same body as the
/// `20260521000004_backfill_experiment_assignments.sql` migration). We
/// extract this rather than calling `ch_migrations::run` so the test can
/// trigger the backfill independent of the migration tracker.
async fn run_backfill_query(client: &clickhouse::Client) {
    client
        .query(
            "INSERT INTO experiment_assignments \
             SELECT \
                 dictGet('experiment_iterations_active', 'experiment_id', \
                         (e.env_id, e.flag_id, e.matched_rule_id, e.context_type)), \
                 dictGet('experiment_iterations_active', 'iteration_id', \
                         (e.env_id, e.flag_id, e.matched_rule_id, e.context_type)), \
                 e.env_id, e.flag_id, e.matched_rule_id, \
                 CAST(e.context_type AS LowCardinality(String)), \
                 e.context_key, e.variant_key, e.evaluated_at, \
                 -toUnixTimestamp64Milli(e.evaluated_at) \
             FROM flag_evaluation_log AS e \
             WHERE e.targeting_on = true \
               AND e.evaluated_at >= now() - INTERVAL 90 DAY \
               AND e.matched_rule_id IS NOT NULL \
               AND dictHas('experiment_iterations_active', \
                           (e.env_id, e.flag_id, e.matched_rule_id, e.context_type))",
        )
        .execute()
        .await
        .expect("backfill insert");
}

async fn count_assignments(client: &clickhouse::Client, experiment_id: Uuid) -> u64 {
    client
        .query(
            "SELECT count() FROM experiment_assignments FINAL \
             WHERE experiment_id = ?",
        )
        .bind(experiment_id.to_string())
        .fetch_one()
        .await
        .expect("count")
}

/// Backfill catches eval-log rows that were inserted BEFORE the iteration
/// existed. Pre-iteration evals are filtered by the live MV (dictHas=0);
/// the backfill picks them up after the iteration starts.
#[tokio::test]
async fn backfill_attributes_history_after_iteration_starts() {
    let Some(client) = ch_client() else {
        eprintln!("STITCHD_CLICKHOUSE_URL not set — skipping");
        return;
    };
    let Some(pool) = pg_pool().await else {
        return;
    };
    ch_migrations::run(&client).await.expect("migrations");

    let mut seed = seed_pg_base(&pool).await;
    reload_dict(&client).await;

    // Eval-log rows written BEFORE the iteration exists. The MV's dictHas
    // returns 0 → these rows do NOT become assignments at insert time.
    let now = Utc::now();
    let rows = vec![
        eval_row(
            &seed,
            "alice",
            "control",
            Some(seed.rule_a),
            now - chrono::Duration::minutes(30),
        ),
        eval_row(
            &seed,
            "bob",
            "treatment",
            Some(seed.rule_a),
            now - chrono::Duration::minutes(20),
        ),
        eval_row(
            &seed,
            "carol",
            "control",
            Some(seed.rule_a),
            now - chrono::Duration::minutes(10),
        ),
    ];
    insert_eval_log_rows(&client, &rows)
        .await
        .expect("insert eval log");

    // Wait for the MV to settle (it'll process the rows but the dictHas
    // filter drops them — no assignments produced).
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    // Start the iteration in PG (the trigger bumps the audit row, and we
    // also force a reload).
    start_iteration(&pool, &mut seed).await;
    reload_dict(&client).await;

    // Run the backfill SELECT-INSERT. The dictHas now resolves the
    // existing eval-log rows to the freshly-started iteration.
    run_backfill_query(&client).await;

    // Wait + OPTIMIZE to collapse ReplacingMergeTree.
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    client
        .query("OPTIMIZE TABLE experiment_assignments FINAL")
        .execute()
        .await
        .expect("optimize");

    let count = count_assignments(&client, seed.experiment_id).await;
    cleanup(&pool, &seed).await;
    assert_eq!(
        count, 3,
        "backfill must produce one assignment per distinct context"
    );
}

/// Backfill skips rows with `matched_rule_id IS NULL` to avoid attributing
/// pre-migration NULL rows to default-rule-bound experiments. (Spec §1.)
#[tokio::test]
async fn backfill_skips_null_matched_rule_id_rows() {
    let Some(client) = ch_client() else {
        eprintln!("STITCHD_CLICKHOUSE_URL not set — skipping");
        return;
    };
    let Some(pool) = pg_pool().await else {
        return;
    };
    ch_migrations::run(&client).await.expect("migrations");

    let mut seed = seed_pg_base(&pool).await;
    reload_dict(&client).await;

    // Two eval-log rows: one with a matched_rule_id (should backfill), one
    // with NULL (should be skipped even though we'll later have a
    // default-rule iteration in scope — pre-migration semantics).
    let now = Utc::now();
    let rows = vec![
        eval_row(
            &seed,
            "alice",
            "control",
            Some(seed.rule_a),
            now - chrono::Duration::minutes(30),
        ),
        eval_row(
            &seed,
            "bob",
            "treatment",
            None, // NULL matched_rule_id — pre-migration shape
            now - chrono::Duration::minutes(20),
        ),
    ];
    insert_eval_log_rows(&client, &rows)
        .await
        .expect("insert eval log");

    start_iteration(&pool, &mut seed).await;
    reload_dict(&client).await;
    run_backfill_query(&client).await;
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    client
        .query("OPTIMIZE TABLE experiment_assignments FINAL")
        .execute()
        .await
        .expect("optimize");

    let count = count_assignments(&client, seed.experiment_id).await;
    cleanup(&pool, &seed).await;
    assert_eq!(
        count, 1,
        "only the matched_rule_id-tagged row backfills; NULL-rule row skipped"
    );
}
