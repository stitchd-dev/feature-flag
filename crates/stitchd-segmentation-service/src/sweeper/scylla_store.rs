//! `SweeperRepository` implementation backed by `ScyllaSegmentStore`.

use chrono::{DateTime, Utc};
use stitchd_core::id::SegmentId;
use stitchd_db::{OrphanedGeneration, scylla::segment::ScyllaSegmentStore};

use super::{SweeperError, SweeperRepository};

// ---------------------------------------------------------------------------
// SweeperRepository impl
// ---------------------------------------------------------------------------

/// Adapt `ScyllaSegmentStore` errors to `SweeperError`.
impl From<stitchd_db::scylla::ScyllaError> for SweeperError {
    fn from(e: stitchd_db::scylla::ScyllaError) -> Self {
        Self::Query(e.to_string())
    }
}

impl SweeperRepository for ScyllaSegmentStore {
    async fn list_orphaned_older_than(
        &self,
        cutoff: DateTime<Utc>,
    ) -> Result<Vec<OrphanedGeneration>, SweeperError> {
        self.list_orphaned_older_than(cutoff)
            .await
            .map_err(SweeperError::from)
    }

    async fn delete_generation_entries(
        &self,
        id: SegmentId,
        context_type: &str,
        generation: i64,
    ) -> Result<(), SweeperError> {
        self.delete_generation_entries(id, context_type, generation)
            .await
            .map_err(SweeperError::from)
    }

    async fn mark_generation_swept(
        &self,
        id: SegmentId,
        context_type: &str,
        generation: i64,
    ) -> Result<(), SweeperError> {
        self.mark_generation_swept(id, context_type, generation)
            .await
            .map_err(SweeperError::from)
    }
}
