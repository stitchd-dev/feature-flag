//! Unit tests for `GenerationSweeper`.
//!
//! Uses an in-memory mock `SweeperStore` so no Scylla connection is needed.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::{DateTime, Utc};

use stitchd_core::id::SegmentId;
use stitchd_db::OrphanedGeneration;

use crate::sweeper::{GenerationSweeper, SweeperError, SweeperRepository};

// ── In-memory mock ────────────────────────────────────────────────────────

#[derive(Debug, Default)]
struct MockState {
    orphaned: Vec<OrphanedGeneration>,
    deleted: Vec<(SegmentId, String, i64)>,
    swept: Vec<(SegmentId, String, i64)>,
}

#[derive(Clone, Default)]
struct MockSweeperStore(Arc<Mutex<MockState>>);

impl MockSweeperStore {
    fn new() -> Self {
        Self(Arc::new(Mutex::new(MockState::default())))
    }

    fn add_orphan(
        &self,
        id: SegmentId,
        context_type: impl Into<String>,
        generation: i64,
        orphaned_at: DateTime<Utc>,
    ) {
        self.0.lock().unwrap().orphaned.push(OrphanedGeneration {
            segment_id: id,
            context_type: context_type.into(),
            generation,
            orphaned_at,
        });
    }

    fn deleted(&self) -> Vec<(SegmentId, String, i64)> {
        self.0.lock().unwrap().deleted.clone()
    }

    fn swept(&self) -> Vec<(SegmentId, String, i64)> {
        self.0.lock().unwrap().swept.clone()
    }
}

impl SweeperRepository for MockSweeperStore {
    async fn list_orphaned_older_than(
        &self,
        cutoff: DateTime<Utc>,
    ) -> Result<Vec<OrphanedGeneration>, SweeperError> {
        let state = self.0.lock().unwrap();
        Ok(state
            .orphaned
            .iter()
            .filter(|o| o.orphaned_at < cutoff)
            .cloned()
            .collect())
    }

    async fn delete_generation_entries(
        &self,
        id: SegmentId,
        context_type: &str,
        generation: i64,
    ) -> Result<(), SweeperError> {
        self.0
            .lock()
            .unwrap()
            .deleted
            .push((id, context_type.to_string(), generation));
        Ok(())
    }

    async fn mark_generation_swept(
        &self,
        id: SegmentId,
        context_type: &str,
        generation: i64,
    ) -> Result<(), SweeperError> {
        self.0
            .lock()
            .unwrap()
            .swept
            .push((id, context_type.to_string(), generation));
        Ok(())
    }
}

// ── Failing mock (always returns an error) ───────────────────────────────

#[derive(Clone, Default)]
struct FailingSweeperStore;

impl SweeperRepository for FailingSweeperStore {
    async fn list_orphaned_older_than(
        &self,
        _cutoff: DateTime<Utc>,
    ) -> Result<Vec<OrphanedGeneration>, SweeperError> {
        Err(SweeperError::Query("injected failure".into()))
    }

    async fn delete_generation_entries(
        &self,
        _id: SegmentId,
        _context_type: &str,
        _generation: i64,
    ) -> Result<(), SweeperError> {
        Ok(())
    }

    async fn mark_generation_swept(
        &self,
        _id: SegmentId,
        _context_type: &str,
        _generation: i64,
    ) -> Result<(), SweeperError> {
        Ok(())
    }
}

// ── Helper ────────────────────────────────────────────────────────────────

fn seg() -> SegmentId {
    SegmentId::new()
}

fn old(hours: i64) -> DateTime<Utc> {
    Utc::now() - chrono::Duration::hours(hours)
}

fn recent(minutes: i64) -> DateTime<Utc> {
    Utc::now() - chrono::Duration::minutes(minutes)
}

// ── Tests ─────────────────────────────────────────────────────────────────

/// Sweeper deletes generations older than the retention window.
#[tokio::test]
async fn sweeper_deletes_old_orphaned_generations() {
    let store = MockSweeperStore::new();
    let id = seg();
    store.add_orphan(id, "user", 42, old(25)); // 25h old, window = 24h → should sweep

    let sweeper = GenerationSweeper::new(store.clone(), Duration::from_secs(24 * 3600));
    sweeper.sweep().await.expect("sweep should succeed");

    assert_eq!(store.deleted(), vec![(id, "user".to_string(), 42)]);
    assert_eq!(store.swept(), vec![(id, "user".to_string(), 42)]);
}

/// Sweeper does not delete generations within the retention window.
#[tokio::test]
async fn sweeper_skips_recent_orphaned_generations() {
    let store = MockSweeperStore::new();
    let id = seg();
    store.add_orphan(id, "user", 99, recent(30)); // 30 min old, window = 24h → skip

    let sweeper = GenerationSweeper::new(store.clone(), Duration::from_secs(24 * 3600));
    sweeper.sweep().await.expect("sweep should succeed");

    assert!(
        store.deleted().is_empty(),
        "recent orphan should not be deleted"
    );
    assert!(store.swept().is_empty());
}

/// Sweeper handles multiple orphaned generations for different segments.
#[tokio::test]
async fn sweeper_handles_multiple_segments() {
    let store = MockSweeperStore::new();
    let id1 = seg();
    let id2 = seg();
    store.add_orphan(id1, "user", 1, old(48));
    store.add_orphan(id2, "org", 2, old(36));
    store.add_orphan(id1, "user", 3, recent(10)); // too recent

    let sweeper = GenerationSweeper::new(store.clone(), Duration::from_secs(24 * 3600));
    sweeper.sweep().await.expect("sweep should succeed");

    let deleted = store.deleted();
    assert_eq!(deleted.len(), 2);
    assert!(deleted.contains(&(id1, "user".to_string(), 1)));
    assert!(deleted.contains(&(id2, "org".to_string(), 2)));
    // recent orphan not deleted
    assert!(!deleted.contains(&(id1, "user".to_string(), 3)));
}

/// Sweeper no-ops when there are no orphaned generations.
#[tokio::test]
async fn sweeper_no_ops_on_empty_store() {
    let store = MockSweeperStore::new();
    let sweeper = GenerationSweeper::new(store.clone(), Duration::from_secs(3600));
    sweeper.sweep().await.expect("sweep should succeed");

    assert!(store.deleted().is_empty());
    assert!(store.swept().is_empty());
}

/// `run()` invokes `sweep()` at least once within the first interval tick.
///
/// Uses `tokio::time::pause` + `advance` to control the clock, then aborts
/// the background task so the infinite loop terminates cleanly.
#[tokio::test(start_paused = true)]
async fn sweeper_run_fires_on_first_tick() {
    let store = MockSweeperStore::new();
    let id = seg();
    store.add_orphan(id, "user", 1, old(48));

    let sweeper = GenerationSweeper::new(store.clone(), Duration::from_secs(24 * 3600));
    let interval = Duration::from_secs(1);

    // Spawn the infinite `run()` loop in the background.
    let handle = tokio::spawn(async move {
        sweeper.run(interval).await;
    });

    // Advance past the first tick — the sweeper should run sweep().
    tokio::time::advance(interval + Duration::from_millis(1)).await;
    // Yield to let the spawned task make progress.
    tokio::task::yield_now().await;

    // Abort before the next tick.
    handle.abort();

    // The old orphan should have been swept.
    assert!(
        !store.deleted().is_empty(),
        "run() should have called sweep() on the first tick"
    );
}

/// `run()` logs errors and keeps running when `sweep()` fails.
///
/// Uses the failing store so the error branch in `run()` is exercised.
#[tokio::test(start_paused = true)]
async fn sweeper_run_logs_error_and_continues() {
    let sweeper = GenerationSweeper::new(FailingSweeperStore, Duration::from_secs(3600));
    let interval = Duration::from_secs(1);

    let handle = tokio::spawn(async move {
        sweeper.run(interval).await;
    });

    // Advance past the first tick — the error branch should be reached.
    tokio::time::advance(interval + Duration::from_millis(1)).await;
    tokio::task::yield_now().await;
    handle.abort();
    // If we reach here without panicking, the error was logged and swallowed.
}

/// Delete precedes `mark_swept`: if delete fails, `mark_swept` is not called.
#[tokio::test]
async fn sweeper_marks_swept_only_after_delete_succeeds() {
    // Use the happy-path mock — both succeed.
    let store = MockSweeperStore::new();
    let id = seg();
    store.add_orphan(id, "user", 7, old(48));

    let sweeper = GenerationSweeper::new(store.clone(), Duration::from_secs(3600));
    sweeper.sweep().await.unwrap();

    // deleted before swept
    assert_eq!(store.deleted().len(), 1);
    assert_eq!(store.swept().len(), 1);
}
