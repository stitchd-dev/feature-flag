//! Integration tests for ScyllaClient.
//!
//! These tests require a running ScyllaDB instance. Set `SCYLLA_TEST_URI`
//! (default: `127.0.0.1:9042`) and `SCYLLA_TEST_KEYSPACE` (default: `stitchd_test`)
//! to point at it. Tests are skipped automatically if ScyllaDB is unreachable.

use stitchd_db::scylla::{ScyllaClient, ScyllaConfig, ScyllaError};

fn test_config() -> ScyllaConfig {
    ScyllaConfig {
        uri: std::env::var("SCYLLA_TEST_URI")
            .unwrap_or_else(|_| "127.0.0.1:9042".to_string()),
        keyspace: std::env::var("SCYLLA_TEST_KEYSPACE")
            .unwrap_or_else(|_| "stitchd_test".to_string()),
    }
}

/// Returns true if ScyllaDB is reachable, false otherwise.
/// Used to skip tests when Scylla is not running locally.
async fn scylla_available(config: &ScyllaConfig) -> bool {
    ScyllaClient::connect(config).await.is_ok()
}

/// ScyllaClient::connect returns a usable session when ScyllaDB is running.
#[tokio::test]
async fn connect_returns_usable_session() {
    let config = test_config();
    if !scylla_available(&config).await {
        eprintln!("SKIP: ScyllaDB not available at {}", config.uri);
        return;
    }
    let client = ScyllaClient::connect(&config).await.expect("connect should succeed");
    // Verify the session is usable by running a trivial system query.
    let result = client
        .session()
        .query_unpaged("SELECT cluster_name FROM system.local", &[])
        .await;
    assert!(result.is_ok(), "system.local query should succeed: {result:?}");
}

/// Calling prepare() twice for the same CQL should not increase the cache size.
#[tokio::test]
async fn prepared_statement_cache_deduplicates() {
    let config = test_config();
    if !scylla_available(&config).await {
        eprintln!("SKIP: ScyllaDB not available at {}", config.uri);
        return;
    }
    let client = ScyllaClient::connect(&config).await.expect("connect should succeed");

    let cql = "SELECT cluster_name FROM system.local";

    // First prepare — cache miss, goes to cluster.
    let _stmt1 = client.prepare(cql).await.expect("first prepare should succeed");
    assert_eq!(client.cached_statement_count().await, 1, "one entry after first prepare");

    // Second prepare — cache hit, should NOT add another entry.
    let _stmt2 = client.prepare(cql).await.expect("second prepare should succeed");
    assert_eq!(
        client.cached_statement_count().await,
        1,
        "cache should still have exactly one entry after second prepare of same CQL"
    );
}

/// ScyllaClient::connect returns a typed ScyllaError when the host is unreachable.
#[tokio::test]
async fn connection_failure_returns_typed_error() {
    let config = ScyllaConfig {
        uri: "127.0.0.1:19999".to_string(), // nothing listening here
        keyspace: "irrelevant".to_string(),
    };
    let result = ScyllaClient::connect(&config).await;
    assert!(
        matches!(result, Err(ScyllaError::Connection(_))),
        "expected ScyllaError::Connection, got an unexpected variant"
    );
}
