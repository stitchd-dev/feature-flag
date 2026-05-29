//! Schema-level tests for migration `20260521000001_experiment_attribution_fields`,
//! `20260521000002_flag_default_rule_distribution`, and
//! `20260521000003_experiment_iterations_snapshot`.
//!
//! These tests assert the live schema shape rather than going through the
//! Rust repository layer, because the repo + domain types are updated in
//! later tasks of Phase 1 (the cache regeneration in Task 1.5 explicitly
//! follows the domain + repo updates in Tasks 3 + 4). Raw `sqlx::query`
//! strings are used per the convention in `conductor/patterns.md`
//! (*sqlx::query_as for new tables*).
//!
//! Test coverage:
//!
//!   1. XOR constraint accepts `(flag_rule_id IS NOT NULL, targets_default_rule=false)`
//!   2. XOR constraint accepts `(flag_rule_id IS NULL,     targets_default_rule=true)`
//!   3. XOR constraint rejects `(flag_rule_id IS NULL,     targets_default_rule=false)`
//!   4. XOR constraint rejects `(flag_rule_id IS NOT NULL, targets_default_rule=true)`
//!   5. `idx_experiments_one_active_per_flag` rejects two running experiments on
//!      the same flag (even when bound to different rules).
//!   6. `experiments_unit_context_types_nonempty` rejects an empty array.
//!   7. `experiments_pre_period_days_nonneg` rejects a negative value.
//!   8. `feature_flags.default_rule_distribution` accepts arbitrary JSONB.
//!   9. `experiment_iterations` carries the snapshot columns + flag_id NOT NULL.

use sqlx::PgPool;
use uuid::Uuid;

/// Seed the minimum set of rows required to insert an experiment row.
/// Returns `(env_id, flag_id, flag_rule_id, alt_flag_rule_id)`.
async fn seed_minimum(pool: &PgPool) -> (Uuid, Uuid, Uuid, Uuid) {
    let org_id = Uuid::new_v4();
    let project_id = Uuid::new_v4();
    let env_id = Uuid::new_v4();
    let flag_id = Uuid::new_v4();
    let rule_a = Uuid::new_v4();
    let rule_b = Uuid::new_v4();

    sqlx::query("INSERT INTO organisations (id, name) VALUES ($1, $2)")
        .bind(org_id)
        .bind(format!("org-{org_id}"))
        .execute(pool)
        .await
        .expect("seed organisation");

    sqlx::query("INSERT INTO projects (id, organisation_id, name) VALUES ($1, $2, $3)")
        .bind(project_id)
        .bind(org_id)
        .bind(format!("proj-{project_id}"))
        .execute(pool)
        .await
        .expect("seed project");

    sqlx::query("INSERT INTO environments (id, project_id, name) VALUES ($1, $2, $3)")
        .bind(env_id)
        .bind(project_id)
        .bind("dev")
        .execute(pool)
        .await
        .expect("seed environment");

    sqlx::query(
        "INSERT INTO feature_flags (id, project_id, key, value_type, enabled) \
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(flag_id)
    .bind(project_id)
    .bind(format!("flag-{flag_id}"))
    .bind("bool")
    .bind(true)
    .execute(pool)
    .await
    .expect("seed feature_flag");

    for (idx, rid) in [(0_i32, rule_a), (1_i32, rule_b)] {
        sqlx::query(
            "INSERT INTO feature_flag_rules (id, flag_id, rule_index, rule_def) \
             VALUES ($1, $2, $3, $4::jsonb)",
        )
        .bind(rid)
        .bind(flag_id)
        .bind(idx)
        .bind("{}")
        .execute(pool)
        .await
        .expect("seed feature_flag_rule");
    }

    (env_id, flag_id, rule_a, rule_b)
}

#[allow(clippy::too_many_arguments)] // test helper; constraints make consolidation noisy
async fn insert_experiment(
    pool: &PgPool,
    env_id: Uuid,
    flag_id: Uuid,
    flag_rule_id: Option<Uuid>,
    targets_default_rule: bool,
    status: &str,
    unit_context_types: &[&str],
    pre_period_days: i32,
) -> Result<Uuid, sqlx::Error> {
    let exp_id = Uuid::new_v4();
    let ucts: Vec<String> = unit_context_types
        .iter()
        .map(|s| (*s).to_string())
        .collect();
    sqlx::query(
        "INSERT INTO experiments \
            (id, env_id, flag_id, flag_rule_id, name, status, traffic_allocation, \
             targets_default_rule, unit_context_types, pre_period_days) \
         VALUES ($1, $2, $3, $4, $5, $6, $7::numeric, $8, $9, $10)",
    )
    .bind(exp_id)
    .bind(env_id)
    .bind(flag_id)
    .bind(flag_rule_id)
    .bind(format!("exp-{exp_id}"))
    .bind(status)
    .bind(100.0_f64)
    .bind(targets_default_rule)
    .bind(&ucts)
    .bind(pre_period_days)
    .execute(pool)
    .await
    .map(|_| exp_id)
}

#[sqlx::test(migrations = "./migrations")]
async fn xor_accepts_rule_bound_experiment(pool: PgPool) {
    let (env, flag, rule, _) = seed_minimum(&pool).await;
    insert_experiment(&pool, env, flag, Some(rule), false, "draft", &["user"], 0)
        .await
        .expect("rule-bound + targets_default_rule=false must satisfy XOR");
}

#[sqlx::test(migrations = "./migrations")]
async fn xor_accepts_default_rule_bound_experiment(pool: PgPool) {
    let (env, flag, _, _) = seed_minimum(&pool).await;
    insert_experiment(&pool, env, flag, None, true, "draft", &["user"], 0)
        .await
        .expect("flag_rule_id=NULL + targets_default_rule=true must satisfy XOR");
}

#[sqlx::test(migrations = "./migrations")]
async fn xor_rejects_neither_set(pool: PgPool) {
    let (env, flag, _, _) = seed_minimum(&pool).await;
    let err = insert_experiment(&pool, env, flag, None, false, "draft", &["user"], 0)
        .await
        .expect_err("must violate XOR when neither is set");
    let msg = err.to_string();
    assert!(
        msg.contains("experiments_rule_xor_default"),
        "expected XOR constraint violation, got: {msg}"
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn xor_rejects_both_set(pool: PgPool) {
    let (env, flag, rule, _) = seed_minimum(&pool).await;
    let err = insert_experiment(&pool, env, flag, Some(rule), true, "draft", &["user"], 0)
        .await
        .expect_err("must violate XOR when both are set");
    assert!(err.to_string().contains("experiments_rule_xor_default"));
}

#[sqlx::test(migrations = "./migrations")]
async fn unique_per_flag_index_blocks_second_running_experiment(pool: PgPool) {
    let (env, flag, rule_a, rule_b) = seed_minimum(&pool).await;
    insert_experiment(
        &pool,
        env,
        flag,
        Some(rule_a),
        false,
        "running",
        &["user"],
        0,
    )
    .await
    .expect("first running experiment on flag should succeed");
    let err = insert_experiment(
        &pool,
        env,
        flag,
        Some(rule_b),
        false,
        "running",
        &["user"],
        0,
    )
    .await
    .expect_err("second running experiment on same flag must violate uniqueness");
    assert!(
        err.to_string()
            .contains("idx_experiments_one_active_per_flag"),
        "expected per-flag uniqueness violation, got: {err}"
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn empty_unit_context_types_is_rejected(pool: PgPool) {
    let (env, flag, rule, _) = seed_minimum(&pool).await;
    let empty: Vec<&str> = vec![];
    let err = insert_experiment(&pool, env, flag, Some(rule), false, "draft", &empty, 0)
        .await
        .expect_err("empty unit_context_types must violate CHECK");
    assert!(
        err.to_string()
            .contains("experiments_unit_context_types_nonempty"),
        "expected unit_context_types CHECK violation, got: {err}"
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn negative_pre_period_days_is_rejected(pool: PgPool) {
    let (env, flag, rule, _) = seed_minimum(&pool).await;
    let err = insert_experiment(&pool, env, flag, Some(rule), false, "draft", &["user"], -1)
        .await
        .expect_err("negative pre_period_days must violate CHECK");
    assert!(
        err.to_string()
            .contains("experiments_pre_period_days_nonneg"),
        "expected pre_period_days CHECK violation, got: {err}"
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn flag_default_rule_distribution_accepts_jsonb(pool: PgPool) {
    let (_, flag, _, _) = seed_minimum(&pool).await;
    let distribution = serde_json::json!({
        "allocations": [
            { "variant_key": "control",  "percentage": 50.0 },
            { "variant_key": "treatment","percentage": 50.0 }
        ]
    });
    sqlx::query("UPDATE feature_flags SET default_rule_distribution = $1::jsonb WHERE id = $2")
        .bind(distribution.clone())
        .bind(flag)
        .execute(&pool)
        .await
        .expect("UPDATE default_rule_distribution must succeed");

    let row: (Option<serde_json::Value>,) =
        sqlx::query_as("SELECT default_rule_distribution FROM feature_flags WHERE id = $1")
            .bind(flag)
            .fetch_one(&pool)
            .await
            .expect("SELECT must succeed");
    assert_eq!(row.0, Some(distribution));
}

#[sqlx::test(migrations = "./migrations")]
async fn experiment_iterations_carries_snapshot_columns(pool: PgPool) {
    let (env, flag, rule, _) = seed_minimum(&pool).await;
    let exp = insert_experiment(
        &pool,
        env,
        flag,
        Some(rule),
        false,
        "draft",
        &["user", "account"],
        7,
    )
    .await
    .expect("seed experiment");

    let iter_id = Uuid::new_v4();
    let ucts: Vec<String> = vec!["user".into(), "account".into()];
    sqlx::query(
        "INSERT INTO experiment_iterations \
            (id, experiment_id, flag_id, iteration_number, started_at, \
             metric_ids, traffic_allocation, targets_default_rule, \
             guardrail_metric_ids, pre_period_days, unit_context_types) \
         VALUES ($1, $2, $3, $4, NOW(), $5, 100.0, $6, '{}', $7, $8)",
    )
    .bind(iter_id)
    .bind(exp)
    .bind(flag)
    .bind(1_i32)
    .bind(Vec::<Uuid>::new())
    .bind(false)
    .bind(7_i32)
    .bind(&ucts)
    .execute(&pool)
    .await
    .expect("INSERT iteration with new snapshot columns must succeed");

    let (got_flag, got_pre, got_ucts): (Uuid, i32, Vec<String>) = sqlx::query_as(
        "SELECT flag_id, pre_period_days, unit_context_types \
         FROM experiment_iterations WHERE id = $1",
    )
    .bind(iter_id)
    .fetch_one(&pool)
    .await
    .expect("SELECT iteration");
    assert_eq!(got_flag, flag);
    assert_eq!(got_pre, 7);
    assert_eq!(got_ucts, vec!["user".to_string(), "account".to_string()]);
}
