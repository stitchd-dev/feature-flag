//! Integration tests for the ScyllaDB migration applier.
//!
//! Requires a running ScyllaDB. Set `SCYLLA_TEST_URI` (default `127.0.0.1:9042`).
//! Uses a randomly-named keyspace per test run so parallel tests don't conflict.

use stitchd_db::scylla::{ScyllaClient, ScyllaConfig, migrate};

fn test_config_with_keyspace(keyspace: &str) -> ScyllaConfig {
    ScyllaConfig {
        uri: std::env::var("SCYLLA_TEST_URI").unwrap_or_else(|_| "127.0.0.1:9042".to_string()),
        keyspace: keyspace.to_string(),
    }
}

async fn scylla_available() -> bool {
    let cfg = ScyllaConfig {
        uri: std::env::var("SCYLLA_TEST_URI").unwrap_or_else(|_| "127.0.0.1:9042".to_string()),
        keyspace: "system".to_string(),
    };
    ScyllaClient::connect(&cfg).await.is_ok()
}

/// Migration applier runs .cql files in numeric order.
#[tokio::test]
async fn migrations_apply_in_order() {
    if !scylla_available().await {
        eprintln!("SKIP: ScyllaDB not available");
        return;
    }
    let ks = format!(
        "stitchd_mig_{}",
        &uuid::Uuid::new_v4().simple().to_string()[..8]
    );
    let config = test_config_with_keyspace(&ks);
    let client = ScyllaClient::connect(&config).await.expect("connect");

    let migrations_dir = concat!(env!("CARGO_MANIFEST_DIR"), "/scylla-migrations");
    migrate::run(&client, migrations_dir)
        .await
        .expect("migrations should apply");

    // Verify segment_list_entries table was created (migration 0002).
    let rows = client
        .session()
        .query_unpaged(
            format!(
                "SELECT table_name FROM system_schema.tables \
                 WHERE keyspace_name = '{ks}' AND table_name = 'segment_list_entries'"
            ),
            &[],
        )
        .await
        .expect("schema query");
    let count = rows.into_rows_result().expect("rows result").rows_num();
    assert!(
        count > 0,
        "segment_list_entries table should exist after migrations"
    );

    // Cleanup
    client
        .session()
        .query_unpaged(format!("DROP KEYSPACE IF EXISTS {ks}"), &[])
        .await
        .ok();
}

/// Running migrations twice is idempotent.
#[tokio::test]
async fn migrations_are_idempotent() {
    if !scylla_available().await {
        eprintln!("SKIP: ScyllaDB not available");
        return;
    }
    let ks = format!(
        "stitchd_idem_{}",
        &uuid::Uuid::new_v4().simple().to_string()[..8]
    );
    let config = test_config_with_keyspace(&ks);
    let client = ScyllaClient::connect(&config).await.expect("connect");

    let migrations_dir = concat!(env!("CARGO_MANIFEST_DIR"), "/scylla-migrations");
    migrate::run(&client, migrations_dir)
        .await
        .expect("first run");
    migrate::run(&client, migrations_dir)
        .await
        .expect("second run should be idempotent");

    // Cleanup
    client
        .session()
        .query_unpaged(format!("DROP KEYSPACE IF EXISTS {ks}"), &[])
        .await
        .ok();
}

/// Applied migration versions are tracked in `scylla_migrations` table.
#[tokio::test]
async fn applied_versions_are_tracked() {
    if !scylla_available().await {
        eprintln!("SKIP: ScyllaDB not available");
        return;
    }
    let ks = format!(
        "stitchd_trk_{}",
        &uuid::Uuid::new_v4().simple().to_string()[..8]
    );
    let config = test_config_with_keyspace(&ks);
    let client = ScyllaClient::connect(&config).await.expect("connect");

    let migrations_dir = concat!(env!("CARGO_MANIFEST_DIR"), "/scylla-migrations");
    migrate::run(&client, migrations_dir)
        .await
        .expect("migrations should apply");

    let rows = client
        .session()
        .query_unpaged(format!("SELECT version FROM {ks}.scylla_migrations"), &[])
        .await
        .expect("query scylla_migrations table");

    let count = rows.into_rows_result().expect("rows").rows_num();
    assert!(
        count > 0,
        "scylla_migrations should have at least one row after applying"
    );

    // Cleanup
    client
        .session()
        .query_unpaged(format!("DROP KEYSPACE IF EXISTS {ks}"), &[])
        .await
        .ok();
}
