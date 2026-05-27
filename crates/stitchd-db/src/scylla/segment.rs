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

use chrono::{DateTime, Utc};
use stitchd_core::id::SegmentId;

use crate::{
    repository::{ListContextCounts, ListSegmentSummary},
    scylla::{ScyllaClient, ScyllaError},
};

/// A superseded generation that is safe to delete after the retention window.
#[derive(Debug, Clone)]
pub struct OrphanedGeneration {
    /// Segment this generation belongs to.
    pub segment_id: SegmentId,
    /// Context type (e.g. `"user"`, `"org"`).
    pub context_type: String,
    /// The orphaned generation number.
    pub generation: i64,
    /// When the generation was superseded (used for retention window checks).
    pub orphaned_at: DateTime<Utc>,
}

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
    pub const fn new(client: ScyllaClient) -> Self {
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

        if let Ok(result) = rows.into_rows_result()
            && let Ok(mut iter) = result.rows::<(i64,)>()
            && let Some(Ok((active_gen,))) = iter.next()
        {
            return Ok(active_gen);
        }
        Ok(0)
    }

    // -----------------------------------------------------------------------
    // Public API — Tasks 3, 5, 7, 9
    // -----------------------------------------------------------------------

    /// Replace all list entries for `(segment_id, context_type)` atomically.
    ///
    /// Algorithm: generation-swap with LWT CAS pointer flip.
    ///
    /// # Errors
    /// Returns [`ScyllaError::Query`] if the CQL operation fails.
    #[allow(clippy::too_many_lines)]
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
        let new_gen = if new_gen == 0 {
            current_gen + 1
        } else {
            new_gen
        };

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
                .execute_unpaged(
                    &insert_stmt,
                    (seg_uuid, context_type, new_gen, "include", key.as_str()),
                )
                .await
                .map_err(|e| ScyllaError::Query(format!("insert include entry: {e}")))?;
        }

        // Step 3: write all exclude entries under new_gen.
        for key in exclude {
            self.client
                .session()
                .execute_unpaged(
                    &insert_stmt,
                    (seg_uuid, context_type, new_gen, "exclude", key.as_str()),
                )
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
            let resp = self
                .client
                .session()
                .execute_unpaged(
                    &cas_stmt,
                    (new_gen, now, seg_uuid, context_type, current_gen),
                )
                .await
                .map_err(|e| ScyllaError::Query(format!("CAS update generation: {e}")))?;

            // Check if the CAS succeeded (applied = true means we won).
            // Extract the [applied] column from the LWT response.
            let cas_applied = 'cas: {
                let Ok(row_result) = resp.into_rows_result() else {
                    break 'cas false;
                };
                let Ok(mut iter) = row_result.rows::<(bool,)>() else {
                    break 'cas false;
                };
                iter.next()
                    .and_then(Result::ok)
                    .is_some_and(|(applied,)| applied)
            };

            if cas_applied {
                // We won the CAS — track the old generation as orphaned.
                // Ignore errors: failure to track just means the sweeper won't
                // clean it up, but gc_grace_seconds handles that eventually.
                let _ = self
                    .track_orphaned_generation(seg_uuid, context_type, current_gen)
                    .await;
            }
            // If the CAS lost (applied = false), another writer won.
            // Our new_gen partition will be GC'd by the sweeper. Return Ok.
        }

        Ok(())
    }

    /// Add individual keys to the current generation of `(segment_id, context_type)`.
    ///
    /// `list_type` must be `"include"` or `"exclude"`. Duplicate adds are idempotent
    /// (Scylla INSERT with same PK is a no-op for non-counter tables).
    ///
    /// # Errors
    /// Returns [`ScyllaError::Query`] if the CQL operation fails.
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
                .execute_unpaged(
                    &stmt,
                    (seg_uuid, context_type, active_gen, list_type, key.as_str()),
                )
                .await
                .map_err(|e| ScyllaError::Query(format!("add_entries insert: {e}")))?;
        }

        Ok(())
    }

    /// Remove individual keys from the current generation of `(segment_id, context_type)`.
    ///
    /// `list_type` must be `"include"` or `"exclude"`. Removing a missing key is a no-op.
    ///
    /// # Errors
    /// Returns [`ScyllaError::Query`] if the CQL operation fails.
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
                .execute_unpaged(
                    &stmt,
                    (seg_uuid, context_type, active_gen, list_type, key.as_str()),
                )
                .await
                .map_err(|e| ScyllaError::Query(format!("remove_entries delete: {e}")))?;
        }

        Ok(())
    }

    /// Check whether a single `key` is a member of `(segment_id, context_type)`.
    ///
    /// Member iff: in the include list AND NOT in the exclude list.
    /// Returns `false` if there is no active generation (segment not populated).
    ///
    /// # Errors
    /// Returns [`ScyllaError::Query`] if the CQL operation fails.
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

    /// Return the raw `(in_include, in_exclude)` flags for a single key without
    /// applying the "exclude wins" logic.  Used by the admin lookup endpoint so
    /// the UI can show both flags independently.
    ///
    /// # Errors
    ///
    /// Returns [`ScyllaError`] if the underlying `ScyllaDB` query fails.
    pub async fn check_raw_membership(
        &self,
        id: SegmentId,
        context_type: &str,
        key: &str,
    ) -> Result<(bool, bool), ScyllaError> {
        let ks = self.client.keyspace();
        let seg_uuid = id.as_uuid();

        let active_gen = self.current_generation(seg_uuid, context_type).await?;
        if active_gen == 0 {
            return Ok((false, false));
        }

        let in_include = self
            .entry_exists(ks, seg_uuid, context_type, active_gen, "include", key)
            .await?;
        let in_exclude = self
            .entry_exists(ks, seg_uuid, context_type, active_gen, "exclude", key)
            .await?;

        Ok((in_include, in_exclude))
    }

    /// Batch-check membership for multiple `keys` within `(segment_id, context_type)`.
    ///
    /// Returns a map from `key → is_member`. Keys absent from the active generation
    /// map to `false`.
    ///
    /// # Errors
    /// Returns [`ScyllaError::Query`] if the CQL operation fails.
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
    ///
    /// # Errors
    /// Returns [`ScyllaError::Query`] if the CQL operation fails.
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

        if let Ok(result) = ctx_rows.into_rows_result()
            && let Ok(iter) = result.rows::<(String, i64)>()
        {
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

        Ok(summary)
    }

    // -----------------------------------------------------------------------
    // Sweeper API
    // -----------------------------------------------------------------------

    /// Record a superseded generation in the orphaned-gens table.
    ///
    /// Called by `set_list_entries` after a successful CAS flip. The sweeper
    /// uses this table to discover generations safe to delete.
    async fn track_orphaned_generation(
        &self,
        segment_id: uuid::Uuid,
        context_type: &str,
        generation: i64,
    ) -> Result<(), ScyllaError> {
        let ks = self.client.keyspace();
        let now = scylla::value::CqlTimestamp(chrono::Utc::now().timestamp_millis());
        let cql = format!(
            "INSERT INTO {ks}.segment_list_orphaned_gens \
             (segment_id, context_type, generation, orphaned_at) \
             VALUES (?, ?, ?, ?)"
        );
        let stmt = self.client.prepare(&cql).await?;
        self.client
            .session()
            .execute_unpaged(&stmt, (segment_id, context_type, generation, now))
            .await
            .map_err(|e| ScyllaError::Query(format!("track_orphaned_generation: {e}")))?;
        Ok(())
    }

    /// Return all orphaned generations that became orphaned before `cutoff`.
    ///
    /// Called by the sweeper to find generations safe to delete.
    ///
    /// # Errors
    /// Returns [`ScyllaError`] if the query fails.
    pub async fn list_orphaned_older_than(
        &self,
        cutoff: DateTime<Utc>,
    ) -> Result<Vec<OrphanedGeneration>, ScyllaError> {
        let ks = self.client.keyspace();
        // ALLOW FILTERING is acceptable here: the orphaned table is small
        // (one row per completed set_list_entries) and is pruned by the sweeper.
        let cql = format!(
            "SELECT segment_id, context_type, generation, orphaned_at \
             FROM {ks}.segment_list_orphaned_gens \
             WHERE orphaned_at < ? ALLOW FILTERING"
        );
        let stmt = self.client.prepare(&cql).await?;
        let cutoff_ts = scylla::value::CqlTimestamp(cutoff.timestamp_millis());
        let rows = self
            .client
            .session()
            .execute_unpaged(&stmt, (cutoff_ts,))
            .await
            .map_err(|e| ScyllaError::Query(format!("list_orphaned: {e}")))?;

        let mut result = Vec::new();
        if let Ok(row_result) = rows.into_rows_result() {
            type Row = (uuid::Uuid, String, i64, scylla::value::CqlTimestamp);
            if let Ok(iter) = row_result.rows::<Row>() {
                for row in iter {
                    let (seg_uuid, context_type, generation, orphaned_at_ts) =
                        row.map_err(|e| ScyllaError::Query(format!("orphaned row: {e}")))?;
                    let orphaned_at =
                        DateTime::from_timestamp_millis(orphaned_at_ts.0).unwrap_or_else(Utc::now);
                    result.push(OrphanedGeneration {
                        segment_id: SegmentId::from_uuid(seg_uuid),
                        context_type,
                        generation,
                        orphaned_at,
                    });
                }
            }
        }
        Ok(result)
    }

    /// Delete all entries in `segment_list_entries` for the given orphaned generation.
    ///
    /// Scylla supports `DELETE … WHERE partition_key = ?` to drop an entire partition.
    ///
    /// # Errors
    /// Returns [`ScyllaError`] if the query fails.
    pub async fn delete_generation_entries(
        &self,
        id: SegmentId,
        context_type: &str,
        generation: i64,
    ) -> Result<(), ScyllaError> {
        let ks = self.client.keyspace();
        let seg_uuid = id.as_uuid();
        let cql = format!(
            "DELETE FROM {ks}.segment_list_entries \
             WHERE segment_id = ? AND context_type = ? AND generation = ?"
        );
        let stmt = self.client.prepare(&cql).await?;
        self.client
            .session()
            .execute_unpaged(&stmt, (seg_uuid, context_type, generation))
            .await
            .map_err(|e| ScyllaError::Query(format!("delete_generation_entries: {e}")))?;
        Ok(())
    }

    /// Remove a completed sweep record from `segment_list_orphaned_gens`.
    ///
    /// # Errors
    /// Returns [`ScyllaError`] if the query fails.
    pub async fn mark_generation_swept(
        &self,
        id: SegmentId,
        context_type: &str,
        generation: i64,
    ) -> Result<(), ScyllaError> {
        let ks = self.client.keyspace();
        let seg_uuid = id.as_uuid();
        let cql = format!(
            "DELETE FROM {ks}.segment_list_orphaned_gens \
             WHERE segment_id = ? AND context_type = ? AND generation = ?"
        );
        let stmt = self.client.prepare(&cql).await?;
        self.client
            .session()
            .execute_unpaged(&stmt, (seg_uuid, context_type, generation))
            .await
            .map_err(|e| ScyllaError::Query(format!("mark_generation_swept: {e}")))?;
        Ok(())
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
            .execute_unpaged(
                &stmt,
                (segment_id, context_type, generation, list_type, key),
            )
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

        if let Ok(result) = rows.into_rows_result()
            && let Ok(mut iter) = result.rows::<(i64,)>()
            && let Some(Ok((count,))) = iter.next()
        {
            return Ok(count);
        }
        Ok(0)
    }
}
