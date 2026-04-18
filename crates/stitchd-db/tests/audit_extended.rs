/// Integration tests for audit.rs — the PgAuditLogger.
use stitchd_db::{AuditLogger, repository::pg::PgAuditLogger};
use uuid::Uuid;

/// Verify that logging an event with no actor succeeds.
#[sqlx::test(migrations = "./migrations")]
async fn test_audit_log_no_actor(pool: sqlx::PgPool) {
    let logger = PgAuditLogger::new(pool.clone());

    logger
        .log(
            None,
            "test_resource",
            Uuid::new_v4(),
            "create",
            serde_json::json!({ "key": "value" }),
        )
        .await
        .expect("audit log should succeed");

    // Verify row was inserted
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM audit_log")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 1);
}

/// Verify that logging multiple events works.
#[sqlx::test(migrations = "./migrations")]
async fn test_audit_log_multiple_events(pool: sqlx::PgPool) {
    let logger = PgAuditLogger::new(pool.clone());
    let resource_id = Uuid::new_v4();

    for action in ["create", "update", "soft_delete"] {
        logger
            .log(None, "flag", resource_id, action, serde_json::json!({}))
            .await
            .unwrap();
    }

    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM audit_log")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 3);
}

/// Verify that logging with a complex diff JSON works.
#[sqlx::test(migrations = "./migrations")]
async fn test_audit_log_complex_diff(pool: sqlx::PgPool) {
    let logger = PgAuditLogger::new(pool.clone());

    logger
        .log(
            None,
            "segment",
            Uuid::new_v4(),
            "update",
            serde_json::json!({
                "before": { "key": "old", "version": 1 },
                "after": { "key": "new", "version": 2 }
            }),
        )
        .await
        .expect("complex diff should log successfully");
}
