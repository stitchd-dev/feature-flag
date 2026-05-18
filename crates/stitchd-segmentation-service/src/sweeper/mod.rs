//! Generation sweeper — background task that deletes orphaned Scylla partitions.
//!
//! After `set_list_entries` performs a generation-swap CAS flip, the superseded
//! generation's partition remains in `segment_list_entries`.  The sweeper
//! periodically queries `segment_list_orphaned_gens` for entries older than the
//! retention window and deletes their corresponding `segment_list_entries` partitions.

use std::time::Duration;

use chrono::{DateTime, Utc};

use stitchd_core::id::SegmentId;
use stitchd_db::OrphanedGeneration;

// ── Error type ────────────────────────────────────────────────────────────────

/// Errors that can occur during a sweep pass.
#[derive(Debug, thiserror::Error)]
pub enum SweeperError {
    /// A Scylla query failed.
    #[error("scylla error: {0}")]
    Query(String),
}

// ── Trait ─────────────────────────────────────────────────────────────────────

/// Abstraction over Scylla operations needed by the sweeper.
///
/// Implemented by [`stitchd_db::scylla::segment::ScyllaSegmentStore`] in
/// production.  Tests use an in-memory mock.
pub trait SweeperRepository: Send + Sync + 'static {
    /// Return all orphaned generations with `orphaned_at < cutoff`.
    fn list_orphaned_older_than(
        &self,
        cutoff: DateTime<Utc>,
    ) -> impl std::future::Future<Output = Result<Vec<OrphanedGeneration>, SweeperError>> + Send;

    /// Delete all `segment_list_entries` rows for `(id, context_type, generation)`.
    fn delete_generation_entries(
        &self,
        id: SegmentId,
        context_type: &str,
        generation: i64,
    ) -> impl std::future::Future<Output = Result<(), SweeperError>> + Send;

    /// Remove the orphaned-gen record after the partition has been deleted.
    fn mark_generation_swept(
        &self,
        id: SegmentId,
        context_type: &str,
        generation: i64,
    ) -> impl std::future::Future<Output = Result<(), SweeperError>> + Send;
}

// ── Sweeper ───────────────────────────────────────────────────────────────────

/// Background sweeper that removes orphaned generation partitions from Scylla.
pub struct GenerationSweeper<R> {
    store: R,
    retention: Duration,
}

impl<R: SweeperRepository> GenerationSweeper<R> {
    /// Create a new sweeper.
    ///
    /// # Arguments
    /// * `store`     — Storage abstraction (Scylla or mock).
    /// * `retention` — How long an orphaned generation must be old before it
    ///                  is eligible for deletion.
    pub fn new(store: R, retention: Duration) -> Self {
        Self { store, retention }
    }

    /// Execute one sweep pass: delete all orphaned generations older than the
    /// retention window.
    pub async fn sweep(&self) -> Result<(), SweeperError> {
        let cutoff = Utc::now()
            - chrono::Duration::from_std(self.retention).unwrap_or(chrono::Duration::hours(24));

        let orphaned = self.store.list_orphaned_older_than(cutoff).await?;
        for o in orphaned {
            // Delete the partition first; only mark swept if delete succeeds.
            self.store
                .delete_generation_entries(o.segment_id, &o.context_type, o.generation)
                .await?;
            // Best-effort cleanup of the orphaned-gens record.
            let _ = self
                .store
                .mark_generation_swept(o.segment_id, &o.context_type, o.generation)
                .await;
        }
        Ok(())
    }

    /// Run the sweeper forever, sleeping `interval` between passes.
    ///
    /// This is the entry point called from `main.rs`.  Errors in individual
    /// passes are logged but do not terminate the loop.
    pub async fn run(self, interval: Duration)
    where
        R: Clone,
    {
        let mut ticker = tokio::time::interval(interval);
        loop {
            ticker.tick().await;
            if let Err(e) = self.sweep().await {
                tracing::warn!("generation sweeper error: {e}");
            }
        }
    }
}

// ── Backend ───────────────────────────────────────────────────────────────────

/// `SweeperRepository` implementation for production Scylla store.
pub mod scylla_store;

// ── Tests ─────────────────────────────────────────────────────────────────────

mod tests;
