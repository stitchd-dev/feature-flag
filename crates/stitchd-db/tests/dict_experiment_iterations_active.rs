//! Integration test: `experiment_iterations_active` ClickHouse dictionary
//! lookups for rule-bound + default-rule-bound experiments.
//!
//! Drives both halves of the Phase 4 attribution pipeline plumbing:
//!   * PG view `v_experiment_iterations_active` (created in PG migration
//!     `20260521000004_v_experiment_iterations_active`) returns the right
//!     `(env_id, flag_id, matched_rule_id, context_type)` tuples.
//!   * CH dictionary `experiment_iterations_active` (created in CH
//!     migration `20260521000002_experiment_iterations_active_dict`) loads
//!     from that view and answers `dictGet` / `dictHas` queries correctly
//!     for both UUID and NULL `matched_rule_id` key parts.
//!
//! Skipped when `STITCHD_CLICKHOUSE_URL`/`DATABASE_URL` are not set (CI
//! without DB).
//!
//! **DB selection:** The CH dictionary's `SOURCE(POSTGRESQL(... db 'stitchd'))`
//! points at the shared `stitchd` database — not the per-test temp DB that
//! `#[sqlx::test]` provisions. So this test uses a manual `PgPool` against
//! the shared DB, uses fresh random UUIDs to avoid collisions, and cleans up
//! after itself. The audit trigger + `SYSTEM RELOAD DICTIONARY` bumps the
//! dictionary contents within the test window.

use chrono::Utc;
use sqlx::PgPool;
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

/// Insert the minimum PG rows for a (rule_bound, default_rule_bound) pair on
/// fresh UUIDs so the test is parallelisable. Returns the seeded IDs + a
/// `Drop`-style cleanup closure embedded as the returned struct.
struct Seed {
    env_id: Uuid,
    flag_a: Uuid,
    flag_b: Uuid,
    rule_a: Uuid,
    rule_b: Uuid,
    rule_bound_exp: Uuid,
    rule_bound_iter: Uuid,
    default_rule_exp: Uuid,
    default_rule_iter: Uuid,
    // Owned IDs for cleanup
    org_id: Uuid,
    project_id: Uuid,
}

async fn seed_minimum(pool: &PgPool) -> Seed {
    let org_id = Uuid::new_v4();
    let project_id = Uuid::new_v4();
    let env_id = Uuid::new_v4();
    let flag_a = Uuid::new_v4();
    let flag_b = Uuid::new_v4();
    let rule_a = Uuid::new_v4();
    let rule_b = Uuid::new_v4();

    sqlx::query("INSERT INTO organisations (id, name) VALUES ($1, $2)")
        .bind(org_id)
        .bind(format!("org-dict-{org_id}"))
        .execute(pool)
        .await
        .expect("seed org");
    sqlx::query("INSERT INTO projects (id, organisation_id, name) VALUES ($1, $2, $3)")
        .bind(project_id)
        .bind(org_id)
        .bind(format!("proj-dict-{project_id}"))
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

    for (fid, key) in [(flag_a, "flag-a"), (flag_b, "flag-b")] {
        sqlx::query(
            "INSERT INTO feature_flags (id, project_id, key, value_type, enabled) \
             VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(fid)
        .bind(project_id)
        .bind(format!("{key}-{}", &fid.to_string()[..8]))
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
        .bind(flag_a)
        .bind(i32::try_from(i).unwrap())
        .bind("{}")
        .execute(pool)
        .await
        .expect("seed rule");
    }

    // Rule-bound experiment + iteration on flag_a + rule_a.
    let rule_bound_exp = Uuid::new_v4();
    let rule_bound_iter = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO experiments \
            (id, env_id, flag_id, flag_rule_id, name, status, traffic_allocation, \
             targets_default_rule, unit_context_types, analysis_type) \
         VALUES ($1, $2, $3, $4, $5, 'running', 100.0, false, '{user}', 'frequentist')",
    )
    .bind(rule_bound_exp)
    .bind(env_id)
    .bind(flag_a)
    .bind(rule_a)
    .bind(format!("rule-bound-{rule_bound_exp}"))
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
    .bind(flag_a)
    .bind(Utc::now())
    .execute(pool)
    .await
    .expect("seed rule-bound iteration");

    // Default-rule-bound experiment + iteration on flag_b — two context types.
    let default_rule_exp = Uuid::new_v4();
    let default_rule_iter = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO experiments \
            (id, env_id, flag_id, flag_rule_id, name, status, traffic_allocation, \
             targets_default_rule, unit_context_types, analysis_type) \
         VALUES ($1, $2, $3, NULL, $4, 'running', 100.0, true, '{user,account}', 'frequentist')",
    )
    .bind(default_rule_exp)
    .bind(env_id)
    .bind(flag_b)
    .bind(format!("default-rule-{default_rule_exp}"))
    .execute(pool)
    .await
    .expect("seed default-rule experiment");

    sqlx::query(
        "INSERT INTO experiment_iterations \
            (id, experiment_id, flag_id, iteration_number, started_at, traffic_allocation, \
             targets_default_rule, unit_context_types) \
         VALUES ($1, $2, $3, 1, $4, 100.0, true, '{user,account}')",
    )
    .bind(default_rule_iter)
    .bind(default_rule_exp)
    .bind(flag_b)
    .bind(Utc::now())
    .execute(pool)
    .await
    .expect("seed default-rule iteration");

    Seed {
        env_id,
        flag_a,
        flag_b,
        rule_a,
        rule_b,
        rule_bound_exp,
        rule_bound_iter,
        default_rule_exp,
        default_rule_iter,
        org_id,
        project_id,
    }
}

async fn cleanup(pool: &PgPool, seed: &Seed) {
    // CASCADE the cleanup: iterations + experiments first, then rules, flags,
    // env, project, org. Most tables have ON DELETE restrictions, so explicit
    // order matters.
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
    let _ = sqlx::query("DELETE FROM feature_flag_rules WHERE id IN ($1, $2)")
        .bind(seed.rule_a)
        .bind(seed.rule_b)
        .execute(pool)
        .await;
    let _ = sqlx::query("DELETE FROM feature_flags WHERE id IN ($1, $2)")
        .bind(seed.flag_a)
        .bind(seed.flag_b)
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

/// Apply CH migrations + force the dictionary to reload so the freshly-seeded
/// PG rows are visible.
async fn prime_ch(client: &clickhouse::Client) {
    ch_migrations::run(client).await.expect("CH migrations");
    client
        .query("SYSTEM RELOAD DICTIONARY experiment_iterations_active")
        .execute()
        .await
        .expect("reload dictionary");
}

/// Rule-bound iteration: dictGet returns experiment_id + iteration_id for the
/// matching (env_id, flag_id, rule_a, 'user') tuple.
#[tokio::test]
async fn rule_bound_iteration_resolves_via_dict_get() {
    let Some(client) = ch_client() else {
        eprintln!("STITCHD_CLICKHOUSE_URL not set — skipping");
        return;
    };
    let Some(pool) = pg_pool().await else {
        eprintln!("DATABASE_URL not set — skipping");
        return;
    };
    let seed = seed_minimum(&pool).await;
    prime_ch(&client).await;

    let exp_id: String = client
        .query(
            "SELECT toString(dictGet('experiment_iterations_active', 'experiment_id', \
                            (toUUID(?), toUUID(?), toNullable(toUUID(?)), 'user')))",
        )
        .bind(seed.env_id.to_string())
        .bind(seed.flag_a.to_string())
        .bind(seed.rule_a.to_string())
        .fetch_one()
        .await
        .expect("dictGet experiment_id");

    let iter_id: String = client
        .query(
            "SELECT toString(dictGet('experiment_iterations_active', 'iteration_id', \
                            (toUUID(?), toUUID(?), toNullable(toUUID(?)), 'user')))",
        )
        .bind(seed.env_id.to_string())
        .bind(seed.flag_a.to_string())
        .bind(seed.rule_a.to_string())
        .fetch_one()
        .await
        .expect("dictGet iteration_id");

    cleanup(&pool, &seed).await;
    assert_eq!(exp_id, seed.rule_bound_exp.to_string());
    assert_eq!(iter_id, seed.rule_bound_iter.to_string());
}

/// dictHas with the wrong rule UUID returns 0.
#[tokio::test]
async fn rule_bound_other_rule_does_not_match() {
    let Some(client) = ch_client() else {
        eprintln!("STITCHD_CLICKHOUSE_URL not set — skipping");
        return;
    };
    let Some(pool) = pg_pool().await else {
        return;
    };
    let seed = seed_minimum(&pool).await;
    prime_ch(&client).await;

    let has: u8 = client
        .query(
            "SELECT dictHas('experiment_iterations_active', \
                            (toUUID(?), toUUID(?), toNullable(toUUID(?)), 'user'))",
        )
        .bind(seed.env_id.to_string())
        .bind(seed.flag_a.to_string())
        .bind(seed.rule_b.to_string())
        .fetch_one()
        .await
        .expect("dictHas");
    cleanup(&pool, &seed).await;
    assert_eq!(has, 0, "rule_b is unbound; dictHas must return 0");
}

/// Default-rule-bound iteration: NULL `matched_rule_id` key part still
/// resolves to the right experiment via dictGet.
#[tokio::test]
async fn default_rule_iteration_resolves_with_null_key() {
    let Some(client) = ch_client() else {
        eprintln!("STITCHD_CLICKHOUSE_URL not set — skipping");
        return;
    };
    let Some(pool) = pg_pool().await else {
        return;
    };
    let seed = seed_minimum(&pool).await;
    prime_ch(&client).await;

    let exp_id: String = client
        .query(
            "SELECT toString(dictGet('experiment_iterations_active', 'experiment_id', \
                            (toUUID(?), toUUID(?), CAST(NULL AS Nullable(UUID)), 'user')))",
        )
        .bind(seed.env_id.to_string())
        .bind(seed.flag_b.to_string())
        .fetch_one()
        .await
        .expect("dictGet with NULL matched_rule_id");
    assert_eq!(exp_id, seed.default_rule_exp.to_string());

    // Same iteration covers the 'account' context type too.
    let iter_id: String = client
        .query(
            "SELECT toString(dictGet('experiment_iterations_active', 'iteration_id', \
                            (toUUID(?), toUUID(?), CAST(NULL AS Nullable(UUID)), 'account')))",
        )
        .bind(seed.env_id.to_string())
        .bind(seed.flag_b.to_string())
        .fetch_one()
        .await
        .expect("dictGet iteration_id (account)");
    cleanup(&pool, &seed).await;
    assert_eq!(iter_id, seed.default_rule_iter.to_string());
}

/// A context_type outside `unit_context_types` (e.g. `org`) is filtered out
/// of the view, so `dictHas` returns 0 — the MV uses this to drop
/// out-of-scope eval rows.
#[tokio::test]
async fn out_of_scope_context_type_does_not_match() {
    let Some(client) = ch_client() else {
        eprintln!("STITCHD_CLICKHOUSE_URL not set — skipping");
        return;
    };
    let Some(pool) = pg_pool().await else {
        return;
    };
    let seed = seed_minimum(&pool).await;
    prime_ch(&client).await;

    let has_org: u8 = client
        .query(
            "SELECT dictHas('experiment_iterations_active', \
                            (toUUID(?), toUUID(?), CAST(NULL AS Nullable(UUID)), 'org'))",
        )
        .bind(seed.env_id.to_string())
        .bind(seed.flag_b.to_string())
        .fetch_one()
        .await
        .expect("dictHas org");
    cleanup(&pool, &seed).await;
    assert_eq!(has_org, 0, "org context is out-of-scope; must not match");
}
