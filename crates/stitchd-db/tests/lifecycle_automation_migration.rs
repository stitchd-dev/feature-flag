//! Smoke test for `20260604000001_lifecycle_automation.sql`.
//!
//! `#[sqlx::test]` provisions a fresh isolated database and auto-runs every
//! migration in `./migrations`, so a passing test proves the new migration
//! applies cleanly on top of the full baseline. Uses runtime `sqlx::query_as`
//! (NOT the compile-time `query!`/`query_as!` macros) because the offline
//! `.sqlx` cache is not populated for these new tables.

use sqlx::PgPool;
use sqlx::Row;

/// All four new tables exist after migrations run.
#[sqlx::test(migrations = "./migrations")]
async fn lifecycle_tables_exist(pool: PgPool) {
    for table in [
        "scheduled_changes",
        "scheduled_change_runs",
        "flag_prerequisites",
        "entity_dependencies",
    ] {
        let exists: bool = sqlx::query_scalar(
            "SELECT EXISTS (
                SELECT 1 FROM information_schema.tables
                WHERE table_schema = 'public' AND table_name = $1
            )",
        )
        .bind(table)
        .fetch_one(&pool)
        .await
        .expect("schema query failed");
        assert!(exists, "table `{table}` should exist after migration");
    }
}

/// `feature_flags.fallback_variant_id` column was added.
#[sqlx::test(migrations = "./migrations")]
async fn feature_flags_has_fallback_variant_id(pool: PgPool) {
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS (
            SELECT 1 FROM information_schema.columns
            WHERE table_schema = 'public'
              AND table_name = 'feature_flags'
              AND column_name = 'fallback_variant_id'
        )",
    )
    .fetch_one(&pool)
    .await
    .expect("schema query failed");
    assert!(exists, "feature_flags.fallback_variant_id should exist");
}

/// `experiment_start_prerequisites` (migration `20260604000002`) exists with its
/// index after migrations run.
#[sqlx::test(migrations = "./migrations")]
async fn experiment_start_prerequisites_table_and_index_exist(pool: PgPool) {
    let table_exists: bool = sqlx::query_scalar(
        "SELECT EXISTS (
            SELECT 1 FROM information_schema.tables
            WHERE table_schema = 'public'
              AND table_name = 'experiment_start_prerequisites'
        )",
    )
    .fetch_one(&pool)
    .await
    .expect("schema query failed");
    assert!(table_exists, "experiment_start_prerequisites should exist");

    let index_exists: bool = sqlx::query_scalar(
        "SELECT EXISTS (
            SELECT 1 FROM pg_indexes
            WHERE schemaname = 'public'
              AND indexname = 'idx_experiment_start_prereq_experiment'
        )",
    )
    .fetch_one(&pool)
    .await
    .expect("schema query failed");
    assert!(
        index_exists,
        "idx_experiment_start_prereq_experiment should exist"
    );
}

/// The partial due-change index exists.
#[sqlx::test(migrations = "./migrations")]
async fn due_change_partial_index_exists(pool: PgPool) {
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS (
            SELECT 1 FROM pg_indexes
            WHERE schemaname = 'public'
              AND indexname = 'idx_scheduled_changes_due'
        )",
    )
    .fetch_one(&pool)
    .await
    .expect("schema query failed");
    assert!(exists, "idx_scheduled_changes_due should exist");
}

/// A round-trip insert/select against `scheduled_changes` honours the status
/// default and CHECK constraints, proving the table is usable.
#[sqlx::test(migrations = "./migrations")]
async fn scheduled_changes_round_trip(pool: PgPool) {
    let id: uuid::Uuid = uuid::Uuid::new_v4();
    let entity_id: uuid::Uuid = uuid::Uuid::new_v4();
    let env_id: uuid::Uuid = uuid::Uuid::new_v4();

    sqlx::query(
        "INSERT INTO scheduled_changes
            (id, entity_type, entity_id, env_id, mutation_payload, schedule_kind,
             scheduled_at, next_run_at)
         VALUES ($1, 'flag', $2, $3, '{}'::jsonb, 'one_shot', now(), now())",
    )
    .bind(id)
    .bind(entity_id)
    .bind(env_id)
    .execute(&pool)
    .await
    .expect("insert failed");

    let row = sqlx::query("SELECT status, version FROM scheduled_changes WHERE id = $1")
        .bind(id)
        .fetch_one(&pool)
        .await
        .expect("select failed");
    let status: String = row.get("status");
    let version: i64 = row.get("version");
    assert_eq!(status, "pending", "status should default to pending");
    assert_eq!(version, 1, "version should default to 1");

    // A run-history row references the change.
    sqlx::query(
        "INSERT INTO scheduled_change_runs (scheduled_change_id, outcome, detail)
         VALUES ($1, 'applied', 'ok')",
    )
    .bind(id)
    .execute(&pool)
    .await
    .expect("run-history insert failed");
}

/// The `entity_dependencies` unique edge constraint is enforced.
#[sqlx::test(migrations = "./migrations")]
async fn entity_dependencies_unique_edge(pool: PgPool) {
    let from_id = uuid::Uuid::new_v4();
    let to_id = uuid::Uuid::new_v4();

    let insert = |from_id: uuid::Uuid, to_id: uuid::Uuid| {
        sqlx::query(
            "INSERT INTO entity_dependencies (from_type, from_id, to_type, to_id, kind)
             VALUES ('flag', $1, 'flag', $2, 'prerequisite')",
        )
        .bind(from_id)
        .bind(to_id)
    };

    insert(from_id, to_id)
        .execute(&pool)
        .await
        .expect("first edge insert should succeed");

    let dup = insert(from_id, to_id).execute(&pool).await;
    assert!(
        dup.is_err(),
        "duplicate (from,to,kind) edge should violate the unique constraint"
    );
}
