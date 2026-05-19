//! Postgres implementation for the `segment` repository.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use sqlx::{PgPool, Row as _, types::Json};
use uuid::Uuid;

use stitchd_core::{
    id::{EnvironmentId, RuleId, SegmentId, VariantId},
    rule_engine::types::{ConditionExpr, Rule, RuleOutput},
    segment::{RuleBasedSegment, Segment, SegmentType},
};

use crate::{
    ContextMembership, RepositoryError,
    repository::{AuditLogger, SegmentRepository},
};

/// Map a raw sqlx `Row` (non-macro query) to a [`Segment`] domain object.
fn row_to_segment(row: &sqlx::postgres::PgRow) -> Result<Segment, RepositoryError> {
    let seg_type_str: String = row.get("segment_type");
    let seg_type = match seg_type_str.as_str() {
        "rule" => SegmentType::Rule,
        "list" => SegmentType::List,
        other => {
            return Err(RepositoryError::Database(sqlx::Error::Decode(
                format!("unknown segment_type: {other}").into(),
            )));
        }
    };
    let tags: Vec<String> = row.get("tags");
    Ok(Segment {
        id: SegmentId::from_uuid(row.get::<Uuid, _>("id")),
        environment_id: EnvironmentId::from_uuid(row.get::<Uuid, _>("environment_id")),
        key: row.get("key"),
        name: row.get("name"),
        description: row.get("description"),
        tags,
        segment_type: seg_type,
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
        deleted_at: row.get("deleted_at"),
        version: row.get("version"),
    })
}

/// Postgres-backed implementation of [`SegmentRepository`].
pub struct PgSegmentRepository {
    pool: PgPool,
    audit: Arc<dyn AuditLogger>,
}

impl PgSegmentRepository {
    /// Construct a new repository bound to `pool` and `audit`.
    pub fn new(pool: PgPool, audit: Arc<dyn AuditLogger>) -> Self {
        Self { pool, audit }
    }
}

#[async_trait]
impl SegmentRepository for PgSegmentRepository {
    async fn find_by_id(&self, id: SegmentId) -> Result<Segment, RepositoryError> {
        sqlx::query_as!(
            Segment,
            r#"
            SELECT
                id             AS "id: SegmentId",
                environment_id AS "environment_id: EnvironmentId",
                key,
                name,
                description,
                tags,
                segment_type   AS "segment_type: SegmentType",
                created_at,
                updated_at,
                deleted_at,
                version
            FROM segments
            WHERE id = $1 AND deleted_at IS NULL
            "#,
            id as SegmentId
        )
        .fetch_one(&self.pool)
        .await
        .map_err(|e| match e {
            sqlx::Error::RowNotFound => RepositoryError::NotFound { id: id.to_string() },
            other => RepositoryError::Database(other),
        })
    }

    async fn find_by_key(
        &self,
        key: &str,
        environment_id: EnvironmentId,
    ) -> Result<Segment, RepositoryError> {
        sqlx::query_as!(
            Segment,
            r#"
            SELECT
                id             AS "id: SegmentId",
                environment_id AS "environment_id: EnvironmentId",
                key,
                name,
                description,
                tags,
                segment_type   AS "segment_type: SegmentType",
                created_at,
                updated_at,
                deleted_at,
                version
            FROM segments
            WHERE key = $1 AND environment_id = $2 AND deleted_at IS NULL
            "#,
            key,
            environment_id as EnvironmentId
        )
        .fetch_one(&self.pool)
        .await
        .map_err(|e| match e {
            sqlx::Error::RowNotFound => RepositoryError::NotFound {
                id: format!("{key}@{environment_id}"),
            },
            other => RepositoryError::Database(other),
        })
    }

    async fn list_by_environment(
        &self,
        environment_id: EnvironmentId,
    ) -> Result<Vec<Segment>, RepositoryError> {
        sqlx::query_as!(
            Segment,
            r#"
            SELECT
                id             AS "id: SegmentId",
                environment_id AS "environment_id: EnvironmentId",
                key,
                name,
                description,
                tags,
                segment_type   AS "segment_type: SegmentType",
                created_at,
                updated_at,
                deleted_at,
                version
            FROM segments
            WHERE environment_id = $1 AND deleted_at IS NULL
            ORDER BY created_at
            "#,
            environment_id as EnvironmentId
        )
        .fetch_all(&self.pool)
        .await
        .map_err(RepositoryError::Database)
    }

    async fn list_by_environment_paginated(
        &self,
        environment_id: EnvironmentId,
        offset: u64,
        limit: u64,
    ) -> Result<(Vec<Segment>, u64), RepositoryError> {
        let rows = sqlx::query(
            r"
            SELECT id, environment_id, key, name, description, tags,
                   segment_type, created_at, updated_at, deleted_at, version,
                   COUNT(*) OVER() AS total_count
            FROM segments
            WHERE environment_id = $1 AND deleted_at IS NULL
            ORDER BY created_at
            LIMIT $2 OFFSET $3
            ",
        )
        .bind(environment_id.as_uuid())
        .bind({
            #[allow(clippy::cast_possible_wrap)]
            let v = limit as i64;
            v
        })
        .bind({
            #[allow(clippy::cast_possible_wrap)]
            let v = offset as i64;
            v
        })
        .fetch_all(&self.pool)
        .await
        .map_err(RepositoryError::Database)?;

        let total = rows.first().map_or(0, |r| {
            let n: i64 = r.get("total_count");
            #[allow(clippy::cast_sign_loss)]
            let result = n.max(0) as u64;
            result
        });

        let segments = rows
            .iter()
            .map(row_to_segment)
            .collect::<Result<Vec<_>, _>>()?;

        Ok((segments, total))
    }

    async fn create(&self, segment: &Segment) -> Result<(), RepositoryError> {
        sqlx::query!(
            r#"
            INSERT INTO segments
                (id, environment_id, key, name, description, tags, segment_type,
                 created_at, updated_at, deleted_at, version)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
            "#,
            segment.id as SegmentId,
            segment.environment_id as EnvironmentId,
            segment.key,
            segment.name,
            segment.description,
            &segment.tags as &[String],
            segment.segment_type as SegmentType,
            segment.created_at,
            segment.updated_at,
            segment.deleted_at,
            segment.version,
        )
        .execute(&self.pool)
        .await
        .map_err(|e| {
            if let sqlx::Error::Database(ref dbe) = e {
                if let Some(constraint) = dbe.constraint() {
                    return RepositoryError::UniqueViolation {
                        field: constraint.to_string(),
                    };
                }
            }
            RepositoryError::Database(e)
        })?;

        self.audit
            .log(
                None,
                "segment",
                segment.id.as_uuid(),
                "create",
                serde_json::json!({
                    "key": segment.key,
                    "environment_id": segment.environment_id.to_string(),
                }),
            )
            .await?;

        Ok(())
    }

    async fn update(&self, segment: &Segment) -> Result<Segment, RepositoryError> {
        let new_version = segment.version + 1;
        let result = sqlx::query_as!(
            Segment,
            r#"
            UPDATE segments
            SET key = $1, name = $2, description = $3, tags = $4,
                segment_type = $5, updated_at = NOW(), version = $6
            WHERE id = $7 AND version = $8 AND deleted_at IS NULL
            RETURNING
                id             AS "id: SegmentId",
                environment_id AS "environment_id: EnvironmentId",
                key,
                name,
                description,
                tags,
                segment_type   AS "segment_type: SegmentType",
                created_at,
                updated_at,
                deleted_at,
                version
            "#,
            segment.key,
            segment.name,
            segment.description,
            &segment.tags as &[String],
            segment.segment_type as SegmentType,
            new_version,
            segment.id as SegmentId,
            segment.version,
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(RepositoryError::Database)?;

        if let Some(updated) = result {
            self.audit
                .log(
                    None,
                    "segment",
                    segment.id.as_uuid(),
                    "update",
                    serde_json::json!({ "key": segment.key }),
                )
                .await?;
            Ok(updated)
        } else {
            let current = sqlx::query!(
                r#"SELECT version, deleted_at FROM segments WHERE id = $1"#,
                segment.id as SegmentId
            )
            .fetch_optional(&self.pool)
            .await
            .map_err(RepositoryError::Database)?;

            match current {
                None => Err(RepositoryError::NotFound {
                    id: segment.id.to_string(),
                }),
                Some(row) if row.deleted_at.is_some() => Err(RepositoryError::NotFound {
                    id: segment.id.to_string(),
                }),
                Some(row) => Err(RepositoryError::VersionConflict {
                    expected: segment.version,
                    actual: row.version,
                }),
            }
        }
    }

    async fn get_condition_expr(
        &self,
        id: SegmentId,
    ) -> Result<Option<serde_json::Value>, RepositoryError> {
        let row = sqlx::query!(
            r#"SELECT condition_expr FROM segments WHERE id = $1 AND deleted_at IS NULL"#,
            id as SegmentId
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(RepositoryError::Database)?;

        match row {
            None => Err(RepositoryError::NotFound { id: id.to_string() }),
            Some(r) => Ok(r.condition_expr),
        }
    }

    async fn set_condition_expr(
        &self,
        id: SegmentId,
        expr: Option<&serde_json::Value>,
    ) -> Result<(), RepositoryError> {
        sqlx::query!(
            r#"UPDATE segments SET condition_expr = $1, updated_at = NOW() WHERE id = $2 AND deleted_at IS NULL"#,
            expr as Option<&serde_json::Value>,
            id as SegmentId
        )
        .execute(&self.pool)
        .await
        .map_err(RepositoryError::Database)?;
        Ok(())
    }

    async fn soft_delete(&self, id: SegmentId) -> Result<(), RepositoryError> {
        let result = sqlx::query!(
            r#"
            UPDATE segments
            SET deleted_at = NOW(), updated_at = NOW(), version = version + 1
            WHERE id = $1 AND deleted_at IS NULL
            "#,
            id as SegmentId
        )
        .execute(&self.pool)
        .await
        .map_err(RepositoryError::Database)?;

        if result.rows_affected() == 0 {
            return Err(RepositoryError::NotFound { id: id.to_string() });
        }

        self.audit
            .log(
                None,
                "segment",
                id.as_uuid(),
                "soft_delete",
                serde_json::json!({}),
            )
            .await?;

        Ok(())
    }

    async fn find_with_rules(&self, id: SegmentId) -> Result<RuleBasedSegment, RepositoryError> {
        let segment = self.find_by_id(id).await?;
        if segment.segment_type != SegmentType::Rule {
            return Err(RepositoryError::NotFound { id: id.to_string() });
        }

        let mut rules: Vec<Rule> = sqlx::query!(
            r#"
            SELECT rule_def as "rule_def: Json<Rule>"
            FROM segment_rules
            WHERE segment_id = $1
            ORDER BY rule_index ASC
            "#,
            id as SegmentId
        )
        .fetch_all(&self.pool)
        .await
        .map_err(RepositoryError::Database)?
        .into_iter()
        .map(|r| r.rule_def.0)
        .collect();

        // If no legacy segment_rules rows exist, fall back to the condition_expr column.
        // The UI condition builder stores a single ConditionExpr there instead of rows.
        if rules.is_empty() {
            if let Ok(Some(expr_json)) = self.get_condition_expr(id).await {
                if let Ok(expr) = serde_json::from_value::<ConditionExpr>(expr_json) {
                    rules.push(Rule {
                        id: RuleId::new(),
                        name: None,
                        condition: expr,
                        output: RuleOutput::Variant(VariantId::new()),
                    });
                }
            }
        }

        Ok(RuleBasedSegment { id, rules })
    }

    async fn upsert_rules(&self, id: SegmentId, rules: &[Rule]) -> Result<(), RepositoryError> {
        let mut tx = self.pool.begin().await.map_err(RepositoryError::Database)?;

        // Delete existing rules for this segment
        sqlx::query!(
            r#"DELETE FROM segment_rules WHERE segment_id = $1"#,
            id as SegmentId
        )
        .execute(&mut *tx)
        .await
        .map_err(RepositoryError::Database)?;

        // Insert new rules
        for (i, rule) in rules.iter().enumerate() {
            sqlx::query!(
                r#"
                INSERT INTO segment_rules (segment_id, rule_index, rule_def)
                VALUES ($1, $2, $3)
                "#,
                id as SegmentId,
                i32::try_from(i).unwrap_or(i32::MAX),
                Json(rule) as _
            )
            .execute(&mut *tx)
            .await
            .map_err(RepositoryError::Database)?;
        }

        // Update segment updated_at and version
        sqlx::query!(
            r#"UPDATE segments SET updated_at = NOW(), version = version + 1 WHERE id = $1"#,
            id as SegmentId
        )
        .execute(&mut *tx)
        .await
        .map_err(RepositoryError::Database)?;

        tx.commit().await.map_err(RepositoryError::Database)?;

        self.audit
            .log(
                None,
                "segment",
                id.as_uuid(),
                "upsert_rules",
                serde_json::json!({ "rule_count": rules.len() }),
            )
            .await?;

        Ok(())
    }

    async fn set_list_entries(
        &self,
        _id: SegmentId,
        _context_type: &str,
        _include: &[String],
        _exclude: &[String],
    ) -> Result<(), RepositoryError> {
        Err(RepositoryError::Unexpected(anyhow::anyhow!(
            "set_list_entries is not supported by the PostgreSQL backend; use the Scylla-backed implementation"
        )))
    }

    // ── List-membership reads — STUBBED on the PG backend ────────────────────
    //
    // Historically these queried the `segment_list_entries` PG table, but that
    // table was dropped in migration `20260516000005_drop_segment_list_entries`
    // (Phase 3: list-entry membership moved to ScyllaDB). If we ran the old
    // SQL now the queries would fail with `relation segment_list_entries does
    // not exist`.
    //
    // In production, `SegmentRepository` is provided by
    // [`crate::repository::composite::CompositeSegmentRepository`], which
    // overrides each of these methods and routes list-membership reads to
    // Scylla. `PgSegmentRepository` is also used directly by some unit tests
    // and by `Composite` itself for metadata operations (`find_by_key`,
    // `find_by_id`, …) — so it must still implement the trait, but the
    // list-read methods are intentionally inert here.
    //
    // The stubs return empty / all-false results rather than `Err` so any
    // accidental caller fails-safe to "no membership" instead of crashing.

    async fn check_list_membership(
        &self,
        _environment_id: EnvironmentId,
        _context_type: &str,
        _context_key: &str,
        _segment_keys: &[String],
    ) -> Result<HashMap<String, bool>, RepositoryError> {
        // Empty map: per trait contract, segment keys missing from the map
        // are treated as "not a member" by callers. Use the Composite
        // implementation to get real Scylla-backed answers.
        Ok(HashMap::new())
    }

    async fn batch_check_list_membership(
        &self,
        _environment_id: EnvironmentId,
        _contexts: &[(String, String)],
        _segment_keys: &[String],
    ) -> Result<Vec<ContextMembership>, RepositoryError> {
        // See module note above — Scylla-backed in production.
        Ok(Vec::new())
    }

    async fn find_memberships_batch(
        &self,
        _environment_id: EnvironmentId,
        contexts: &[(String, String)],
        segment_ids: &[SegmentId],
    ) -> Result<Vec<crate::SegmentIdMembership>, RepositoryError> {
        // SDK-facing path expects a fully-populated map per context — every
        // supplied `segment_id` must appear in the output (defaulting to
        // false). Return that shape with all-false entries so any accidental
        // caller still gets a structurally-valid response.
        let results: Vec<crate::SegmentIdMembership> = contexts
            .iter()
            .map(|(t, k)| crate::SegmentIdMembership {
                context_type: t.clone(),
                context_key: k.clone(),
                memberships: segment_ids.iter().map(|id| (*id, false)).collect(),
            })
            .collect();
        Ok(results)
    }

    async fn find_batch_by_ids(&self, ids: &[SegmentId]) -> Result<Vec<Segment>, RepositoryError> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let uuids: Vec<uuid::Uuid> = ids
            .iter()
            .map(stitchd_core::id::SegmentId::as_uuid)
            .collect();
        let rows = sqlx::query(
            r"
            SELECT id, environment_id, key, name, description, tags,
                   segment_type, created_at, updated_at, deleted_at, version
            FROM segments
            WHERE id = ANY($1) AND deleted_at IS NULL
            ",
        )
        .bind(&uuids)
        .fetch_all(&self.pool)
        .await
        .map_err(RepositoryError::Database)?;

        let mut segments = Vec::with_capacity(rows.len());
        for row in rows {
            segments.push(row_to_segment(&row)?);
        }
        Ok(segments)
    }

    async fn find_rules_batch(
        &self,
        ids: &[SegmentId],
    ) -> Result<HashMap<SegmentId, RuleBasedSegment>, RepositoryError> {
        if ids.is_empty() {
            return Ok(HashMap::new());
        }
        let uuids: Vec<uuid::Uuid> = ids
            .iter()
            .map(stitchd_core::id::SegmentId::as_uuid)
            .collect();

        // Load all segment_rules rows for these IDs in one query.
        let rule_rows = sqlx::query(
            r"
            SELECT segment_id, rule_def
            FROM segment_rules
            WHERE segment_id = ANY($1)
            ORDER BY segment_id, rule_index
            ",
        )
        .bind(&uuids)
        .fetch_all(&self.pool)
        .await
        .map_err(RepositoryError::Database)?;

        let mut rules_by_id: HashMap<SegmentId, Vec<Rule>> = HashMap::new();
        for row in rule_rows {
            let seg_uuid: uuid::Uuid = row.get("segment_id");
            let seg_id = SegmentId::from_uuid(seg_uuid);
            let rule_json: serde_json::Value = row.get("rule_def");
            if let Ok(rule) = serde_json::from_value::<Rule>(rule_json) {
                rules_by_id.entry(seg_id).or_default().push(rule);
            }
        }

        // For any IDs with no segment_rules rows, fall back to condition_expr.
        let ids_needing_fallback: Vec<SegmentId> = ids
            .iter()
            .copied()
            .filter(|id| !rules_by_id.contains_key(id))
            .collect();

        if !ids_needing_fallback.is_empty() {
            let fallback_uuids: Vec<uuid::Uuid> = ids_needing_fallback
                .iter()
                .map(stitchd_core::id::SegmentId::as_uuid)
                .collect();
            let fallback_rows = sqlx::query(
                r"SELECT id, condition_expr FROM segments WHERE id = ANY($1) AND condition_expr IS NOT NULL",
            )
            .bind(&fallback_uuids)
            .fetch_all(&self.pool)
            .await
            .map_err(RepositoryError::Database)?;

            for row in fallback_rows {
                let seg_uuid: uuid::Uuid = row.get("id");
                let seg_id = SegmentId::from_uuid(seg_uuid);
                let expr_json: serde_json::Value = row.get("condition_expr");
                if let Ok(expr) = serde_json::from_value::<ConditionExpr>(expr_json) {
                    rules_by_id.entry(seg_id).or_default().push(Rule {
                        id: RuleId::new(),
                        name: None,
                        condition: expr,
                        output: RuleOutput::Variant(VariantId::new()),
                    });
                }
            }
        }

        Ok(ids
            .iter()
            .map(|&id| {
                (
                    id,
                    RuleBasedSegment {
                        id,
                        rules: rules_by_id.remove(&id).unwrap_or_default(),
                    },
                )
            })
            .collect())
    }

    async fn add_entries(
        &self,
        _id: SegmentId,
        _context_type: &str,
        _list_type: &str,
        _keys: &[String],
    ) -> Result<(), RepositoryError> {
        Err(RepositoryError::Unexpected(anyhow::anyhow!(
            "add_entries is not supported by the PostgreSQL backend; use the Scylla-backed implementation"
        )))
    }

    async fn remove_entries(
        &self,
        _id: SegmentId,
        _context_type: &str,
        _list_type: &str,
        _keys: &[String],
    ) -> Result<(), RepositoryError> {
        Err(RepositoryError::Unexpected(anyhow::anyhow!(
            "remove_entries is not supported by the PostgreSQL backend; use the Scylla-backed implementation"
        )))
    }

    async fn get_list_segment_summary(
        &self,
        _id: SegmentId,
    ) -> Result<crate::repository::ListSegmentSummary, RepositoryError> {
        Err(RepositoryError::Unexpected(anyhow::anyhow!(
            "get_list_segment_summary is not supported by the PostgreSQL backend; use the Scylla-backed implementation"
        )))
    }
}
