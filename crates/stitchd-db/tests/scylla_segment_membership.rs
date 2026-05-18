//! Integration tests for ScyllaSegmentStore membership read paths.
//!
//! Requires a running ScyllaDB. Set `SCYLLA_TEST_URI` (default: `127.0.0.1:9042`).
//! Each test uses a randomly-named keyspace so parallel runs don't conflict.
//! Tests skip gracefully if ScyllaDB is unreachable.

use stitchd_core::id::SegmentId;
use stitchd_db::scylla::{ScyllaClient, ScyllaConfig, migrate, segment::ScyllaSegmentStore};

// ---------------------------------------------------------------------------
// Test helpers
// ---------------------------------------------------------------------------

async fn scylla_available() -> bool {
    let cfg = ScyllaConfig {
        uri: std::env::var("SCYLLA_TEST_URI").unwrap_or_else(|_| "127.0.0.1:9042".to_string()),
        keyspace: "system".to_string(),
    };
    ScyllaClient::connect(&cfg).await.is_ok()
}

async fn setup_client(ks: &str) -> ScyllaClient {
    let config = ScyllaConfig {
        uri: std::env::var("SCYLLA_TEST_URI").unwrap_or_else(|_| "127.0.0.1:9042".to_string()),
        keyspace: ks.to_string(),
    };
    let client = ScyllaClient::connect(&config)
        .await
        .expect("connect to ScyllaDB");
    let migrations_dir = concat!(env!("CARGO_MANIFEST_DIR"), "/scylla-migrations");
    migrate::run(&client, migrations_dir)
        .await
        .expect("apply migrations");
    client
}

async fn cleanup(client: &ScyllaClient, ks: &str) {
    client
        .session()
        .query_unpaged(format!("DROP KEYSPACE IF EXISTS {ks}"), &[])
        .await
        .ok();
}

fn short_id() -> String {
    uuid::Uuid::new_v4().simple().to_string()[..8].to_string()
}

// ---------------------------------------------------------------------------
// Task 6 (Red): Tests for membership read paths
// ---------------------------------------------------------------------------

/// check_membership returns true for a key in the include list.
#[tokio::test]
async fn check_membership_include_only() {
    if !scylla_available().await {
        eprintln!("SKIP: ScyllaDB not available");
        return;
    }

    let ks = format!("stitchd_mem_{}", short_id());
    let client = setup_client(&ks).await;
    let store = ScyllaSegmentStore::new(client.clone());

    let seg_id = SegmentId::new();
    store
        .set_list_entries(seg_id, "user", &["alice".to_string()], &[])
        .await
        .expect("set_list_entries");

    let is_member = store
        .check_membership(seg_id, "user", "alice")
        .await
        .expect("check_membership");

    assert!(is_member, "alice should be a member (in include list)");

    let not_member = store
        .check_membership(seg_id, "user", "bob")
        .await
        .expect("check_membership for bob");

    assert!(!not_member, "bob should not be a member");

    cleanup(&client, &ks).await;
}

/// check_membership: exclude takes precedence over include.
#[tokio::test]
async fn check_membership_exclude_takes_precedence() {
    if !scylla_available().await {
        eprintln!("SKIP: ScyllaDB not available");
        return;
    }

    let ks = format!("stitchd_exc_{}", short_id());
    let client = setup_client(&ks).await;
    let store = ScyllaSegmentStore::new(client.clone());

    let seg_id = SegmentId::new();
    // alice is in BOTH include and exclude — exclude should win.
    store
        .set_list_entries(
            seg_id,
            "user",
            &["alice".to_string()],
            &["alice".to_string()],
        )
        .await
        .expect("set_list_entries");

    let is_member = store
        .check_membership(seg_id, "user", "alice")
        .await
        .expect("check_membership");

    assert!(
        !is_member,
        "exclude takes precedence: alice should NOT be a member"
    );

    cleanup(&client, &ks).await;
}

/// check_membership returns false for a key not in any list.
#[tokio::test]
async fn check_membership_not_in_list() {
    if !scylla_available().await {
        eprintln!("SKIP: ScyllaDB not available");
        return;
    }

    let ks = format!("stitchd_nil_{}", short_id());
    let client = setup_client(&ks).await;
    let store = ScyllaSegmentStore::new(client.clone());

    let seg_id = SegmentId::new();
    store
        .set_list_entries(seg_id, "user", &["alice".to_string()], &[])
        .await
        .expect("set_list_entries");

    let is_member = store
        .check_membership(seg_id, "user", "unknown-user")
        .await
        .expect("check_membership");

    assert!(!is_member, "unknown-user should not be a member");

    cleanup(&client, &ks).await;
}

/// batch_check_membership: multiple keys, one member one not.
#[tokio::test]
async fn batch_check_membership_multiple_keys() {
    if !scylla_available().await {
        eprintln!("SKIP: ScyllaDB not available");
        return;
    }

    let ks = format!("stitchd_batch_{}", short_id());
    let client = setup_client(&ks).await;
    let store = ScyllaSegmentStore::new(client.clone());

    let seg_id = SegmentId::new();
    store
        .set_list_entries(
            seg_id,
            "user",
            &["alice".to_string(), "carol".to_string()],
            &[],
        )
        .await
        .expect("set_list_entries");

    let keys = vec!["alice".to_string(), "bob".to_string(), "carol".to_string()];
    let memberships = store
        .batch_check_membership(seg_id, "user", &keys)
        .await
        .expect("batch_check_membership");

    assert_eq!(
        *memberships.get("alice").expect("alice"),
        true,
        "alice is a member"
    );
    assert_eq!(
        *memberships.get("bob").expect("bob"),
        false,
        "bob is not a member"
    );
    assert_eq!(
        *memberships.get("carol").expect("carol"),
        true,
        "carol is a member"
    );

    cleanup(&client, &ks).await;
}
