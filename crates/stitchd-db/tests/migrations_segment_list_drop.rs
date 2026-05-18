//! Verifies that the `20260516000005_drop_segment_list_entries` migration was applied:
//! the `segment_list_entries` table and its covering index no longer exist.
//!
//! Requires a running PostgreSQL with all migrations applied (standard sqlx test setup).

use sqlx::PgPool;

#[sqlx::test]
async fn segment_list_entries_table_is_dropped(pool: PgPool) {
    let row: (bool,) = sqlx::query_as(
        "SELECT EXISTS (
            SELECT 1 FROM information_schema.tables
            WHERE table_schema = 'public'
              AND table_name = 'segment_list_entries'
        )",
    )
    .fetch_one(&pool)
    .await
    .expect("schema query failed");

    assert!(
        !row.0,
        "segment_list_entries table should not exist after the drop migration"
    );
}

#[sqlx::test]
async fn segment_list_entries_covering_index_is_dropped(pool: PgPool) {
    let row: (bool,) = sqlx::query_as(
        "SELECT EXISTS (
            SELECT 1 FROM pg_indexes
            WHERE schemaname = 'public'
              AND indexname = 'idx_segment_list_entries_covering'
        )",
    )
    .fetch_one(&pool)
    .await
    .expect("schema query failed");

    assert!(
        !row.0,
        "idx_segment_list_entries_covering index should not exist after the drop migration"
    );
}
