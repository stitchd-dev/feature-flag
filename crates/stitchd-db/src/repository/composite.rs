//! Composite segment repository that delegates metadata operations to PostgreSQL
//! and list-entry operations (storage, membership, summary) to [`ScyllaDB`].
//!
//! This is the production implementation wired into the segmentation service.
//! Unit tests use [`crate::repository::pg::PgSegmentRepository`] (via a mock) directly
//! so they don't require a running [`ScyllaDB`] cluster.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;

use stitchd_core::{
    id::{EnvironmentId, SegmentId},
    segment::{RuleBasedSegment, Segment},
};

use crate::{
    ContextMembership, RepositoryError, SegmentIdMembership, SegmentRepository,
    repository::{ListSegmentSummary, pg::PgSegmentRepository},
    scylla::{ScyllaError, segment::ScyllaSegmentStore},
};

// ---------------------------------------------------------------------------
// Error conversion
// ---------------------------------------------------------------------------

impl From<ScyllaError> for RepositoryError {
    fn from(e: ScyllaError) -> Self {
        Self::Unexpected(anyhow::anyhow!("{e}"))
    }
}

// ---------------------------------------------------------------------------
// CompositeSegmentRepository
// ---------------------------------------------------------------------------

/// Production [`SegmentRepository`] that stores segment metadata in PostgreSQL
/// and list entries in `ScyllaDB`.
///
/// Cloning is cheap — both inner stores wrap an `Arc`.
#[derive(Clone)]
pub struct CompositeSegmentRepository {
    pg: Arc<PgSegmentRepository>,
    scylla: Arc<ScyllaSegmentStore>,
}

impl CompositeSegmentRepository {
    /// Create a composite backed by the given PG and Scylla stores.
    ///
    /// # Arguments
    /// * `pg` — PostgreSQL-backed segment repository (metadata operations)
    /// * `scylla` — ScyllaDB-backed list store (entry operations)
    #[must_use]
    pub const fn new(pg: Arc<PgSegmentRepository>, scylla: Arc<ScyllaSegmentStore>) -> Self {
        Self { pg, scylla }
    }
}

#[async_trait]
impl SegmentRepository for CompositeSegmentRepository {
    // ── Metadata — delegate to PG ─────────────────────────────────────────────

    async fn find_by_id(&self, id: SegmentId) -> Result<Segment, RepositoryError> {
        self.pg.find_by_id(id).await
    }

    async fn find_by_key(&self, key: &str, env: EnvironmentId) -> Result<Segment, RepositoryError> {
        self.pg.find_by_key(key, env).await
    }

    async fn list_by_environment(
        &self,
        env: EnvironmentId,
    ) -> Result<Vec<Segment>, RepositoryError> {
        self.pg.list_by_environment(env).await
    }

    async fn list_by_environment_paginated(
        &self,
        env: EnvironmentId,
        offset: u64,
        limit: u64,
    ) -> Result<(Vec<Segment>, u64), RepositoryError> {
        self.pg
            .list_by_environment_paginated(env, offset, limit)
            .await
    }

    async fn create(&self, segment: &Segment) -> Result<(), RepositoryError> {
        self.pg.create(segment).await
    }

    async fn update(&self, segment: &Segment) -> Result<Segment, RepositoryError> {
        self.pg.update(segment).await
    }

    async fn find_with_rules(&self, id: SegmentId) -> Result<RuleBasedSegment, RepositoryError> {
        self.pg.find_with_rules(id).await
    }

    async fn upsert_rules(
        &self,
        id: SegmentId,
        rules: &[stitchd_core::rule_engine::types::Rule],
    ) -> Result<(), RepositoryError> {
        self.pg.upsert_rules(id, rules).await
    }

    async fn get_condition_expr(
        &self,
        id: SegmentId,
    ) -> Result<Option<serde_json::Value>, RepositoryError> {
        self.pg.get_condition_expr(id).await
    }

    async fn set_condition_expr(
        &self,
        id: SegmentId,
        expr: Option<&serde_json::Value>,
    ) -> Result<(), RepositoryError> {
        self.pg.set_condition_expr(id, expr).await
    }

    async fn soft_delete(&self, id: SegmentId) -> Result<(), RepositoryError> {
        self.pg.soft_delete(id).await
    }

    async fn find_batch_by_ids(&self, ids: &[SegmentId]) -> Result<Vec<Segment>, RepositoryError> {
        self.pg.find_batch_by_ids(ids).await
    }

    async fn find_rules_batch(
        &self,
        ids: &[SegmentId],
    ) -> Result<HashMap<SegmentId, RuleBasedSegment>, RepositoryError> {
        self.pg.find_rules_batch(ids).await
    }

    // ── List operations — delegate to Scylla ─────────────────────────────────

    async fn set_list_entries(
        &self,
        id: SegmentId,
        context_type: &str,
        include: &[String],
        exclude: &[String],
    ) -> Result<(), RepositoryError> {
        self.scylla
            .set_list_entries(id, context_type, include, exclude)
            .await
            .map_err(RepositoryError::from)
    }

    async fn add_entries(
        &self,
        id: SegmentId,
        context_type: &str,
        list_type: &str,
        keys: &[String],
    ) -> Result<(), RepositoryError> {
        self.scylla
            .add_entries(id, context_type, list_type, keys)
            .await
            .map_err(RepositoryError::from)
    }

    async fn remove_entries(
        &self,
        id: SegmentId,
        context_type: &str,
        list_type: &str,
        keys: &[String],
    ) -> Result<(), RepositoryError> {
        self.scylla
            .remove_entries(id, context_type, list_type, keys)
            .await
            .map_err(RepositoryError::from)
    }

    async fn get_list_segment_summary(
        &self,
        id: SegmentId,
    ) -> Result<ListSegmentSummary, RepositoryError> {
        self.scylla
            .get_list_segment_summary(id)
            .await
            .map_err(RepositoryError::from)
    }

    /// Check list-segment membership for a single context key across multiple segment keys.
    ///
    /// Resolves segment keys to IDs via PG, then performs point reads in Scylla.
    async fn check_list_membership(
        &self,
        env_id: EnvironmentId,
        context_type: &str,
        context_key: &str,
        segment_keys: &[String],
    ) -> Result<HashMap<String, bool>, RepositoryError> {
        let mut result = HashMap::with_capacity(segment_keys.len());

        for seg_key in segment_keys {
            // Resolve segment key → ID via PG.
            match self.pg.find_by_key(seg_key, env_id).await {
                Ok(seg) => {
                    let is_member = self
                        .scylla
                        .check_membership(seg.id, context_type, context_key)
                        .await
                        .map_err(RepositoryError::from)?;
                    result.insert(seg_key.clone(), is_member);
                }
                Err(RepositoryError::NotFound { .. }) => {
                    // Segment not found — skip (omit from result as per trait contract).
                }
                Err(e) => return Err(e),
            }
        }

        Ok(result)
    }

    /// Batch check list-segment membership for multiple contexts × multiple segment keys.
    async fn batch_check_list_membership(
        &self,
        env_id: EnvironmentId,
        contexts: &[(String, String)],
        segment_keys: &[String],
    ) -> Result<Vec<ContextMembership>, RepositoryError> {
        // Resolve segment keys → IDs once.
        let mut key_to_id: HashMap<String, SegmentId> = HashMap::new();
        for seg_key in segment_keys {
            if let Ok(seg) = self.pg.find_by_key(seg_key, env_id).await {
                key_to_id.insert(seg_key.clone(), seg.id);
            }
        }

        let mut memberships = Vec::with_capacity(contexts.len());
        for (ctx_type, ctx_key) in contexts {
            let mut ctx_map = HashMap::with_capacity(segment_keys.len());
            for (seg_key, seg_id) in &key_to_id {
                let is_member = self
                    .scylla
                    .check_membership(*seg_id, ctx_type, ctx_key)
                    .await
                    .map_err(RepositoryError::from)?;
                ctx_map.insert(seg_key.clone(), is_member);
            }
            memberships.push(ContextMembership {
                context_type: ctx_type.clone(),
                context_key: ctx_key.clone(),
                memberships: ctx_map,
            });
        }

        Ok(memberships)
    }

    /// SDK-facing batch membership check keyed by segment UUID.
    async fn find_memberships_batch(
        &self,
        _env_id: EnvironmentId,
        contexts: &[(String, String)],
        segment_ids: &[SegmentId],
    ) -> Result<Vec<SegmentIdMembership>, RepositoryError> {
        let mut result = Vec::with_capacity(contexts.len());

        for (ctx_type, ctx_key) in contexts {
            let mut memberships = HashMap::with_capacity(segment_ids.len());
            for &seg_id in segment_ids {
                let is_member = self
                    .scylla
                    .check_membership(seg_id, ctx_type, ctx_key)
                    .await
                    .map_err(RepositoryError::from)?;
                memberships.insert(seg_id, is_member);
            }
            result.push(SegmentIdMembership {
                context_type: ctx_type.clone(),
                context_key: ctx_key.clone(),
                memberships,
            });
        }

        Ok(result)
    }
}
