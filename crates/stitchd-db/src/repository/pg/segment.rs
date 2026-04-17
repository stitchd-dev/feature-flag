//! Postgres implementation for the `segment` repository.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use sqlx::{PgPool, Row as _, types::Json};

use stitchd_core::{
    id::{EnvironmentId, SegmentId},
    rule_engine::types::Rule,
    segment::{ContextList, ListBasedSegment, RuleBasedSegment, Segment, SegmentType},
};

use crate::{
    ContextMembership, RepositoryError,
    repository::{AuditLogger, SegmentRepository},
};

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

    async fn create(&self, segment: &Segment) -> Result<(), RepositoryError> {
        sqlx::query!(
            r#"
            INSERT INTO segments
                (id, environment_id, key, segment_type,
                 created_at, updated_at, deleted_at, version)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
            "#,
            segment.id as SegmentId,
            segment.environment_id as EnvironmentId,
            segment.key,
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
            SET key = $1, segment_type = $2, updated_at = NOW(), version = $3
            WHERE id = $4 AND version = $5 AND deleted_at IS NULL
            RETURNING
                id             AS "id: SegmentId",
                environment_id AS "environment_id: EnvironmentId",
                key,
                segment_type   AS "segment_type: SegmentType",
                created_at,
                updated_at,
                deleted_at,
                version
            "#,
            segment.key,
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

        let rules = sqlx::query!(
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

        Ok(RuleBasedSegment { id, rules })
    }

    async fn find_with_list(&self, id: SegmentId) -> Result<ListBasedSegment, RepositoryError> {
        let segment = self.find_by_id(id).await?;
        if segment.segment_type != SegmentType::List {
            return Err(RepositoryError::NotFound { id: id.to_string() });
        }

        let entries = sqlx::query!(
            r#"
            SELECT context_type, entry_key, list_type
            FROM segment_list_entries
            WHERE segment_id = $1
            "#,
            id as SegmentId
        )
        .fetch_all(&self.pool)
        .await
        .map_err(RepositoryError::Database)?;

        let mut lists = HashMap::new();
        for row in entries {
            let list = lists
                .entry(row.context_type)
                .or_insert_with(ContextList::default);
            match row.list_type.as_str() {
                "include" => {
                    list.include.insert(row.entry_key);
                }
                "exclude" => {
                    list.exclude.insert(row.entry_key);
                }
                _ => {}
            }
        }

        Ok(ListBasedSegment { id, lists })
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
        id: SegmentId,
        context_type: &str,
        include: &[String],
        exclude: &[String],
    ) -> Result<(), RepositoryError> {
        let mut tx = self.pool.begin().await.map_err(RepositoryError::Database)?;

        // Delete existing for this context_type
        sqlx::query!(
            r#"DELETE FROM segment_list_entries WHERE segment_id = $1 AND context_type = $2"#,
            id as SegmentId,
            context_type
        )
        .execute(&mut *tx)
        .await
        .map_err(RepositoryError::Database)?;

        // Insert include
        for key in include {
            sqlx::query!(
                r#"
                INSERT INTO segment_list_entries (segment_id, context_type, entry_key, list_type)
                VALUES ($1, $2, $3, 'include')
                "#,
                id as SegmentId,
                context_type,
                key
            )
            .execute(&mut *tx)
            .await
            .map_err(RepositoryError::Database)?;
        }

        // Insert exclude
        for key in exclude {
            sqlx::query!(
                r#"
                INSERT INTO segment_list_entries (segment_id, context_type, entry_key, list_type)
                VALUES ($1, $2, $3, 'exclude')
                "#,
                id as SegmentId,
                context_type,
                key
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
                "set_list_entries",
                serde_json::json!({
                    "context_type": context_type,
                    "include_count": include.len(),
                    "exclude_count": exclude.len(),
                }),
            )
            .await?;

        Ok(())
    }

    async fn check_list_membership(
        &self,
        environment_id: EnvironmentId,
        context_type: &str,
        context_key: &str,
        segment_keys: &[String],
    ) -> Result<HashMap<String, bool>, RepositoryError> {
        if segment_keys.is_empty() {
            return Ok(HashMap::new());
        }

        // sqlx::query (non-macro) used here to avoid breaking offline mode.
        // Exclude takes precedence: member iff included AND NOT excluded.
        let rows = sqlx::query(
            r"
            SELECT
                s.key AS segment_key,
                (
                    EXISTS (
                        SELECT 1 FROM segment_list_entries e1
                        WHERE e1.segment_id = s.id
                          AND e1.context_type = $2
                          AND e1.entry_key    = $3
                          AND e1.list_type    = 'include'
                    )
                    AND NOT EXISTS (
                        SELECT 1 FROM segment_list_entries e2
                        WHERE e2.segment_id = s.id
                          AND e2.context_type = $2
                          AND e2.entry_key    = $3
                          AND e2.list_type    = 'exclude'
                    )
                ) AS is_member
            FROM segments s
            WHERE s.environment_id = $1
              AND s.key = ANY($4)
              AND s.deleted_at IS NULL
              AND s.segment_type = 'list'
            ",
        )
        .bind(environment_id.as_uuid())
        .bind(context_type)
        .bind(context_key)
        .bind(segment_keys)
        .fetch_all(&self.pool)
        .await
        .map_err(RepositoryError::Database)?;

        let mut result = HashMap::with_capacity(rows.len());
        for row in rows {
            let key: String = row.get("segment_key");
            let is_member: bool = row.get("is_member");
            result.insert(key, is_member);
        }
        Ok(result)
    }

    async fn batch_check_list_membership(
        &self,
        environment_id: EnvironmentId,
        contexts: &[(String, String)],
        segment_keys: &[String],
    ) -> Result<Vec<ContextMembership>, RepositoryError> {
        if segment_keys.is_empty() || contexts.is_empty() {
            return Ok(Vec::new());
        }

        let mut results = Vec::with_capacity(contexts.len());
        for (context_type, context_key) in contexts {
            let memberships = self
                .check_list_membership(environment_id, context_type, context_key, segment_keys)
                .await?;
            results.push(ContextMembership {
                context_type: context_type.clone(),
                context_key: context_key.clone(),
                memberships,
            });
        }
        Ok(results)
    }
}
