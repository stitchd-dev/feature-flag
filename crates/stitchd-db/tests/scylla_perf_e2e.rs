//! Large-scale performance test: 40 segments × 1M entries each (10M unique keys).
//!
//! ## What this test measures
//!
//! ### Write performance
//! Seeds 10M unique keys in memory, partitioned into overlapping 1M-key windows
//! (stride 250k) so each unique key appears in ~4 segments on average.
//!
//! Writes all 40 segments **concurrently** via `tokio::spawn`.  Within each
//! segment, entries are written with `PARALLEL_TASKS` concurrent CQL tasks so
//! the single-node Scylla is saturated rather than serialised.
//!
//! Reports: per-segment write time (p50/p90/p99), total wall time, aggregate
//! throughput (entries/sec).
//!
//! ### Read (lookup) performance
//! After all writes settle, fires 5 000 random point-lookups across all 40
//! segments and reports p50 / p90 / p99 latency.
//!
//! ## Prerequisites
//! Running ScyllaDB node.  Set `SCYLLA_TEST_URI` (default `127.0.0.1:9042`).
//! Test skips gracefully if Scylla is unreachable.
//!
//! ## Expected runtime
//! 5–20 min on a developer laptop (single-node Scylla, 40 concurrent segments).

use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::task::JoinSet;

use stitchd_core::id::SegmentId;
use stitchd_db::scylla::{ScyllaClient, ScyllaConfig, migrate, segment::ScyllaSegmentStore};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const UNIQUE_KEYS: usize = 10_000_000;
const SEGMENTS: usize = 40;
const ENTRIES_PER_SEGMENT: usize = 1_000_000;
/// Keys stride between segment windows → 250k, giving 750k overlap between neighbours.
const STRIDE: usize = UNIQUE_KEYS / SEGMENTS;
/// Keys per CQL task (one `execute_unpaged` per key within the task).
const KEYS_PER_TASK: usize = 1_000;
/// Max concurrent CQL tasks running per segment at once.
const PARALLEL_TASKS: usize = 32;
/// Random point-lookups for the read benchmark.
const LOOKUP_SAMPLES: usize = 5_000;

// ---------------------------------------------------------------------------
// Helpers
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

fn percentile(sorted: &[Duration], pct: f64) -> Duration {
    let idx = ((sorted.len() as f64) * pct / 100.0) as usize;
    sorted[idx.min(sorted.len() - 1)]
}

// ---------------------------------------------------------------------------
// Parallel raw writer
//
// Bypasses the per-key loop in `ScyllaSegmentStore::add_entries` and fires
// PARALLEL_TASKS concurrent `execute_unpaged` streams so the local Scylla
// node is saturated.  Uses the public `ScyllaClient` session directly.
// ---------------------------------------------------------------------------

async fn write_keys_parallel(
    client: Arc<ScyllaClient>,
    ks: &str,
    seg_uuid: uuid::Uuid,
    context_type: &str,
    generation: i64,
    keys: Vec<String>,
) {
    let insert_cql = format!(
        "INSERT INTO {ks}.segment_list_entries \
         (segment_id, context_type, generation, list_type, entry_key) \
         VALUES (?, ?, ?, ?, ?)"
    );
    let stmt = Arc::new(
        client
            .prepare(&insert_cql)
            .await
            .expect("prepare insert"),
    );
    let ctx = context_type.to_string();

    // Split into chunks, then dispatch up to PARALLEL_TASKS chunks concurrently.
    let chunks: Vec<Vec<String>> = keys
        .chunks(KEYS_PER_TASK)
        .map(|c| c.to_vec())
        .collect();

    let mut idx = 0;
    while idx < chunks.len() {
        let window_end = (idx + PARALLEL_TASKS).min(chunks.len());
        let mut set = JoinSet::new();

        for chunk in chunks[idx..window_end].iter().cloned() {
            let stmt = Arc::clone(&stmt);
            let client = Arc::clone(&client);
            let ctx = ctx.clone();

            set.spawn(async move {
                for key in &chunk {
                    client
                        .session()
                        .execute_unpaged(
                            &*stmt,
                            (seg_uuid, ctx.as_str(), generation, "include", key.as_str()),
                        )
                        .await
                        .expect("insert key");
                }
            });
        }

        while set.join_next().await.is_some() {}
        idx = window_end;
    }
}

// ---------------------------------------------------------------------------
// Single-segment writer
//
// Uses the store's `set_list_entries` for the generation-pointer CAS
// (correctness), then the low-level parallel writer for the bulk entries.
// ---------------------------------------------------------------------------

async fn write_segment(
    store: Arc<ScyllaSegmentStore>,
    client: Arc<ScyllaClient>,
    ks: String,
    seg_id: SegmentId,
    keys: Arc<Vec<String>>,
    start: usize,
) -> Duration {
    let t0 = Instant::now();
    let seg_uuid = seg_id.as_uuid();

    // First small batch through the store to create the generation + CAS pointer.
    let seed_size = KEYS_PER_TASK;
    let seed: Vec<String> = (start..start + seed_size)
        .map(|i| keys[i % UNIQUE_KEYS].clone())
        .collect();
    store
        .set_list_entries(seg_id, "user", &seed, &[])
        .await
        .expect("set_list_entries");

    // Fetch the active generation that was just created.
    let gen_cql = format!(
        "SELECT active_generation FROM {ks}.segment_list_generations \
         WHERE segment_id = ? AND context_type = ?",
    );
    let gen_stmt = client.prepare(&gen_cql).await.expect("prepare gen query");
    let gen_rows = client
        .session()
        .execute_unpaged(&gen_stmt, (seg_uuid, "user"))
        .await
        .expect("fetch generation");
    let active_generation: i64 = gen_rows
        .into_rows_result()
        .expect("rows result")
        .rows::<(i64,)>()
        .expect("typed rows")
        .next()
        .expect("one row")
        .expect("row ok")
        .0;

    // Write remaining entries (1M − seed_size) in parallel.
    let rest: Vec<String> = (start + seed_size..start + ENTRIES_PER_SEGMENT)
        .map(|i| keys[i % UNIQUE_KEYS].clone())
        .collect();

    write_keys_parallel(
        Arc::clone(&client),
        &ks,
        seg_uuid,
        "user",
        active_generation,
        rest,
    )
    .await;

    t0.elapsed()
}

// ---------------------------------------------------------------------------
// The test
// ---------------------------------------------------------------------------

/// 40-segment × 1M-entry bulk load + p50/p90/p99 write and lookup benchmark.
#[tokio::test]
async fn perf_40_segments_1m_entries_each() {
    if !scylla_available().await {
        eprintln!("SKIP: ScyllaDB not available");
        return;
    }

    // ── 1. Build 10M unique key pool ─────────────────────────────────────────

    eprintln!("[perf] Generating {UNIQUE_KEYS} unique keys …");
    let t_keygen = Instant::now();
    // "k{i:010}" → 12 bytes each → ~120 MB heap for 10M strings.
    let keys: Arc<Vec<String>> =
        Arc::new((0..UNIQUE_KEYS).map(|i| format!("k{i:010}")).collect());
    eprintln!(
        "[perf] Key pool ready in {:.1}s",
        t_keygen.elapsed().as_secs_f32()
    );

    // ── 2. Setup ──────────────────────────────────────────────────────────────

    let ks = format!("stitchd_perf_{}", short_id());
    eprintln!("[perf] Keyspace: {ks}");
    let client = Arc::new(setup_client(&ks).await);
    let store = Arc::new(ScyllaSegmentStore::new((*client).clone()));
    let seg_ids: Vec<SegmentId> = (0..SEGMENTS).map(|_| SegmentId::new()).collect();

    // ── 3. Write all 40 segments concurrently ────────────────────────────────

    eprintln!(
        "[perf] Writing {SEGMENTS} segments × {} entries \
         ({PARALLEL_TASKS} concurrent CQL tasks/seg) …",
        ENTRIES_PER_SEGMENT
    );
    let t_wall = Instant::now();

    let mut outer_set: JoinSet<(usize, Duration)> = JoinSet::new();
    for (i, &seg_id) in seg_ids.iter().enumerate() {
        let store = Arc::clone(&store);
        let client = Arc::clone(&client);
        let keys = Arc::clone(&keys);
        let ks = ks.clone();
        let start = (i * STRIDE) % UNIQUE_KEYS;

        outer_set.spawn(async move {
            let elapsed = write_segment(store, client, ks, seg_id, keys, start).await;
            (i, elapsed)
        });
    }

    let mut seg_times: Vec<Duration> = Vec::with_capacity(SEGMENTS);
    while let Some(res) = outer_set.join_next().await {
        let (i, elapsed) = res.expect("segment write task");
        eprintln!("[perf]   seg {:02} ✓  {:.1}s", i, elapsed.as_secs_f32());
        seg_times.push(elapsed);
    }

    let total_wall = t_wall.elapsed();
    let total_entries = SEGMENTS * ENTRIES_PER_SEGMENT;
    let throughput = total_entries as f64 / total_wall.as_secs_f64();

    seg_times.sort();
    let wp50 = percentile(&seg_times, 50.0);
    let wp90 = percentile(&seg_times, 90.0);
    let wp99 = percentile(&seg_times, 99.0);

    eprintln!("\n╔══ WRITE PERFORMANCE ════════════════════════════════╗");
    eprintln!(
        "║  Total entries      : {:.0}M ({SEGMENTS} segments × 1M)",
        total_entries as f64 / 1_000_000.0
    );
    eprintln!("║  Unique keys (pool) : {}M  (each key in ~4 segs)", UNIQUE_KEYS / 1_000_000);
    eprintln!("║  Total wall time    : {:.1}s", total_wall.as_secs_f32());
    eprintln!("║  Aggregate throughput: {throughput:.0} entries/sec");
    eprintln!("║  Per-segment write time:");
    eprintln!("║    p50 : {:.2}s", wp50.as_secs_f32());
    eprintln!("║    p90 : {:.2}s", wp90.as_secs_f32());
    eprintln!("║    p99 : {:.2}s", wp99.as_secs_f32());
    eprintln!("╚═════════════════════════════════════════════════════╝\n");

    // Sanity: each segment must have taken at least 500ms (something was actually written).
    assert!(
        wp50 > Duration::from_millis(500),
        "p50 write time {wp50:?} suspiciously fast — data may not have been written"
    );

    // ── 4. Lookup benchmark ──────────────────────────────────────────────────

    eprintln!("[perf] Running {LOOKUP_SAMPLES} random point-lookups …");

    // Simple LCG for reproducible random samples.
    let mut lcg: u64 = 0xDEAD_BEEF_1234_5678;
    let samples: Vec<(usize, String)> = (0..LOOKUP_SAMPLES)
        .map(|_| {
            lcg = lcg
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            let seg_idx = (lcg >> 33) as usize % SEGMENTS;
            let key_idx = ((lcg & 0xFFFF_FFFF) as usize * UNIQUE_KEYS) >> 32;
            (seg_idx, format!("k{key_idx:010}"))
        })
        .collect();

    let mut latencies: Vec<Duration> = Vec::with_capacity(LOOKUP_SAMPLES);
    for (seg_idx, key) in &samples {
        let t0 = Instant::now();
        let _ = store
            .check_membership(seg_ids[*seg_idx], "user", key)
            .await
            .expect("check_membership");
        latencies.push(t0.elapsed());
    }

    latencies.sort();
    let lmin = latencies.first().copied().unwrap_or_default();
    let lp50 = percentile(&latencies, 50.0);
    let lp90 = percentile(&latencies, 90.0);
    let lp99 = percentile(&latencies, 99.0);
    let lmax = latencies.last().copied().unwrap_or_default();

    eprintln!("╔══ LOOKUP PERFORMANCE ═══════════════════════════════╗");
    eprintln!("║  Samples : {LOOKUP_SAMPLES} across {SEGMENTS} segments (post 40M write load)");
    eprintln!("║  min     : {lmin:?}");
    eprintln!("║  p50     : {lp50:?}");
    eprintln!("║  p90     : {lp90:?}");
    eprintln!("║  p99     : {lp99:?}");
    eprintln!("║  max     : {lmax:?}");
    eprintln!("╚═════════════════════════════════════════════════════╝\n");

    // p99 must stay under 100ms even at 40M-entry scale.
    assert!(
        lp99 < Duration::from_millis(100),
        "lookup p99 {lp99:?} exceeded 100ms SLO after 40M-entry write load"
    );

    // ── 5. Cleanup ────────────────────────────────────────────────────────────

    eprintln!("[perf] Dropping keyspace {ks} …");
    cleanup(&client, &ks).await;
    eprintln!("[perf] Done.");
}
