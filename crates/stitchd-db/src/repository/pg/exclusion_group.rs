//! Postgres implementation for the `exclusion_group` repository.
//!
//! Manages mutually-exclusive experiment groups: CRUD over `exclusion_groups`
//! plus a basis-point range allocator that carves disjoint `[lo, hi)` windows
//! out of `[0, 10000)` for member experiments (stored on the `experiments`
//! group-bucket columns). The allocator is first-fit over the sorted set of
//! existing member ranges and rejects when no contiguous free window of the
//! requested size exists.
//!
//! All queries use raw `sqlx::query`/`query_as` (not the `query!` macro) to
//! avoid offline-cache coupling, matching the project's repository pattern.

use std::sync::Arc;

use async_trait::async_trait;
use sqlx::{PgPool, Row};

use stitchd_core::{
    evaluation::exclusion::BucketRange,
    experimentation::ExclusionGroup,
    id::{EnvironmentId, ExclusionGroupId, ExperimentId},
};

use crate::{
    RepositoryError,
    repository::AuditLogger,
};

/// Total basis-point space available to a single exclusion group.
const BP_TOTAL: u32 = 10_000;

/// CRUD + range-allocation operations for [`ExclusionGroup`] records.
///
/// Range allocation places member experiments into disjoint `[lo, hi)`
/// basis-point windows within `[0, 10000)`. `allocated_bp`/`free_bp` on the
/// returned [`ExclusionGroup`] are computed live from the member experiments'
/// stored ranges.
#[async_trait]
pub trait ExclusionGroupRepository: Send + Sync {
    /// Fetch a single group by ID (excludes soft-deleted), with computed
    /// `allocated_bp`/`free_bp`.
    async fn find_by_id(&self, id: ExclusionGroupId) -> Result<ExclusionGroup, RepositoryError>;

    /// List all non-deleted groups for an environment, with computed
    /// allocated/free basis points.
    async fn list_by_environment(
        &self,
        env_id: EnvironmentId,
    ) -> Result<Vec<ExclusionGroup>, RepositoryError>;

    /// Create a new group. A random immutable `salt` is generated and stored.
    /// `unit_context_type` is the group's diversion (randomization) unit; all
    /// member experiments must randomize on it. Returns the persisted group
    /// (with `allocated_bp = 0`, `free_bp = 10000`).
    async fn create(
        &self,
        env_id: EnvironmentId,
        name: &str,
        description: Option<&str>,
        unit_context_type: &str,
    ) -> Result<ExclusionGroup, RepositoryError>;

    /// Update a group's `name`/`description` (optimistic locking via `version`).
    /// The `salt` is immutable and never changes.
    async fn update(
        &self,
        id: ExclusionGroupId,
        name: &str,
        description: Option<&str>,
        expected_version: i64,
    ) -> Result<ExclusionGroup, RepositoryError>;

    /// Soft-delete a group.
    async fn soft_delete(&self, id: ExclusionGroupId) -> Result<(), RepositoryError>;

    /// Allocate a disjoint `[lo, hi)` window of `requested_bp` basis points for
    /// `experiment_id` within `group_id`, persisting it on the experiment's
    /// group-bucket columns and setting `exclusion_group_id`.
    ///
    /// Uses first-fit over the existing members' ranges. Returns
    /// [`RepositoryError::InvalidState`] (→ `FAILED_PRECONDITION`) when no
    /// contiguous free window of the requested size exists, or when
    /// `requested_bp` is zero / exceeds the total space.
    async fn allocate_range(
        &self,
        group_id: ExclusionGroupId,
        experiment_id: ExperimentId,
        requested_bp: u32,
    ) -> Result<BucketRange, RepositoryError>;

    /// Release the range held by `experiment_id`, clearing its
    /// `exclusion_group_id`/`group_bucket_lo`/`group_bucket_hi`. Idempotent:
    /// a no-op when the experiment holds no range.
    async fn free_range(&self, experiment_id: ExperimentId) -> Result<(), RepositoryError>;

    /// Compute `(allocated_bp, free_bp)` for a group from its member ranges.
    async fn allocated_free_bp(
        &self,
        group_id: ExclusionGroupId,
    ) -> Result<(u32, u32), RepositoryError>;
}

/// Postgres-backed implementation of [`ExclusionGroupRepository`].
pub struct PgExclusionGroupRepository {
    pool: PgPool,
    audit: Arc<dyn AuditLogger>,
}

impl PgExclusionGroupRepository {
    /// Construct a new repository bound to `pool` and `audit`.
    pub fn new(pool: PgPool, audit: Arc<dyn AuditLogger>) -> Self {
        Self { pool, audit }
    }

    /// Sum of basis points currently allocated to a group's members,
    /// computed within an executor (so it can run inside a transaction).
    async fn sum_allocated_bp<'e, E>(
        executor: E,
        group_id: ExclusionGroupId,
    ) -> Result<u32, RepositoryError>
    where
        E: sqlx::PgExecutor<'e>,
    {
        let sum: Option<i64> = sqlx::query_scalar(
            r"
            SELECT COALESCE(SUM(group_bucket_hi - group_bucket_lo), 0)::bigint
            FROM experiments
            WHERE exclusion_group_id = $1
              AND group_bucket_lo IS NOT NULL
              AND group_bucket_hi IS NOT NULL
              AND deleted_at IS NULL
            ",
        )
        .bind(group_id.as_uuid())
        .fetch_one(executor)
        .await
        .map_err(RepositoryError::Database)?;

        let allocated = sum.unwrap_or(0).clamp(0, i64::from(BP_TOTAL));
        #[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
        Ok(allocated as u32)
    }
}

#[async_trait]
impl ExclusionGroupRepository for PgExclusionGroupRepository {
    async fn find_by_id(&self, id: ExclusionGroupId) -> Result<ExclusionGroup, RepositoryError> {
        let row = sqlx::query(
            r"
            SELECT id, env_id, name, description, salt, unit_context_type, version
            FROM exclusion_groups
            WHERE id = $1 AND deleted_at IS NULL
            ",
        )
        .bind(id.as_uuid())
        .fetch_optional(&self.pool)
        .await
        .map_err(RepositoryError::Database)?;

        let row = row.ok_or_else(|| RepositoryError::NotFound { id: id.to_string() })?;
        let allocated = Self::sum_allocated_bp(&self.pool, id).await?;
        Ok(row_to_group(&row, allocated))
    }

    async fn list_by_environment(
        &self,
        env_id: EnvironmentId,
    ) -> Result<Vec<ExclusionGroup>, RepositoryError> {
        let rows = sqlx::query(
            r"
            SELECT id, env_id, name, description, salt, unit_context_type, version
            FROM exclusion_groups
            WHERE env_id = $1 AND deleted_at IS NULL
            ORDER BY created_at
            ",
        )
        .bind(env_id.as_uuid())
        .fetch_all(&self.pool)
        .await
        .map_err(RepositoryError::Database)?;

        let mut groups = Vec::with_capacity(rows.len());
        for row in &rows {
            let id = ExclusionGroupId::from_uuid(row.get("id"));
            let allocated = Self::sum_allocated_bp(&self.pool, id).await?;
            groups.push(row_to_group(row, allocated));
        }
        Ok(groups)
    }

    async fn create(
        &self,
        env_id: EnvironmentId,
        name: &str,
        description: Option<&str>,
        unit_context_type: &str,
    ) -> Result<ExclusionGroup, RepositoryError> {
        let id = ExclusionGroupId::new();
        // Generate a random, immutable salt server-side. Two fresh UUIDs (hex,
        // dashes stripped) give 64 hex chars of entropy — ample and dependency
        // free (no `rand` crate needed in this repo).
        let salt = generate_salt();

        let row = sqlx::query(
            r"
            INSERT INTO exclusion_groups (id, env_id, name, description, salt, unit_context_type, version)
            VALUES ($1, $2, $3, $4, $5, $6, 1)
            RETURNING id, env_id, name, description, salt, unit_context_type, version
            ",
        )
        .bind(id.as_uuid())
        .bind(env_id.as_uuid())
        .bind(name)
        .bind(description)
        .bind(&salt)
        .bind(unit_context_type)
        .fetch_one(&self.pool)
        .await
        .map_err(map_group_db_err)?;

        self.audit
            .log(
                None,
                "exclusion_group",
                id.as_uuid(),
                "create",
                serde_json::json!({
                    "name": name,
                    "environment_id": env_id.to_string(),
                }),
            )
            .await?;

        // Freshly created: no members, so allocated = 0.
        Ok(row_to_group(&row, 0))
    }

    async fn update(
        &self,
        id: ExclusionGroupId,
        name: &str,
        description: Option<&str>,
        expected_version: i64,
    ) -> Result<ExclusionGroup, RepositoryError> {
        let new_version = expected_version + 1;
        let row = sqlx::query(
            r"
            UPDATE exclusion_groups
            SET name = $1, description = $2, version = $3, updated_at = NOW()
            WHERE id = $4 AND version = $5 AND deleted_at IS NULL
            RETURNING id, env_id, name, description, salt, unit_context_type, version
            ",
        )
        .bind(name)
        .bind(description)
        .bind(new_version)
        .bind(id.as_uuid())
        .bind(expected_version)
        .fetch_optional(&self.pool)
        .await
        .map_err(map_group_db_err)?;

        if let Some(row) = row {
            let allocated = Self::sum_allocated_bp(&self.pool, id).await?;
            self.audit
                .log(
                    None,
                    "exclusion_group",
                    id.as_uuid(),
                    "update",
                    serde_json::json!({ "name": name, "version": new_version }),
                )
                .await?;
            return Ok(row_to_group(&row, allocated));
        }

        // Distinguish NotFound vs VersionConflict.
        let current = sqlx::query_scalar::<_, i32>(
            "SELECT version FROM exclusion_groups WHERE id = $1 AND deleted_at IS NULL",
        )
        .bind(id.as_uuid())
        .fetch_optional(&self.pool)
        .await
        .map_err(RepositoryError::Database)?;

        current.map_or_else(
            || Err(RepositoryError::NotFound { id: id.to_string() }),
            |current_version| {
                Err(RepositoryError::VersionConflict {
                    expected: expected_version,
                    actual: i64::from(current_version),
                })
            },
        )
    }

    async fn soft_delete(&self, id: ExclusionGroupId) -> Result<(), RepositoryError> {
        let result = sqlx::query(
            r"
            UPDATE exclusion_groups
            SET deleted_at = NOW(), updated_at = NOW()
            WHERE id = $1 AND deleted_at IS NULL
            ",
        )
        .bind(id.as_uuid())
        .execute(&self.pool)
        .await
        .map_err(RepositoryError::Database)?;

        if result.rows_affected() == 0 {
            return Err(RepositoryError::NotFound { id: id.to_string() });
        }

        self.audit
            .log(
                None,
                "exclusion_group",
                id.as_uuid(),
                "soft_delete",
                serde_json::json!({}),
            )
            .await?;

        Ok(())
    }

    #[allow(clippy::too_many_lines)]
    async fn allocate_range(
        &self,
        group_id: ExclusionGroupId,
        experiment_id: ExperimentId,
        requested_bp: u32,
    ) -> Result<BucketRange, RepositoryError> {
        if requested_bp == 0 {
            return Err(RepositoryError::InvalidState {
                reason: "requested basis points must be greater than zero".to_string(),
            });
        }
        if requested_bp > BP_TOTAL {
            return Err(RepositoryError::InvalidState {
                reason: format!(
                    "requested {requested_bp} bp exceeds the {BP_TOTAL} bp group capacity"
                ),
            });
        }

        let mut tx = self.pool.begin().await.map_err(RepositoryError::Database)?;

        // The group must exist and be live.
        let group_exists: Option<uuid::Uuid> = sqlx::query_scalar(
            "SELECT id FROM exclusion_groups WHERE id = $1 AND deleted_at IS NULL FOR UPDATE",
        )
        .bind(group_id.as_uuid())
        .fetch_optional(&mut *tx)
        .await
        .map_err(RepositoryError::Database)?;
        if group_exists.is_none() {
            return Err(RepositoryError::NotFound {
                id: group_id.to_string(),
            });
        }

        // Fetch existing member ranges (excluding the experiment being assigned,
        // so re-assignment after free is well-defined), sorted by lo.
        let rows = sqlx::query(
            r"
            SELECT group_bucket_lo, group_bucket_hi
            FROM experiments
            WHERE exclusion_group_id = $1
              AND id <> $2
              AND group_bucket_lo IS NOT NULL
              AND group_bucket_hi IS NOT NULL
              AND deleted_at IS NULL
            ORDER BY group_bucket_lo ASC
            FOR UPDATE
            ",
        )
        .bind(group_id.as_uuid())
        .bind(experiment_id.as_uuid())
        .fetch_all(&mut *tx)
        .await
        .map_err(RepositoryError::Database)?;

        let mut used: Vec<(u32, u32)> = rows
            .iter()
            .map(|r| {
                let lo: i32 = r.get("group_bucket_lo");
                let hi: i32 = r.get("group_bucket_hi");
                #[allow(clippy::cast_sign_loss)]
                (lo.max(0) as u32, hi.max(0) as u32)
            })
            .collect();
        used.sort_unstable();

        let Some((lo, hi)) = first_fit(&used, requested_bp) else {
            return Err(RepositoryError::InvalidState {
                reason: format!(
                    "no contiguous free window of {requested_bp} bp available in group {group_id}"
                ),
            });
        };

        // Persist the allocation on the experiment. The experiment must exist.
        #[allow(clippy::cast_possible_wrap)]
        let result = sqlx::query(
            r"
            UPDATE experiments
            SET exclusion_group_id = $1,
                group_bucket_lo = $2,
                group_bucket_hi = $3,
                updated_at = NOW()
            WHERE id = $4 AND deleted_at IS NULL
            ",
        )
        .bind(group_id.as_uuid())
        .bind(lo as i32)
        .bind(hi as i32)
        .bind(experiment_id.as_uuid())
        .execute(&mut *tx)
        .await
        .map_err(RepositoryError::Database)?;

        if result.rows_affected() == 0 {
            return Err(RepositoryError::NotFound {
                id: experiment_id.to_string(),
            });
        }

        tx.commit().await.map_err(RepositoryError::Database)?;

        self.audit
            .log(
                None,
                "exclusion_group",
                group_id.as_uuid(),
                "allocate_range",
                serde_json::json!({
                    "experiment_id": experiment_id.to_string(),
                    "bucket_lo": lo,
                    "bucket_hi": hi,
                }),
            )
            .await?;

        #[allow(clippy::cast_possible_truncation)]
        Ok(BucketRange {
            lo: lo as u16,
            hi: hi as u16,
        })
    }

    async fn free_range(&self, experiment_id: ExperimentId) -> Result<(), RepositoryError> {
        let group_id: Option<uuid::Uuid> = sqlx::query_scalar(
            "SELECT exclusion_group_id FROM experiments WHERE id = $1 AND deleted_at IS NULL",
        )
        .bind(experiment_id.as_uuid())
        .fetch_optional(&self.pool)
        .await
        .map_err(RepositoryError::Database)?
        .flatten();

        sqlx::query(
            r"
            UPDATE experiments
            SET exclusion_group_id = NULL,
                group_bucket_lo = NULL,
                group_bucket_hi = NULL,
                updated_at = NOW()
            WHERE id = $1 AND deleted_at IS NULL
            ",
        )
        .bind(experiment_id.as_uuid())
        .execute(&self.pool)
        .await
        .map_err(RepositoryError::Database)?;

        if let Some(gid) = group_id {
            self.audit
                .log(
                    None,
                    "exclusion_group",
                    gid,
                    "free_range",
                    serde_json::json!({ "experiment_id": experiment_id.to_string() }),
                )
                .await?;
        }

        Ok(())
    }

    async fn allocated_free_bp(
        &self,
        group_id: ExclusionGroupId,
    ) -> Result<(u32, u32), RepositoryError> {
        let allocated = Self::sum_allocated_bp(&self.pool, group_id).await?;
        Ok((allocated, BP_TOTAL - allocated))
    }
}

// ---------------------------------------------------------------------------
// Free helpers
// ---------------------------------------------------------------------------

/// First-fit allocator: given the sorted, disjoint set of `used` `[lo, hi)`
/// ranges, find the lowest free `[lo, hi)` of exactly `requested` basis points
/// within `[0, BP_TOTAL)`. Returns `None` when no such gap exists.
fn first_fit(used: &[(u32, u32)], requested: u32) -> Option<(u32, u32)> {
    let mut cursor: u32 = 0;
    for &(lo, hi) in used {
        // Gap before this used range.
        if lo >= cursor && lo - cursor >= requested {
            return Some((cursor, cursor + requested));
        }
        cursor = cursor.max(hi);
    }
    // Trailing gap after the last used range.
    if BP_TOTAL >= cursor && BP_TOTAL - cursor >= requested {
        return Some((cursor, cursor + requested));
    }
    None
}

/// Generate a random, immutable salt for a new group from two fresh v4 UUIDs
/// (hex, dashes stripped) — 64 hex chars of entropy, no extra crate needed.
fn generate_salt() -> String {
    let a = uuid::Uuid::new_v4().simple().to_string();
    let b = uuid::Uuid::new_v4().simple().to_string();
    format!("{a}{b}")
}

fn row_to_group(row: &sqlx::postgres::PgRow, allocated_bp: u32) -> ExclusionGroup {
    let id: uuid::Uuid = row.get("id");
    let env_id: uuid::Uuid = row.get("env_id");
    let version: i32 = row.get("version");
    ExclusionGroup {
        id: ExclusionGroupId::from_uuid(id),
        environment_id: EnvironmentId::from_uuid(env_id),
        name: row.get("name"),
        description: row.get("description"),
        salt: row.get("salt"),
        unit_context_type: row.get("unit_context_type"),
        allocated_bp,
        free_bp: BP_TOTAL - allocated_bp.min(BP_TOTAL),
        version: i64::from(version),
    }
}

fn map_group_db_err(e: sqlx::Error) -> RepositoryError {
    if let sqlx::Error::Database(ref dbe) = e {
        let code_cow = dbe.code();
        let code = code_cow.as_deref().unwrap_or("");
        let constraint = dbe.constraint().unwrap_or("unknown").to_string();
        return match code {
            "23505" => RepositoryError::UniqueViolation { field: constraint },
            "23503" => RepositoryError::ForeignKeyViolation { constraint },
            _ => RepositoryError::Database(e),
        };
    }
    RepositoryError::Database(e)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_fit_empty_allocates_from_zero() {
        assert_eq!(first_fit(&[], 2500), Some((0, 2500)));
        assert_eq!(first_fit(&[], BP_TOTAL), Some((0, BP_TOTAL)));
    }

    #[test]
    fn first_fit_after_single_range() {
        let used = [(0u32, 2500u32)];
        assert_eq!(first_fit(&used, 2500), Some((2500, 5000)));
    }

    #[test]
    fn first_fit_fills_internal_gap() {
        // Members at [0,2500) and [5000,7500); a 2500 gap exists at [2500,5000).
        let used = [(0u32, 2500u32), (5000u32, 7500u32)];
        assert_eq!(first_fit(&used, 2500), Some((2500, 5000)));
    }

    #[test]
    fn first_fit_prefers_lowest_gap() {
        // Internal gap [2500,5000) of 2500 and trailing [7500,10000) of 2500.
        let used = [(0u32, 2500u32), (5000u32, 7500u32)];
        assert_eq!(first_fit(&used, 1000), Some((2500, 3500)));
    }

    #[test]
    fn first_fit_rejects_when_full() {
        let used = [(0u32, 10_000u32)];
        assert_eq!(first_fit(&used, 1), None);
    }

    #[test]
    fn first_fit_rejects_when_no_contiguous_window() {
        // 5000 free total but fragmented into two 2500 gaps.
        let used = [(2500u32, 5000u32), (7500u32, 10_000u32)];
        assert_eq!(first_fit(&used, 5000), None);
        // But a 2500 request fits the first gap.
        assert_eq!(first_fit(&used, 2500), Some((0, 2500)));
    }

    #[test]
    fn first_fit_exact_remaining_space() {
        let used = [(0u32, 7500u32)];
        assert_eq!(first_fit(&used, 2500), Some((7500, 10_000)));
        assert_eq!(first_fit(&used, 2501), None);
    }

    #[test]
    fn generate_salt_is_nonempty_and_varies() {
        let a = generate_salt();
        let b = generate_salt();
        assert_eq!(a.len(), 64);
        assert_ne!(a, b, "two salts collided (vanishingly unlikely)");
    }
}
