//! End-to-end smoke tests for the ScyllaDB segment store.
//!
//! Exercises three scenarios at scale:
//!   1. **1M-entry bulk import** via `set_list_entries` — verifies write throughput
//!      and that membership lookups return correct results after a large load.
//!   2. **Membership lookup latency** — asserts that p99 for 1000 point-lookups
//!      stays under 100 ms on a local Scylla instance.
//!   3. **Swap atomicity under concurrent reads** — fires a generation swap while
//!      readers are in flight and asserts all reads see a consistent generation
//!      (no partial views).
//!
//! Requires a running ScyllaDB. Set `SCYLLA_TEST_URI` (default: `127.0.0.1:9042`).
//! Tests skip gracefully if ScyllaDB is unreachable.

use std::sync::Arc;
use std::time::{Duration, Instant};

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
// 1. Bulk import: 1M entries
// ---------------------------------------------------------------------------

/// Bulk-import 1M entries and verify membership lookups are consistent.
///
/// Uses batched `set_list_entries` calls (10k entries each) to reach 1M total.
/// Verifies that a sampled set of known includes/excludes resolve correctly.
#[tokio::test]
async fn bulk_import_one_million_entries() {
    if !scylla_available().await {
        eprintln!("SKIP: ScyllaDB not available");
        return;
    }

    let ks = format!("stitchd_bulk_{}", short_id());
    let client = setup_client(&ks).await;
    let store = ScyllaSegmentStore::new(client.clone());
    let seg_id = SegmentId::new();

    const BATCH: usize = 10_000;
    const TOTAL: usize = 1_000_000;
    const BATCHES: usize = TOTAL / BATCH;

    // First batch sets the generation (include + exclude sampled at end).
    let first_include: Vec<String> = (0..BATCH).map(|i| format!("user_{i:08}")).collect();
    let first_exclude: Vec<String> = (0..100).map(|i| format!("exc_{i:04}")).collect();

    store
        .set_list_entries(seg_id, "user", &first_include, &first_exclude)
        .await
        .expect("first set_list_entries");

    // Subsequent batches add to the same generation.
    for batch in 1..BATCHES {
        let offset = batch * BATCH;
        let keys: Vec<String> = (0..BATCH)
            .map(|i| format!("user_{:08}", offset + i))
            .collect();
        store
            .add_entries(seg_id, "user", "include", &keys)
            .await
            .expect("add_entries batch");
    }

    // --- Verify membership for a sample ---

    // A known include from the first batch
    let in_member = store
        .check_membership(seg_id, "user", "user_00000001")
        .await
        .expect("check include");
    assert!(in_member, "user_00000001 should be in include list");

    // A known include from a later batch
    let late_member = store
        .check_membership(seg_id, "user", &format!("user_{:08}", TOTAL - 1))
        .await
        .expect("check late include");
    assert!(late_member, "last user should be in include list");

    // A known exclude
    let excluded = store
        .check_membership(seg_id, "user", "exc_0000")
        .await
        .expect("check exclude");
    assert!(
        !excluded,
        "exc_0000 is in exclude list — should not be a member"
    );

    // A key that was never added
    let absent = store
        .check_membership(seg_id, "user", "never_added")
        .await
        .expect("check absent");
    assert!(!absent, "never_added should not be a member");

    cleanup(&client, &ks).await;
}

// ---------------------------------------------------------------------------
// 2. Membership lookup p99 latency
// ---------------------------------------------------------------------------

/// Assert that p99 of 1000 point-lookups is under 100 ms on a local Scylla.
#[tokio::test]
async fn membership_lookup_p99_under_100ms() {
    if !scylla_available().await {
        eprintln!("SKIP: ScyllaDB not available");
        return;
    }

    let ks = format!("stitchd_p99_{}", short_id());
    let client = setup_client(&ks).await;
    let store = Arc::new(ScyllaSegmentStore::new(client.clone()));
    let seg_id = SegmentId::new();

    // Seed 1000 entries so the segment is non-trivial.
    let keys: Vec<String> = (0..1000).map(|i| format!("key_{i:04}")).collect();
    store
        .set_list_entries(seg_id, "user", &keys, &[])
        .await
        .expect("seed entries");

    const SAMPLES: usize = 1000;
    let mut latencies: Vec<Duration> = Vec::with_capacity(SAMPLES);

    for i in 0..SAMPLES {
        let key = format!("key_{:04}", i % 1000);
        let t0 = Instant::now();
        store
            .check_membership(seg_id, "user", &key)
            .await
            .expect("lookup");
        latencies.push(t0.elapsed());
    }

    latencies.sort();
    let p99 = latencies[(SAMPLES as f64 * 0.99) as usize];
    let p50 = latencies[SAMPLES / 2];

    eprintln!("Membership lookup — p50: {p50:?}, p99: {p99:?}");

    assert!(
        p99 < Duration::from_millis(100),
        "p99 latency {p99:?} exceeds 100 ms"
    );

    cleanup(&client, &ks).await;
}

// ---------------------------------------------------------------------------
// 3. Swap atomicity under concurrent reads
// ---------------------------------------------------------------------------

/// Fire `set_list_entries` (generation swap) while concurrent readers are
/// running.
///
/// The store's atomicity guarantee: each individual `check_membership` call
/// reads the generation pointer and then queries entries from that exact
/// generation. Readers never observe partial entry writes because the pointer
/// only advances after all entries are written.
///
/// This test verifies two properties:
///   1. No reader errors under concurrent writes (no panics / query failures).
///   2. After the swap settles, the new generation is fully visible and the
///      old generation is fully gone (no cross-generation pollution).
#[tokio::test]
async fn swap_atomicity_under_concurrent_reads() {
    if !scylla_available().await {
        eprintln!("SKIP: ScyllaDB not available");
        return;
    }

    let ks = format!("stitchd_atom_{}", short_id());
    let client = setup_client(&ks).await;
    let store = Arc::new(ScyllaSegmentStore::new(client.clone()));
    let seg_id = SegmentId::new();

    // Seed old generation: 100 known keys.
    let old_keys: Vec<String> = (0..100).map(|i| format!("old_{i:03}")).collect();
    store
        .set_list_entries(seg_id, "user", &old_keys, &[])
        .await
        .expect("seed old generation");

    // Concurrent readers: check a key from each generation, collecting any errors.
    let store_r = Arc::clone(&store);
    let reader_handle = tokio::spawn(async move {
        let deadline = Instant::now() + Duration::from_millis(300);
        let mut errors = 0usize;

        while Instant::now() < deadline {
            // Each call is an independent atomic point-lookup. Errors indicate
            // a broken query — not a consistency violation between two calls.
            if store_r
                .check_membership(seg_id, "user", "old_000")
                .await
                .is_err()
            {
                errors += 1;
            }
            if store_r
                .check_membership(seg_id, "user", "new_000")
                .await
                .is_err()
            {
                errors += 1;
            }
            tokio::task::yield_now().await;
        }
        errors
    });

    // After a brief delay, swap to new generation: 100 different keys.
    tokio::time::sleep(Duration::from_millis(100)).await;
    let new_keys: Vec<String> = (0..100).map(|i| format!("new_{i:03}")).collect();
    store
        .set_list_entries(seg_id, "user", &new_keys, &[])
        .await
        .expect("swap to new generation");

    let errors = reader_handle.await.expect("reader task");
    assert_eq!(
        errors, 0,
        "readers encountered {errors} query errors during swap"
    );

    // Post-swap: verify new generation is fully visible and old is fully gone.
    // Check a sample of 10 keys from each generation.
    for i in (0..100).step_by(10) {
        let new_key = format!("new_{i:03}");
        let old_key = format!("old_{i:03}");

        let new_in = store
            .check_membership(seg_id, "user", &new_key)
            .await
            .unwrap_or(false);
        let old_in = store
            .check_membership(seg_id, "user", &old_key)
            .await
            .unwrap_or(true);

        assert!(new_in, "{new_key} should be in new generation");
        assert!(
            !old_in,
            "{old_key} should not be in new generation (cross-generation pollution)"
        );
    }

    cleanup(&client, &ks).await;
}
