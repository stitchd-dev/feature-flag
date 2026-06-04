//! Repository for the `flag_prerequisites` + `entity_dependencies` tables
//! (`flag_lifecycle_20260604`, Phase 4 Task 1).
//!
//! A flag prerequisite is a flag→flag gate edge: the dependent flag (`flag_id`)
//! requires the prerequisite flag (`prerequisite_flag_id`) to resolve to
//! `required_variant_id` before it proceeds to its own rules; otherwise the
//! dependent flag returns its configured fallback variant
//! (`feature_flags.fallback_variant_id`).
//!
//! Every prerequisite edge is mirrored as a generic row in
//! `entity_dependencies` (`from_type='flag', from_id=<flag>, to_type='flag',
//! to_id=<prereq_flag>, kind='prerequisite'`) so the cross-entity referential
//! integrity guard can answer "who depends on me?" ([`dependents_of`]) and
//! block deletion/archival of a still-referenced flag. The two tables are kept
//! consistent inside a single transaction by [`replace`].
//!
//! [`replace`]: PrerequisiteRepository::replace
//! [`dependents_of`]: PrerequisiteRepository::dependents_of
//!
//! New tables → runtime `sqlx::query`/`query_as` (no compile-time `query!`
//! macros), matching the rest of `repository/pg/`.

use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

use crate::RepositoryError;

/// Entity type stamped on `entity_dependencies` rows for a flag.
pub const ENTITY_TYPE_FLAG: &str = "flag";
/// Edge kind stamped on `entity_dependencies` rows for a prerequisite gate edge.
pub const DEPENDENCY_KIND_PREREQUISITE: &str = "prerequisite";

// ---------------------------------------------------------------------------
// Row / input types
// ---------------------------------------------------------------------------

/// A single prerequisite-gate edge as persisted in `flag_prerequisites`.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct FlagPrerequisiteRow {
    /// The dependent flag (the one that has the prerequisite).
    pub flag_id: Uuid,
    /// The flag this gate depends on.
    pub prerequisite_flag_id: Uuid,
    /// The variant the prerequisite flag must resolve to for the gate to pass.
    pub required_variant_id: Uuid,
    /// Stable display / evaluation ordering.
    #[sqlx(rename = "order")]
    pub order: i32,
}

/// Input for a single prerequisite edge in [`PrerequisiteRepository::replace`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewFlagPrerequisite {
    /// The flag this gate depends on.
    pub prerequisite_flag_id: Uuid,
    /// The variant the prerequisite flag must resolve to for the gate to pass.
    pub required_variant_id: Uuid,
}

/// A dependent surfaced by [`PrerequisiteRepository::dependents_of`]: an entity
/// that references the queried entity (so the queried entity may not be deleted
/// until the reference is removed).
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct DependentRow {
    /// The dependent entity's type (e.g. `flag`).
    pub from_type: String,
    /// The dependent entity's UUID.
    pub from_id: Uuid,
    /// The edge kind (e.g. `prerequisite`).
    pub kind: String,
}

// ---------------------------------------------------------------------------
// Repository
// ---------------------------------------------------------------------------

/// Postgres-backed repository for flag prerequisites + their mirror edges in
/// `entity_dependencies`.
#[derive(Clone)]
pub struct PrerequisiteRepository {
    pool: PgPool,
}

impl PrerequisiteRepository {
    /// Construct a new repository bound to `pool`.
    #[must_use]
    pub const fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Read the prerequisite edges for a flag, ordered by `order` then by
    /// `prerequisite_flag_id` for determinism.
    ///
    /// # Errors
    /// [`RepositoryError::Database`] on SQL failure.
    pub async fn get(&self, flag_id: Uuid) -> Result<Vec<FlagPrerequisiteRow>, RepositoryError> {
        sqlx::query_as::<_, FlagPrerequisiteRow>(
            r#"
            SELECT flag_id, prerequisite_flag_id, required_variant_id, "order"
            FROM flag_prerequisites
            WHERE flag_id = $1
            ORDER BY "order" ASC, prerequisite_flag_id ASC
            "#,
        )
        .bind(flag_id)
        .fetch_all(&self.pool)
        .await
        .map_err(RepositoryError::Database)
    }

    /// Read the persisted fallback variant for a flag (`NULL` → `None`, meaning
    /// "fall back to the flag's off/disabled variant").
    ///
    /// # Errors
    /// [`RepositoryError::NotFound`] if the flag does not exist; otherwise
    /// [`RepositoryError::Database`].
    pub async fn get_fallback_variant(
        &self,
        flag_id: Uuid,
    ) -> Result<Option<Uuid>, RepositoryError> {
        let row: Option<(Option<Uuid>,)> = sqlx::query_as(
            r"SELECT fallback_variant_id FROM feature_flags WHERE id = $1 AND deleted_at IS NULL",
        )
        .bind(flag_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(RepositoryError::Database)?;
        row.map(|(v,)| v).ok_or_else(|| RepositoryError::NotFound {
            id: flag_id.to_string(),
        })
    }

    /// **Atomically replace** the full prerequisite set for `flag_id` and its
    /// `fallback_variant_id`, keeping the `entity_dependencies` mirror edges in
    /// the same transaction.
    ///
    /// Within one transaction this:
    /// 1. deletes all existing `flag_prerequisites` rows for `flag_id`,
    /// 2. deletes all existing `entity_dependencies` prerequisite edges
    ///    originating from `flag_id`,
    /// 3. inserts the new `flag_prerequisites` rows (preserving caller order via
    ///    the `order` column),
    /// 4. inserts the matching `entity_dependencies` prerequisite edges,
    /// 5. sets `feature_flags.fallback_variant_id` for `flag_id`.
    ///
    /// Passing an empty `prerequisites` slice clears all prerequisites for the
    /// flag (and all its prerequisite edges).
    ///
    /// # Errors
    /// [`RepositoryError::ForeignKeyViolation`] when a referenced flag/variant
    /// does not exist; otherwise [`RepositoryError::Database`].
    pub async fn replace(
        &self,
        flag_id: Uuid,
        prerequisites: &[NewFlagPrerequisite],
        fallback_variant_id: Option<Uuid>,
    ) -> Result<Vec<FlagPrerequisiteRow>, RepositoryError> {
        let mut tx: Transaction<'_, Postgres> =
            self.pool.begin().await.map_err(RepositoryError::Database)?;

        // 1. Clear existing prerequisite rows for this flag.
        sqlx::query("DELETE FROM flag_prerequisites WHERE flag_id = $1")
            .bind(flag_id)
            .execute(&mut *tx)
            .await
            .map_err(map_db_err)?;

        // 2. Clear existing prerequisite mirror edges originating from this flag.
        sqlx::query(
            r"DELETE FROM entity_dependencies
              WHERE from_type = $1 AND from_id = $2 AND kind = $3",
        )
        .bind(ENTITY_TYPE_FLAG)
        .bind(flag_id)
        .bind(DEPENDENCY_KIND_PREREQUISITE)
        .execute(&mut *tx)
        .await
        .map_err(map_db_err)?;

        // 3 + 4. Insert the new rows + mirror edges, preserving order.
        for (idx, prereq) in prerequisites.iter().enumerate() {
            let order = i32::try_from(idx).unwrap_or(i32::MAX);
            sqlx::query(
                r#"
                INSERT INTO flag_prerequisites
                    (flag_id, prerequisite_flag_id, required_variant_id, "order")
                VALUES ($1, $2, $3, $4)
                "#,
            )
            .bind(flag_id)
            .bind(prereq.prerequisite_flag_id)
            .bind(prereq.required_variant_id)
            .bind(order)
            .execute(&mut *tx)
            .await
            .map_err(map_db_err)?;

            sqlx::query(
                r"
                INSERT INTO entity_dependencies (from_type, from_id, to_type, to_id, kind)
                VALUES ($1, $2, $3, $4, $5)
                ON CONFLICT (from_type, from_id, to_type, to_id, kind) DO NOTHING
                ",
            )
            .bind(ENTITY_TYPE_FLAG)
            .bind(flag_id)
            .bind(ENTITY_TYPE_FLAG)
            .bind(prereq.prerequisite_flag_id)
            .bind(DEPENDENCY_KIND_PREREQUISITE)
            .execute(&mut *tx)
            .await
            .map_err(map_db_err)?;
        }

        // 5. Persist the fallback variant.
        sqlx::query("UPDATE feature_flags SET fallback_variant_id = $2 WHERE id = $1")
            .bind(flag_id)
            .bind(fallback_variant_id)
            .execute(&mut *tx)
            .await
            .map_err(map_db_err)?;

        tx.commit().await.map_err(RepositoryError::Database)?;

        self.get(flag_id).await
    }

    /// **Who depends on me?** Return every entity that references
    /// `(entity_type, entity_id)` via an `entity_dependencies` edge — i.e. the
    /// dependents that must be removed before this entity can be deleted or
    /// archived. A non-empty result drives the `409 DEPENDENCY_EXISTS` guard.
    ///
    /// # Errors
    /// [`RepositoryError::Database`] on SQL failure.
    pub async fn dependents_of(
        &self,
        entity_type: &str,
        entity_id: Uuid,
    ) -> Result<Vec<DependentRow>, RepositoryError> {
        sqlx::query_as::<_, DependentRow>(
            r"
            SELECT from_type, from_id, kind
            FROM entity_dependencies
            WHERE to_type = $1 AND to_id = $2
            ORDER BY from_id ASC
            ",
        )
        .bind(entity_type)
        .bind(entity_id)
        .fetch_all(&self.pool)
        .await
        .map_err(RepositoryError::Database)
    }

    /// Return every `flag → prerequisite_flag` edge whose **dependent** flag is
    /// in `flag_ids`. Used to assemble the existing prerequisite graph for
    /// write-time cycle detection (the proposed edge set for the flag being
    /// edited is merged on top by the caller).
    ///
    /// # Errors
    /// [`RepositoryError::Database`] on SQL failure.
    pub async fn edges_for_flags(
        &self,
        flag_ids: &[Uuid],
    ) -> Result<Vec<(Uuid, Uuid)>, RepositoryError> {
        if flag_ids.is_empty() {
            return Ok(Vec::new());
        }
        let rows: Vec<(Uuid, Uuid)> = sqlx::query_as(
            r"
            SELECT flag_id, prerequisite_flag_id
            FROM flag_prerequisites
            WHERE flag_id = ANY($1)
            ",
        )
        .bind(flag_ids)
        .fetch_all(&self.pool)
        .await
        .map_err(RepositoryError::Database)?;
        Ok(rows)
    }
}

/// Map a sqlx error to a structured [`RepositoryError`], translating a
/// foreign-key violation (referenced flag/variant absent) into
/// [`RepositoryError::ForeignKeyViolation`].
fn map_db_err(e: sqlx::Error) -> RepositoryError {
    if let sqlx::Error::Database(ref db) = e
        && db.is_foreign_key_violation()
    {
        return RepositoryError::ForeignKeyViolation {
            constraint: db
                .constraint()
                .unwrap_or("flag_prerequisites_fk")
                .to_string(),
        };
    }
    RepositoryError::Database(e)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Insert a minimal `feature_flags` row directly (the repo never creates
    /// flags itself) so prerequisite FKs resolve. Returns the new flag id.
    async fn insert_flag(pool: &PgPool, key: &str) -> Uuid {
        // feature_flags requires a project; create the minimal org→project
        // chain. Use only the columns present in the baseline schema.
        let org_id: Uuid =
            sqlx::query_scalar("INSERT INTO organisations (name) VALUES ($1) RETURNING id")
                .bind(format!("org-{key}"))
                .fetch_one(pool)
                .await
                .unwrap();
        let project_id: Uuid = sqlx::query_scalar(
            "INSERT INTO projects (organisation_id, name) VALUES ($1, $2) RETURNING id",
        )
        .bind(org_id)
        .bind(format!("proj-{key}"))
        .fetch_one(pool)
        .await
        .unwrap();
        sqlx::query_scalar(
            r"INSERT INTO feature_flags (project_id, key, name, enabled, value_type)
              VALUES ($1, $2, $3, true, 'bool') RETURNING id",
        )
        .bind(project_id)
        .bind(key)
        .bind(key)
        .fetch_one(pool)
        .await
        .unwrap()
    }

    async fn insert_variant(pool: &PgPool, flag_id: Uuid, key: &str) -> Uuid {
        sqlx::query_scalar(
            r"INSERT INTO variants (flag_id, key, value) VALUES ($1, $2, $3) RETURNING id",
        )
        .bind(flag_id)
        .bind(key)
        .bind(serde_json::json!(true))
        .fetch_one(pool)
        .await
        .unwrap()
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn replace_inserts_rows_and_mirror_edges(pool: PgPool) {
        let dependent = insert_flag(&pool, "dependent").await;
        let prereq = insert_flag(&pool, "prereq").await;
        let variant = insert_variant(&pool, prereq, "on").await;
        let repo = PrerequisiteRepository::new(pool);

        let rows = repo
            .replace(
                dependent,
                &[NewFlagPrerequisite {
                    prerequisite_flag_id: prereq,
                    required_variant_id: variant,
                }],
                None,
            )
            .await
            .unwrap();

        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].prerequisite_flag_id, prereq);
        assert_eq!(rows[0].required_variant_id, variant);
        assert_eq!(rows[0].order, 0);

        // get() round-trips.
        let fetched = repo.get(dependent).await.unwrap();
        assert_eq!(fetched, rows);

        // Mirror edge exists: prereq now has a dependent.
        let deps = repo.dependents_of(ENTITY_TYPE_FLAG, prereq).await.unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].from_id, dependent);
        assert_eq!(deps[0].kind, DEPENDENCY_KIND_PREREQUISITE);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn replace_persists_fallback_variant(pool: PgPool) {
        let dependent = insert_flag(&pool, "dependent").await;
        let prereq = insert_flag(&pool, "prereq").await;
        let req_variant = insert_variant(&pool, prereq, "on").await;
        let fallback = insert_variant(&pool, dependent, "off").await;
        let repo = PrerequisiteRepository::new(pool);

        repo.replace(
            dependent,
            &[NewFlagPrerequisite {
                prerequisite_flag_id: prereq,
                required_variant_id: req_variant,
            }],
            Some(fallback),
        )
        .await
        .unwrap();

        assert_eq!(
            repo.get_fallback_variant(dependent).await.unwrap(),
            Some(fallback)
        );
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn replace_is_idempotent_and_clears(pool: PgPool) {
        let dependent = insert_flag(&pool, "dependent").await;
        let prereq = insert_flag(&pool, "prereq").await;
        let variant = insert_variant(&pool, prereq, "on").await;
        let repo = PrerequisiteRepository::new(pool);

        let new = NewFlagPrerequisite {
            prerequisite_flag_id: prereq,
            required_variant_id: variant,
        };
        // Replace twice — must not duplicate rows or edges.
        repo.replace(dependent, std::slice::from_ref(&new), None)
            .await
            .unwrap();
        repo.replace(dependent, std::slice::from_ref(&new), None)
            .await
            .unwrap();
        assert_eq!(repo.get(dependent).await.unwrap().len(), 1);
        assert_eq!(
            repo.dependents_of(ENTITY_TYPE_FLAG, prereq)
                .await
                .unwrap()
                .len(),
            1
        );

        // Empty slice clears everything (rows + edges).
        repo.replace(dependent, &[], None).await.unwrap();
        assert!(repo.get(dependent).await.unwrap().is_empty());
        assert!(
            repo.dependents_of(ENTITY_TYPE_FLAG, prereq)
                .await
                .unwrap()
                .is_empty()
        );
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn replace_preserves_order(pool: PgPool) {
        let dependent = insert_flag(&pool, "dependent").await;
        let p1 = insert_flag(&pool, "p1").await;
        let p2 = insert_flag(&pool, "p2").await;
        let v1 = insert_variant(&pool, p1, "on").await;
        let v2 = insert_variant(&pool, p2, "on").await;
        let repo = PrerequisiteRepository::new(pool);

        repo.replace(
            dependent,
            &[
                NewFlagPrerequisite {
                    prerequisite_flag_id: p2,
                    required_variant_id: v2,
                },
                NewFlagPrerequisite {
                    prerequisite_flag_id: p1,
                    required_variant_id: v1,
                },
            ],
            None,
        )
        .await
        .unwrap();

        let rows = repo.get(dependent).await.unwrap();
        assert_eq!(rows.len(), 2);
        // Ordered by the `order` column: p2 first (order 0), p1 second (order 1).
        assert_eq!(rows[0].prerequisite_flag_id, p2);
        assert_eq!(rows[0].order, 0);
        assert_eq!(rows[1].prerequisite_flag_id, p1);
        assert_eq!(rows[1].order, 1);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn replace_rejects_unknown_prerequisite_flag(pool: PgPool) {
        let dependent = insert_flag(&pool, "dependent").await;
        let repo = PrerequisiteRepository::new(pool);

        let err = repo
            .replace(
                dependent,
                &[NewFlagPrerequisite {
                    prerequisite_flag_id: Uuid::new_v4(),
                    required_variant_id: Uuid::new_v4(),
                }],
                None,
            )
            .await
            .unwrap_err();
        assert!(
            matches!(err, RepositoryError::ForeignKeyViolation { .. }),
            "expected FK violation, got {err:?}"
        );
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn dependents_of_empty_for_unreferenced_entity(pool: PgPool) {
        let repo = PrerequisiteRepository::new(pool);
        let deps = repo
            .dependents_of(ENTITY_TYPE_FLAG, Uuid::new_v4())
            .await
            .unwrap();
        assert!(deps.is_empty());
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn dependents_of_lists_multiple(pool: PgPool) {
        let prereq = insert_flag(&pool, "prereq").await;
        let variant = insert_variant(&pool, prereq, "on").await;
        let d1 = insert_flag(&pool, "d1").await;
        let d2 = insert_flag(&pool, "d2").await;
        let repo = PrerequisiteRepository::new(pool);

        for dep in [d1, d2] {
            repo.replace(
                dep,
                &[NewFlagPrerequisite {
                    prerequisite_flag_id: prereq,
                    required_variant_id: variant,
                }],
                None,
            )
            .await
            .unwrap();
        }

        let deps = repo.dependents_of(ENTITY_TYPE_FLAG, prereq).await.unwrap();
        assert_eq!(deps.len(), 2);
        let ids: std::collections::HashSet<Uuid> = deps.iter().map(|d| d.from_id).collect();
        assert!(ids.contains(&d1));
        assert!(ids.contains(&d2));
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn edges_for_flags_returns_only_requested_dependents(pool: PgPool) {
        let prereq = insert_flag(&pool, "prereq").await;
        let variant = insert_variant(&pool, prereq, "on").await;
        let d1 = insert_flag(&pool, "d1").await;
        let d2 = insert_flag(&pool, "d2").await;
        let repo = PrerequisiteRepository::new(pool);

        for dep in [d1, d2] {
            repo.replace(
                dep,
                &[NewFlagPrerequisite {
                    prerequisite_flag_id: prereq,
                    required_variant_id: variant,
                }],
                None,
            )
            .await
            .unwrap();
        }

        // Only ask for d1's edges.
        let edges = repo.edges_for_flags(&[d1]).await.unwrap();
        assert_eq!(edges, vec![(d1, prereq)]);

        // Both.
        let mut both = repo.edges_for_flags(&[d1, d2]).await.unwrap();
        both.sort();
        let mut expected = vec![(d1, prereq), (d2, prereq)];
        expected.sort();
        assert_eq!(both, expected);

        // Empty input → empty output.
        assert!(repo.edges_for_flags(&[]).await.unwrap().is_empty());
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn get_fallback_variant_missing_flag_is_not_found(pool: PgPool) {
        let repo = PrerequisiteRepository::new(pool);
        let err = repo.get_fallback_variant(Uuid::new_v4()).await.unwrap_err();
        assert!(matches!(err, RepositoryError::NotFound { .. }));
    }
}
