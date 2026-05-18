//! Integration tests for ScyllaSegmentStore.
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
// Helper: count rows in segment_list_entries for a given generation
// ---------------------------------------------------------------------------

async fn count_entries(
    client: &ScyllaClient,
    ks: &str,
    segment_id: uuid::Uuid,
    context_type: &str,
    generation: i64,
    list_type: &str,
) -> usize {
    let cql = format!(
        "SELECT COUNT(*) FROM {ks}.segment_list_entries \
         WHERE segment_id = ? AND context_type = ? AND generation = ? AND list_type = ?",
    );
    let rows = client
        .session()
        .query_unpaged(cql, (segment_id, context_type, generation, list_type))
        .await
        .expect("count query");
    let result = rows.into_rows_result().expect("rows result");
    let mut iter = result.rows::<(i64,)>().expect("typed iter");
    let (count,) = iter.next().expect("one row").expect("row ok");
    count as usize
}

// ---------------------------------------------------------------------------
// Helper: read active generation pointer
// ---------------------------------------------------------------------------

async fn active_generation(
    client: &ScyllaClient,
    ks: &str,
    segment_id: uuid::Uuid,
    context_type: &str,
) -> Option<i64> {
    let cql = format!(
        "SELECT active_generation FROM {ks}.segment_list_generations \
         WHERE segment_id = ? AND context_type = ?",
    );
    let rows = client
        .session()
        .query_unpaged(cql, (segment_id, context_type))
        .await
        .expect("generation query");
    let result = rows.into_rows_result().ok()?;
    let mut iter = result.rows::<(i64,)>().ok()?;
    let row = iter.next()?;
    let (active_gen,) = row.ok()?;
    Some(active_gen)
}

// ---------------------------------------------------------------------------
// Task 2 (Red): Tests for set_list_entries
// ---------------------------------------------------------------------------

/// Happy path: set 3 include + 2 exclude entries, verify they all exist.
#[tokio::test]
async fn set_list_entries_happy_path() {
    if !scylla_available().await {
        eprintln!("SKIP: ScyllaDB not available");
        return;
    }

    let ks = format!("stitchd_sle_{}", short_id());
    let client = setup_client(&ks).await;
    let store = ScyllaSegmentStore::new(client.clone());

    let seg_id = SegmentId::new();
    let include = vec!["alice".to_string(), "bob".to_string(), "carol".to_string()];
    let exclude = vec!["dave".to_string(), "eve".to_string()];

    store
        .set_list_entries(seg_id, "user", &include, &exclude)
        .await
        .expect("set_list_entries should succeed");

    let active_gen = active_generation(&client, &ks, seg_id.as_uuid(), "user")
        .await
        .expect("generation pointer should exist");

    let inc_count = count_entries(
        &client,
        &ks,
        seg_id.as_uuid(),
        "user",
        active_gen,
        "include",
    )
    .await;
    let exc_count = count_entries(
        &client,
        &ks,
        seg_id.as_uuid(),
        "user",
        active_gen,
        "exclude",
    )
    .await;

    assert_eq!(inc_count, 3, "should have 3 include entries");
    assert_eq!(exc_count, 2, "should have 2 exclude entries");

    cleanup(&client, &ks).await;
}

/// Second call replaces atomically: only new entries exist under the new generation.
#[tokio::test]
async fn set_list_entries_replaces_atomically() {
    if !scylla_available().await {
        eprintln!("SKIP: ScyllaDB not available");
        return;
    }

    let ks = format!("stitchd_rep_{}", short_id());
    let client = setup_client(&ks).await;
    let store = ScyllaSegmentStore::new(client.clone());

    let seg_id = SegmentId::new();

    // First write: alice + bob in include.
    store
        .set_list_entries(
            seg_id,
            "user",
            &["alice".to_string(), "bob".to_string()],
            &[],
        )
        .await
        .expect("first set_list_entries");

    let gen1 = active_generation(&client, &ks, seg_id.as_uuid(), "user")
        .await
        .expect("gen1 should exist");

    // Second write: only carol.
    store
        .set_list_entries(seg_id, "user", &["carol".to_string()], &[])
        .await
        .expect("second set_list_entries");

    let gen2 = active_generation(&client, &ks, seg_id.as_uuid(), "user")
        .await
        .expect("gen2 should exist");

    // Pointer must have changed (random generation IDs — not necessarily ordered).
    assert_ne!(gen2, gen1, "generation must change on replace");

    // New generation has only carol.
    let inc_count_new =
        count_entries(&client, &ks, seg_id.as_uuid(), "user", gen2, "include").await;
    assert_eq!(
        inc_count_new, 1,
        "new generation should have only 1 include entry"
    );

    cleanup(&client, &ks).await;
}

/// Concurrent set_list_entries on top of an established generation: CAS pointer stays consistent.
#[tokio::test]
async fn concurrent_set_list_entries_only_one_wins() {
    if !scylla_available().await {
        eprintln!("SKIP: ScyllaDB not available");
        return;
    }

    let ks = format!("stitchd_cas_{}", short_id());
    let client = setup_client(&ks).await;
    let store = std::sync::Arc::new(ScyllaSegmentStore::new(client.clone()));

    let seg_id = SegmentId::new();

    // Establish an initial generation so the concurrent writers both see gen=1.
    store
        .set_list_entries(seg_id, "user", &["initial".to_string()], &[])
        .await
        .expect("initial set");

    // Now launch two concurrent set_list_entries calls starting from gen=1.
    // The LWT CAS ensures only one writer's generation becomes active.
    let store_a = store.clone();
    let store_b = store.clone();
    let alice = vec!["alice".to_string()];
    let bob = vec!["bob".to_string()];
    let (res_a, res_b) = tokio::join!(
        store_a.set_list_entries(seg_id, "user", &alice, &[]),
        store_b.set_list_entries(seg_id, "user", &bob, &[]),
    );

    // Both return Ok — one won the CAS, one lost (stale gen to be swept).
    res_a.ok();
    res_b.ok();

    // The active generation pointer must exist and be at least 2.
    let active_gen = active_generation(&client, &ks, seg_id.as_uuid(), "user")
        .await
        .expect("generation should be set");

    assert!(active_gen >= 2, "generation must have advanced from 1");

    // The active generation must contain exactly 1 include entry (one winner).
    let inc_count = count_entries(
        &client,
        &ks,
        seg_id.as_uuid(),
        "user",
        active_gen,
        "include",
    )
    .await;
    assert_eq!(
        inc_count, 1,
        "active generation should have exactly 1 include entry (one CAS winner)"
    );

    cleanup(&client, &ks).await;
}

// ---------------------------------------------------------------------------
// Task 4 (Red): Tests for add_entries / remove_entries
// ---------------------------------------------------------------------------

/// add_entries inserts into the current generation.
#[tokio::test]
async fn add_entries_inserts_into_current_generation() {
    if !scylla_available().await {
        eprintln!("SKIP: ScyllaDB not available");
        return;
    }

    let ks = format!("stitchd_add_{}", short_id());
    let client = setup_client(&ks).await;
    let store = ScyllaSegmentStore::new(client.clone());

    let seg_id = SegmentId::new();

    // Establish a generation via set_list_entries first.
    store
        .set_list_entries(seg_id, "user", &["alice".to_string()], &[])
        .await
        .expect("set_list_entries");

    let active_gen = active_generation(&client, &ks, seg_id.as_uuid(), "user")
        .await
        .expect("active_gen");

    // Now add a new key.
    store
        .add_entries(seg_id, "user", "include", &["bob".to_string()])
        .await
        .expect("add_entries");

    let inc_count = count_entries(
        &client,
        &ks,
        seg_id.as_uuid(),
        "user",
        active_gen,
        "include",
    )
    .await;
    assert_eq!(inc_count, 2, "should have 2 include entries after add");

    cleanup(&client, &ks).await;
}

/// add_entries is idempotent: adding the same key twice keeps count at 1.
#[tokio::test]
async fn add_entries_is_idempotent() {
    if !scylla_available().await {
        eprintln!("SKIP: ScyllaDB not available");
        return;
    }

    let ks = format!("stitchd_idem2_{}", short_id());
    let client = setup_client(&ks).await;
    let store = ScyllaSegmentStore::new(client.clone());

    let seg_id = SegmentId::new();

    store
        .set_list_entries(seg_id, "user", &[], &[])
        .await
        .expect("set_list_entries");

    let active_gen = active_generation(&client, &ks, seg_id.as_uuid(), "user")
        .await
        .expect("active_gen");

    store
        .add_entries(seg_id, "user", "include", &["alice".to_string()])
        .await
        .expect("first add");

    store
        .add_entries(seg_id, "user", "include", &["alice".to_string()])
        .await
        .expect("second add (idempotent)");

    let inc_count = count_entries(
        &client,
        &ks,
        seg_id.as_uuid(),
        "user",
        active_gen,
        "include",
    )
    .await;
    assert_eq!(inc_count, 1, "duplicate add should not create extra rows");

    cleanup(&client, &ks).await;
}

/// remove_entries deletes from the current generation.
#[tokio::test]
async fn remove_entries_deletes_from_current_generation() {
    if !scylla_available().await {
        eprintln!("SKIP: ScyllaDB not available");
        return;
    }

    let ks = format!("stitchd_rem_{}", short_id());
    let client = setup_client(&ks).await;
    let store = ScyllaSegmentStore::new(client.clone());

    let seg_id = SegmentId::new();

    store
        .set_list_entries(
            seg_id,
            "user",
            &["alice".to_string(), "bob".to_string()],
            &[],
        )
        .await
        .expect("set_list_entries");

    let active_gen = active_generation(&client, &ks, seg_id.as_uuid(), "user")
        .await
        .expect("active_gen");

    store
        .remove_entries(seg_id, "user", "include", &["alice".to_string()])
        .await
        .expect("remove_entries");

    let inc_count = count_entries(
        &client,
        &ks,
        seg_id.as_uuid(),
        "user",
        active_gen,
        "include",
    )
    .await;
    assert_eq!(
        inc_count, 1,
        "should have 1 include entry after removing alice"
    );

    cleanup(&client, &ks).await;
}

/// remove_entries on a missing key is a no-op (no error).
#[tokio::test]
async fn remove_entries_noop_on_missing() {
    if !scylla_available().await {
        eprintln!("SKIP: ScyllaDB not available");
        return;
    }

    let ks = format!("stitchd_noop_{}", short_id());
    let client = setup_client(&ks).await;
    let store = ScyllaSegmentStore::new(client.clone());

    let seg_id = SegmentId::new();

    store
        .set_list_entries(seg_id, "user", &["alice".to_string()], &[])
        .await
        .expect("set_list_entries");

    // Remove a key that doesn't exist — should succeed silently.
    let result = store
        .remove_entries(seg_id, "user", "include", &["nonexistent".to_string()])
        .await;
    assert!(
        result.is_ok(),
        "remove of missing key must not error: {result:?}"
    );

    cleanup(&client, &ks).await;
}

// ---------------------------------------------------------------------------
// Task 8 (Red): Tests for get_list_segment_summary
// ---------------------------------------------------------------------------

/// get_list_segment_summary returns correct counts after set_list_entries.
#[tokio::test]
async fn summary_reflects_set_list_entries() {
    if !scylla_available().await {
        eprintln!("SKIP: ScyllaDB not available");
        return;
    }

    let ks = format!("stitchd_sum_{}", short_id());
    let client = setup_client(&ks).await;
    let store = ScyllaSegmentStore::new(client.clone());

    let seg_id = SegmentId::new();
    let include = vec!["a".to_string(), "b".to_string(), "c".to_string()];
    let exclude = vec!["x".to_string(), "y".to_string()];

    store
        .set_list_entries(seg_id, "user", &include, &exclude)
        .await
        .expect("set_list_entries");

    let summary = store
        .get_list_segment_summary(seg_id)
        .await
        .expect("get_list_segment_summary");

    let counts = summary
        .counts
        .get("user")
        .expect("'user' context should be present");
    assert_eq!(counts.include_count, 3, "include_count should be 3");
    assert_eq!(counts.exclude_count, 2, "exclude_count should be 2");

    cleanup(&client, &ks).await;
}

/// get_list_segment_summary returns empty counts for a segment with no data.
#[tokio::test]
async fn summary_empty_for_unknown_segment() {
    if !scylla_available().await {
        eprintln!("SKIP: ScyllaDB not available");
        return;
    }

    let ks = format!("stitchd_unk_{}", short_id());
    let client = setup_client(&ks).await;
    let store = ScyllaSegmentStore::new(client.clone());

    let seg_id = SegmentId::new(); // never populated

    let summary = store
        .get_list_segment_summary(seg_id)
        .await
        .expect("get_list_segment_summary should succeed even for unknown segment");

    assert!(
        summary.counts.is_empty(),
        "no context entries expected for unknown segment"
    );

    cleanup(&client, &ks).await;
}
