//! ScyllaDB-backed list-segment store.
//!
//! Implements generation-swap full-replace with LWT CAS pointer flip, diff-based
//! add/remove, membership reads, and count-based summary queries.
//!
//! # Schema
//! - `segment_list_entries`: (segment_id, context_type, generation) → (list_type, entry_key)
//! - `segment_list_generations`: (segment_id, context_type) → active_generation
//! - `segment_list_summary`: (segment_id, context_type) → (include_count, exclude_count) COUNTER
//!
//! # Generation-swap algorithm
//! 1. Read `current_gen` from `segment_list_generations` (0 if no row exists).
//! 2. `new_gen = current_gen + 1`.
//! 3. Write all include/exclude entries under `new_gen`.
//! 4. CAS flip: `UPDATE … SET active_generation = new_gen … IF active_generation = current_gen`
//!    (or `INSERT … IF NOT EXISTS` when `current_gen == 0`).
//! 5. If the CAS loses, another writer won — we return Ok(()); the stale `new_gen` partition
//!    will eventually be GC'd by the sweeper.

use std::collections::HashMap;

use stitchd_core::id::SegmentId;

use crate::{
    repository::{ListContextCounts, ListSegmentSummary},
    scylla::{ScyllaClient, ScyllaError},
};

/// ScyllaDB-backed store for list-segment entries.
///
/// `Clone` is cheap — the inner `ScyllaClient` wraps an `Arc<Session>`.
#[derive(Clone)]
pub struct ScyllaSegmentStore {
    client: ScyllaClient,
}

impl ScyllaSegmentStore {
    /// Create a new store backed by the given client.
    #[must_use]
    pub fn new(client: ScyllaClient) -> Self {
        Self { client }
    }

    // -----------------------------------------------------------------------
    // Private helpers
    // -----------------------------------------------------------------------

    /// Read the current active generation for `(segment_id, context_type)`.
    /// Returns `0` when no pointer row exists yet.
    async fn current_generation(
        &self,
        segment_id: uuid::Uuid,
        context_type: &str,
    ) -> Result<i64, ScyllaError> {
        let ks = self.client.keyspace();
        let cql = format!(
            "SELECT active_generation FROM {ks}.segment_list_generations \
             WHERE segment_id = ? AND context_type = ?"
        );
        let stmt = self.client.prepare(&cql).await?;
        let rows = self
            .client
            .session()
            .execute_unpaged(&stmt, (segment_id, context_type))
            .await
            .map_err(|e| ScyllaError::Query(e.to_string()))?;

        if let Ok(result) = rows.into_rows_result() {
            if let Ok(mut iter) = result.rows::<(i64,)>() {
                if let Some(Ok((active_gen,))) = iter.next() {
                    return Ok(active_gen);
                }
            }
        }
        Ok(0)
    }

    // -----------------------------------------------------------------------
    // Public API — Tasks 3, 5, 7, 9
    // -----------------------------------------------------------------------

    /// Replace all list entries for `(segment_id, context_type)` atomically.
    ///
    /// Algorithm: generation-swap with LWT CAS pointer flip.
    pub async fn set_list_entries(
        &self,
        id: SegmentId,
        context_type: &str,
        include: &[String],
        exclude: &[String],
    ) -> Result<(), ScyllaError> {
        let ks = self.client.keyspace();
        let seg_uuid = id.as_uuid();

        // Step 1: read current generation (0 if none).
        let current_gen = self.current_generation(seg_uuid, context_type).await?;
        // Use a random new generation ID so concurrent writers don't pollute each
        // other's partitions. The LWT CAS still ensures only one writer's pointer wins.
        // Taking the absolute value ensures the i64 is positive.
        let new_gen = (i64::from(uuid::Uuid::new_v4().as_fields().0) << 32
            | i64::from(uuid::Uuid::new_v4().as_fields().0))
            .abs();
        // Fallback: if somehow new_gen == 0, use a simple increment.
        let new_gen = if new_gen == 0 { current_gen + 1 } else { new_gen };

        // Step 2: write all include entries under new_gen.
        let insert_cql = format!(
            "INSERT INTO {ks}.segment_list_entries \
             (segment_id, context_type, generation, list_type, entry_key) \
             VALUES (?, ?, ?, ?, ?)"
        );
        let insert_stmt = self.client.prepare(&insert_cql).await?;

        for key in include {
            self.client
                .session()
                .execute_unpaged(&insert_stmt, (seg_uuid, context_type, new_gen, "include", key.as_str()))
                .await
                .map_err(|e| ScyllaError::Query(format!("insert include entry: {e}")))?;
        }

        // Step 3: write all exclude entries under new_gen.
        for key in exclude {
            self.client
                .session()
                .execute_unpaged(&insert_stmt, (seg_uuid, context_type, new_gen, "exclude", key.as_str()))
                .await
                .map_err(|e| ScyllaError::Query(format!("insert exclude entry: {e}")))?;
        }

        // Step 4: CAS flip the pointer.
        // Note: Scylla TIMESTAMP columns require CqlTimestamp (millis since epoch).
        let now = scylla::value::CqlTimestamp(chrono::Utc::now().timestamp_millis());
        if current_gen == 0 {
            // No existing row — use INSERT IF NOT EXISTS.
            let cas_cql = format!(
                "INSERT INTO {ks}.segment_list_generations \
                 (segment_id, context_type, active_generation, updated_at) \
                 VALUES (?, ?, ?, ?) IF NOT EXISTS"
            );
            let cas_stmt = self.client.prepare(&cas_cql).await?;
            self.client
                .session()
                .execute_unpaged(&cas_stmt, (seg_uuid, context_type, new_gen, now))
                .await
                .map_err(|e| ScyllaError::Query(format!("CAS insert generation: {e}")))?;
            // If another writer won the INSERT IF NOT EXISTS race, their generation is active.
            // Our new_gen partition will be GC'd by the sweeper. Return Ok.
        } else {
            // Existing row — use UPDATE … IF active_generation = current_gen.
            let cas_cql = format!(
                "UPDATE {ks}.segment_list_generations \
                 SET active_generation = ?, updated_at = ? \
                 WHERE segment_id = ? AND context_type = ? \
                 IF active_generation = ?"
            );
            let cas_stmt = self.client.prepare(&cas_cql).await?;
            self.client
                .session()
                .execute_unpaged(
                    &cas_stmt,
                    (new_gen, now, seg_uuid, context_type, current_gen),
                )
                .await
                .map_err(|e| ScyllaError::Query(format!("CAS update generation: {e}")))?;
            // If the CAS lost (applied = false), another writer won.
            // Our new_gen partition will be GC'd by the sweeper. Return Ok.
        }

        Ok(())
    }

    /// Add individual keys to the current generation of `(segment_id, context_type)`.
    ///
    /// `list_type` must be `"include"` or `"exclude"`. Duplicate adds are idempotent
    /// (Scylla INSERT with same PK is a no-op for non-counter tables).
    pub async fn add_entries(
        &self,
        id: SegmentId,
        context_type: &str,
        list_type: &str,
        keys: &[String],
    ) -> Result<(), ScyllaError> {
        if keys.is_empty() {
            return Ok(());
        }

        let ks = self.client.keyspace();
        let seg_uuid = id.as_uuid();

        let active_gen = self.current_generation(seg_uuid, context_type).await?;
        if active_gen == 0 {
            // No generation yet — nothing to add into.
            return Ok(());
        }

        let cql = format!(
            "INSERT INTO {ks}.segment_list_entries \
             (segment_id, context_type, generation, list_type, entry_key) \
             VALUES (?, ?, ?, ?, ?)"
        );
        let stmt = self.client.prepare(&cql).await?;

        for key in keys {
            self.client
                .session()
                .execute_unpaged(&stmt, (seg_uuid, context_type, active_gen, list_type, key.as_str()))
                .await
                .map_err(|e| ScyllaError::Query(format!("add_entries insert: {e}")))?;
        }

        Ok(())
    }

    /// Remove individual keys from the current generation of `(segment_id, context_type)`.
    ///
    /// `list_type` must be `"include"` or `"exclude"`. Removing a missing key is a no-op.
    pub async fn remove_entries(
        &self,
        id: SegmentId,
        context_type: &str,
        list_type: &str,
        keys: &[String],
    ) -> Result<(), ScyllaError> {
        if keys.is_empty() {
            return Ok(());
        }

        let ks = self.client.keyspace();
        let seg_uuid = id.as_uuid();

        let active_gen = self.current_generation(seg_uuid, context_type).await?;
        if active_gen == 0 {
            // No generation yet — nothing to remove.
            return Ok(());
        }

        let cql = format!(
            "DELETE FROM {ks}.segment_list_entries \
             WHERE segment_id = ? AND context_type = ? AND generation = ? \
             AND list_type = ? AND entry_key = ?"
        );
        let stmt = self.client.prepare(&cql).await?;

        for key in keys {
            self.client
                .session()
                .execute_unpaged(&stmt, (seg_uuid, context_type, active_gen, list_type, key.as_str()))
                .await
                .map_err(|e| ScyllaError::Query(format!("remove_entries delete: {e}")))?;
        }

        Ok(())
    }

    /// Check whether a single `key` is a member of `(segment_id, context_type)`.
    ///
    /// Member iff: in the include list AND NOT in the exclude list.
    /// Returns `false` if there is no active generation (segment not populated).
    pub async fn check_membership(
        &self,
        id: SegmentId,
        context_type: &str,
        key: &str,
    ) -> Result<bool, ScyllaError> {
        let ks = self.client.keyspace();
        let seg_uuid = id.as_uuid();

        let active_gen = self.current_generation(seg_uuid, context_type).await?;
        if active_gen == 0 {
            return Ok(false);
        }

        // Check include.
        let in_include = self
            .entry_exists(ks, seg_uuid, context_type, active_gen, "include", key)
            .await?;

        if !in_include {
            return Ok(false);
        }

        // Check exclude — if present, exclude wins.
        let in_exclude = self
            .entry_exists(ks, seg_uuid, context_type, active_gen, "exclude", key)
            .await?;

        Ok(!in_exclude)
    }

    /// Batch-check membership for multiple `keys` within `(segment_id, context_type)`.
    ///
    /// Returns a map from `key → is_member`. Keys absent from the active generation
    /// map to `false`.
    pub async fn batch_check_membership(
        &self,
        id: SegmentId,
        context_type: &str,
        keys: &[String],
    ) -> Result<HashMap<String, bool>, ScyllaError> {
        let mut result = HashMap::with_capacity(keys.len());

        for key in keys {
            let is_member = self.check_membership(id, context_type, key).await?;
            result.insert(key.clone(), is_member);
        }

        Ok(result)
    }

    /// Return include/exclude counts for all context types within `segment_id`.
    ///
    /// Uses `COUNT(*)` over the active generation partition for correctness.
    /// Returns an empty `counts` map if the segment has no active generation.
    pub async fn get_list_segment_summary(
        &self,
        id: SegmentId,
    ) -> Result<ListSegmentSummary, ScyllaError> {
        let ks = self.client.keyspace();
        let seg_uuid = id.as_uuid();

        // Find all context_types for this segment by querying the generations table.
        let ctx_cql = format!(
            "SELECT context_type, active_generation FROM {ks}.segment_list_generations \
             WHERE segment_id = ?"
        );
        let ctx_stmt = self.client.prepare(&ctx_cql).await?;
        let ctx_rows = self
            .client
            .session()
            .execute_unpaged(&ctx_stmt, (seg_uuid,))
            .await
            .map_err(|e| ScyllaError::Query(format!("list context_types: {e}")))?;

        let mut summary = ListSegmentSummary::default();

        if let Ok(result) = ctx_rows.into_rows_result() {
            if let Ok(iter) = result.rows::<(String, i64)>() {
                for row in iter {
                    let (context_type, active_gen) =
                        row.map_err(|e| ScyllaError::Query(format!("row error: {e}")))?;

                    if active_gen == 0 {
                        continue;
                    }

                    let inc = self
                        .count_entries_for_type(ks, seg_uuid, &context_type, active_gen, "include")
                        .await?;
                    let exc = self
                        .count_entries_for_type(ks, seg_uuid, &context_type, active_gen, "exclude")
                        .await?;

                    summary.counts.insert(
                        context_type,
                        ListContextCounts {
                            include_count: inc,
                            exclude_count: exc,
                        },
                    );
                }
            }
        }

        Ok(summary)
    }

    // -----------------------------------------------------------------------
    // Private helpers
    // -----------------------------------------------------------------------

    async fn entry_exists(
        &self,
        ks: &str,
        segment_id: uuid::Uuid,
        context_type: &str,
        generation: i64,
        list_type: &str,
        key: &str,
    ) -> Result<bool, ScyllaError> {
        let cql = format!(
            "SELECT entry_key FROM {ks}.segment_list_entries \
             WHERE segment_id = ? AND context_type = ? AND generation = ? \
             AND list_type = ? AND entry_key = ?"
        );
        let stmt = self.client.prepare(&cql).await?;
        let rows = self
            .client
            .session()
            .execute_unpaged(&stmt, (segment_id, context_type, generation, list_type, key))
            .await
            .map_err(|e| ScyllaError::Query(format!("entry_exists query: {e}")))?;

        if let Ok(result) = rows.into_rows_result() {
            return Ok(result.rows_num() > 0);
        }
        Ok(false)
    }

    async fn count_entries_for_type(
        &self,
        ks: &str,
        segment_id: uuid::Uuid,
        context_type: &str,
        generation: i64,
        list_type: &str,
    ) -> Result<i64, ScyllaError> {
        let cql = format!(
            "SELECT COUNT(*) FROM {ks}.segment_list_entries \
             WHERE segment_id = ? AND context_type = ? AND generation = ? AND list_type = ?"
        );
        let stmt = self.client.prepare(&cql).await?;
        let rows = self
            .client
            .session()
            .execute_unpaged(&stmt, (segment_id, context_type, generation, list_type))
            .await
            .map_err(|e| ScyllaError::Query(format!("count_entries: {e}")))?;

        if let Ok(result) = rows.into_rows_result() {
            if let Ok(mut iter) = result.rows::<(i64,)>() {
                if let Some(Ok((count,))) = iter.next() {
                    return Ok(count);
                }
            }
        }
        Ok(0)
    }
}
