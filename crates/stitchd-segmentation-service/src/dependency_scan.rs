//! Segment referential-integrity scan (`flag_lifecycle_20260604`, Phase 6 Task 1).
//!
//! A segment may not be deleted while it is still referenced by another entity.
//! References are computed **authoritatively from the definition sources** at
//! delete time (no reliance on the `entity_dependencies` edge table, which is
//! only populated for flag→flag prerequisites):
//!
//!   * **flag → segment** — any `feature_flag_rules.rule_def` (a serialized
//!     [`ConditionExpr`]) whose tree contains an `InSegment`/`NotInSegment`
//!     leaf naming the target segment. The blocking dependent is the rule's
//!     owning `flag_id`.
//!   * **segment → segment** — any OTHER `segments.condition_expr` (also a
//!     serialized [`ConditionExpr`]) whose tree nests an `InSegment`/
//!     `NotInSegment` leaf naming the target. (In practice the segment write
//!     path forbids segment-membership ops inside a segment's own condition, so
//!     this set is empty for data written through the service — but we compute
//!     it authoritatively regardless, so a reference inserted by any other path
//!     still blocks the delete.)
//!
//! The candidate rows are first narrowed in SQL by a JSONB-text match on the
//! segment UUID (cheap, index-free but selective), then each candidate's
//! `ConditionExpr` is deserialized and walked via
//! [`ConditionExpr::collect_segment_ids`] to confirm a genuine reference (the
//! text match alone could be fooled by a UUID appearing in some unrelated
//! position).
//!
//! New-table-style access → runtime `sqlx::query` (no compile-time macros).

use std::collections::HashSet;

use sqlx::{PgPool, Row};
use uuid::Uuid;

use stitchd_core::id::SegmentId;
use stitchd_core::rule_engine::types::ConditionExpr;
use stitchd_db::RepositoryError;

/// The blocking dependents of a segment, partitioned by entity kind.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SegmentDependents {
    /// Flags whose rules reference the segment (`flag_id`s).
    pub flag_ids: Vec<Uuid>,
    /// Other segments whose condition expression nests the segment (`segment_id`s).
    pub segment_ids: Vec<Uuid>,
}

impl SegmentDependents {
    /// True when no other entity references the segment.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.flag_ids.is_empty() && self.segment_ids.is_empty()
    }

    /// All dependent ids (flags then segments) as a single list, for the
    /// `dependency_exists:<ids>` sentinel.
    #[must_use]
    pub fn all_ids(&self) -> Vec<Uuid> {
        self.flag_ids
            .iter()
            .chain(self.segment_ids.iter())
            .copied()
            .collect()
    }
}

/// Does this serialized-`ConditionExpr` JSON reference `segment_id` via an
/// `InSegment`/`NotInSegment` leaf anywhere in its tree?
fn expr_references_segment(rule_def: &serde_json::Value, segment_id: SegmentId) -> bool {
    let Ok(expr) = serde_json::from_value::<ConditionExpr>(rule_def.clone()) else {
        return false;
    };
    let mut ids: HashSet<SegmentId> = HashSet::new();
    expr.collect_segment_ids(&mut ids);
    ids.contains(&segment_id)
}

/// Compute the authoritative set of entities that reference `segment_id`.
///
/// # Errors
/// Returns a [`RepositoryError`] on a database failure.
pub async fn dependents_of_segment(
    pool: &PgPool,
    segment_id: SegmentId,
) -> Result<SegmentDependents, RepositoryError> {
    let uuid = segment_id.as_uuid();
    let uuid_pat = format!("%{uuid}%");

    // ── flag → segment ────────────────────────────────────────────────────────
    // Candidate flag rules: rule_def text contains the segment UUID. Confirm in
    // Rust by deserializing + walking the ConditionExpr tree. DISTINCT flag_id
    // (a flag may reference the segment in several rules).
    let flag_rows = sqlx::query(
        r"
        SELECT flag_id, rule_def
        FROM feature_flag_rules
        WHERE rule_def::text LIKE $1
        ",
    )
    .bind(&uuid_pat)
    .fetch_all(pool)
    .await
    .map_err(RepositoryError::Database)?;

    let mut flag_ids: Vec<Uuid> = Vec::new();
    let mut seen_flags: HashSet<Uuid> = HashSet::new();
    for row in flag_rows {
        let rule_def: serde_json::Value = row.get("rule_def");
        if expr_references_segment(&rule_def, segment_id) {
            let flag_id: Uuid = row.get("flag_id");
            if seen_flags.insert(flag_id) {
                flag_ids.push(flag_id);
            }
        }
    }

    // ── segment → segment ─────────────────────────────────────────────────────
    // OTHER live segments whose condition_expr nests this segment. Exclude the
    // target itself and soft-deleted rows.
    let seg_rows = sqlx::query(
        r"
        SELECT id, condition_expr
        FROM segments
        WHERE condition_expr IS NOT NULL
          AND deleted_at IS NULL
          AND id <> $1
          AND condition_expr::text LIKE $2
        ",
    )
    .bind(uuid)
    .bind(&uuid_pat)
    .fetch_all(pool)
    .await
    .map_err(RepositoryError::Database)?;

    let mut segment_ids: Vec<Uuid> = Vec::new();
    for row in seg_rows {
        let cond: serde_json::Value = row.get("condition_expr");
        if expr_references_segment(&cond, segment_id) {
            segment_ids.push(row.get("id"));
        }
    }

    flag_ids.sort_unstable();
    segment_ids.sort_unstable();
    Ok(SegmentDependents {
        flag_ids,
        segment_ids,
    })
}

/// Sentinel prefix encoding the blocking dependents (Phase 6 delete-block).
///
/// Stamped onto a `tonic::Status::failed_precondition` message so the gateway
/// can rebuild the structured `409 DEPENDENCY_EXISTS` body. Mirrors
/// `stitchd_flag_service::prerequisites::DEPENDENCY_EXISTS_STATUS_PREFIX`
/// exactly — there is no shared const crate (same convention as the
/// `flag_locked_by_experiment:<uuid>` sentinel).
///
/// Format: `"dependency_exists:<comma-separated dependent ids>"`.
pub const DEPENDENCY_EXISTS_STATUS_PREFIX: &str = "dependency_exists:";

/// Format the `dependency_exists:` sentinel message from blocking dependent ids.
#[must_use]
pub fn dependency_exists_message(dependents: &[Uuid]) -> String {
    let ids = dependents
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(",");
    format!("{DEPENDENCY_EXISTS_STATUS_PREFIX}{ids}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use stitchd_core::rule_engine::condition::Condition;

    fn rule_def_in_segment(seg: SegmentId) -> serde_json::Value {
        serde_json::to_value(ConditionExpr::Leaf(Condition::InSegment(seg))).unwrap()
    }

    #[test]
    fn expr_references_segment_detects_in_segment_leaf() {
        let seg = SegmentId::new();
        assert!(expr_references_segment(&rule_def_in_segment(seg), seg));
    }

    #[test]
    fn expr_references_segment_detects_nested_not_in_segment() {
        let seg = SegmentId::new();
        let expr = ConditionExpr::And(vec![ConditionExpr::Not(Box::new(ConditionExpr::Leaf(
            Condition::NotInSegment(seg),
        )))]);
        let json = serde_json::to_value(expr).unwrap();
        assert!(expr_references_segment(&json, seg));
    }

    #[test]
    fn expr_references_segment_ignores_other_segment() {
        let seg = SegmentId::new();
        let other = SegmentId::new();
        assert!(!expr_references_segment(&rule_def_in_segment(other), seg));
    }

    #[test]
    fn expr_references_segment_ignores_non_condition_json() {
        let seg = SegmentId::new();
        let junk = serde_json::json!({"not": "a condition expr"});
        assert!(!expr_references_segment(&junk, seg));
    }

    #[test]
    fn dependents_is_empty_and_all_ids() {
        let mut deps = SegmentDependents::default();
        assert!(deps.is_empty());
        let f = Uuid::new_v4();
        let s = Uuid::new_v4();
        deps.flag_ids.push(f);
        deps.segment_ids.push(s);
        assert!(!deps.is_empty());
        assert_eq!(deps.all_ids(), vec![f, s]);
    }

    #[test]
    fn dependency_exists_message_joins_ids() {
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        let msg = dependency_exists_message(&[a, b]);
        assert!(msg.starts_with(DEPENDENCY_EXISTS_STATUS_PREFIX));
        assert!(msg.contains(&a.to_string()));
        assert!(msg.contains(&b.to_string()));
        assert!(msg.contains(','));
    }
}
