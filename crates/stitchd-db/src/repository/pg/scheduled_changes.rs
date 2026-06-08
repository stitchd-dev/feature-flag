//! Repository for the `scheduled_changes` + `scheduled_change_runs` tables
//! (`flag_lifecycle_20260604`, Phase 3 Task 1).
//!
//! A scheduled change applies a mutation to a flag, segment, or experiment at a
//! future time — one-shot (a single instant) or recurring (an RFC-5545 RRULE in
//! an IANA timezone). The scheduler interval loop **claims** due rows with a
//! `FOR UPDATE SKIP LOCKED` query so multiple scheduler replicas (or a restart
//! overlapping the previous process) never double-apply a change: each row is
//! row-locked for the duration of the claiming transaction and only released
//! once `next_run_at` / `status` have been advanced.
//!
//! New table → runtime `sqlx::query`/`query_as` (no compile-time `query!`
//! macros), matching the rest of `repository/pg/`.

use chrono::{DateTime, Utc};
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

use crate::RepositoryError;

// ---------------------------------------------------------------------------
// Status / kind / outcome enums (text-backed)
// ---------------------------------------------------------------------------

/// Lifecycle status of a scheduled change.
///
/// One-shot: `Pending → Applied | Failed | Cancelled`.
/// Recurring: `Active ⇄ Paused`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, sqlx::Type)]
#[sqlx(type_name = "text", rename_all = "snake_case")]
pub enum ScheduleStatus {
    /// One-shot, not yet fired.
    Pending,
    /// Recurring, currently firing.
    Active,
    /// Recurring, temporarily suspended.
    Paused,
    /// One-shot, fired successfully.
    Applied,
    /// One-shot, fired but the apply failed terminally.
    Failed,
    /// Cancelled before firing.
    Cancelled,
}

impl ScheduleStatus {
    /// Text form persisted in Postgres (matches the `chk_scheduled_changes_status`
    /// constraint values).
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Active => "active",
            Self::Paused => "paused",
            Self::Applied => "applied",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }
}

/// One-shot vs recurring schedule.
#[derive(Debug, Clone, Copy, PartialEq, Eq, sqlx::Type)]
#[sqlx(type_name = "text", rename_all = "snake_case")]
pub enum ScheduleKind {
    /// Fire exactly once at a specific instant.
    OneShot,
    /// Fire repeatedly per an RRULE.
    Recurring,
}

impl ScheduleKind {
    /// Text form persisted in Postgres.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::OneShot => "one_shot",
            Self::Recurring => "recurring",
        }
    }
}

/// Outcome of a single firing, persisted into `scheduled_change_runs`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, sqlx::Type)]
#[sqlx(type_name = "text", rename_all = "snake_case")]
pub enum RunOutcome {
    /// The mutation was applied.
    Applied,
    /// The firing was skipped (e.g. the target flag is experiment-locked).
    Skipped,
    /// The firing failed.
    Failed,
}

impl RunOutcome {
    /// Text form persisted in Postgres.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Applied => "applied",
            Self::Skipped => "skipped",
            Self::Failed => "failed",
        }
    }
}

// ---------------------------------------------------------------------------
// Row / input types
// ---------------------------------------------------------------------------

/// A fully-hydrated row from `scheduled_changes`.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ScheduledChangeRow {
    /// Primary key.
    pub id: Uuid,
    /// `flag` | `segment` | `experiment`.
    pub entity_type: String,
    /// The targeted entity's UUID.
    pub entity_id: Uuid,
    /// The environment the change applies in.
    pub env_id: Uuid,
    /// JSON-encoded mutation request the owning service's RPC consumes.
    pub mutation_payload: serde_json::Value,
    /// `one_shot` | `recurring`.
    pub schedule_kind: String,
    /// One-shot fire instant (UTC); `None` for recurring.
    pub scheduled_at: Option<DateTime<Utc>>,
    /// RFC-5545 RRULE string (recurring); `None` for one-shot.
    pub rrule: Option<String>,
    /// IANA timezone (recurring); `None` for one-shot.
    pub tz: Option<String>,
    /// Next computed firing instant (UTC).
    pub next_run_at: Option<DateTime<Utc>>,
    /// Last actual firing instant (UTC).
    pub last_run_at: Option<DateTime<Utc>>,
    /// Lifecycle status.
    pub status: ScheduleStatus,
    /// Optimistic-concurrency version.
    pub version: i64,
    /// Row creation instant.
    pub created_at: DateTime<Utc>,
    /// Last-update instant.
    pub updated_at: DateTime<Utc>,
    /// Authoring user, or `None` when system-created.
    pub created_by: Option<Uuid>,
}

/// A row from `scheduled_change_runs`.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ScheduledChangeRunRow {
    /// Primary key.
    pub id: Uuid,
    /// FK → `scheduled_changes.id`.
    pub scheduled_change_id: Uuid,
    /// When the firing happened (UTC).
    pub fired_at: DateTime<Utc>,
    /// `applied` | `skipped` | `failed`.
    pub outcome: RunOutcome,
    /// Free-text reason / error detail.
    pub detail: Option<String>,
}

/// Input for [`ScheduledChangeRepository::create`].
#[derive(Debug, Clone)]
pub struct NewScheduledChange {
    /// `flag` | `segment` | `experiment`.
    pub entity_type: String,
    /// The targeted entity's UUID.
    pub entity_id: Uuid,
    /// The environment the change applies in.
    pub env_id: Uuid,
    /// JSON-encoded mutation request.
    pub mutation_payload: serde_json::Value,
    /// One-shot vs recurring.
    pub schedule_kind: ScheduleKind,
    /// One-shot fire instant (UTC); ignored for recurring.
    pub scheduled_at: Option<DateTime<Utc>>,
    /// RRULE string (recurring); ignored for one-shot.
    pub rrule: Option<String>,
    /// IANA timezone (recurring); ignored for one-shot.
    pub tz: Option<String>,
    /// The initial next-run instant. For one-shot this is `scheduled_at`; for
    /// recurring it is the first computed occurrence.
    pub next_run_at: Option<DateTime<Utc>>,
    /// Authoring user, or `None` when system-created.
    pub created_by: Option<Uuid>,
}

// ---------------------------------------------------------------------------
// Repository
// ---------------------------------------------------------------------------

/// Postgres-backed repository for scheduled changes + their run history.
#[derive(Clone)]
pub struct ScheduledChangeRepository {
    pool: PgPool,
}

const SELECT_COLS: &str = r"
    id, entity_type, entity_id, env_id, mutation_payload, schedule_kind,
    scheduled_at, rrule, tz, next_run_at, last_run_at, status, version,
    created_at, updated_at, created_by
";

impl ScheduledChangeRepository {
    /// Construct a new repository bound to `pool`.
    #[must_use]
    pub const fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Insert a new scheduled change. Status defaults to `pending` for one-shot
    /// and `active` for recurring.
    ///
    /// # Errors
    /// Returns [`RepositoryError::Database`] on any SQL failure.
    pub async fn create(
        &self,
        input: &NewScheduledChange,
    ) -> Result<ScheduledChangeRow, RepositoryError> {
        let status = match input.schedule_kind {
            ScheduleKind::OneShot => ScheduleStatus::Pending,
            ScheduleKind::Recurring => ScheduleStatus::Active,
        };
        let sql = format!(
            r"
            INSERT INTO scheduled_changes
                (entity_type, entity_id, env_id, mutation_payload, schedule_kind,
                 scheduled_at, rrule, tz, next_run_at, status, created_by)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
            RETURNING {SELECT_COLS}
            "
        );
        sqlx::query_as::<_, ScheduledChangeRow>(&sql)
            .bind(&input.entity_type)
            .bind(input.entity_id)
            .bind(input.env_id)
            .bind(&input.mutation_payload)
            .bind(input.schedule_kind.as_str())
            .bind(input.scheduled_at)
            .bind(input.rrule.as_deref())
            .bind(input.tz.as_deref())
            .bind(input.next_run_at)
            .bind(status.as_str())
            .bind(input.created_by)
            .fetch_one(&self.pool)
            .await
            .map_err(RepositoryError::Database)
    }

    /// Fetch a single scheduled change by ID (excludes soft-deleted rows).
    ///
    /// # Errors
    /// [`RepositoryError::NotFound`] if no live row exists; otherwise
    /// [`RepositoryError::Database`].
    pub async fn get(&self, id: Uuid) -> Result<ScheduledChangeRow, RepositoryError> {
        let sql = format!(
            r"SELECT {SELECT_COLS} FROM scheduled_changes WHERE id = $1 AND deleted_at IS NULL"
        );
        sqlx::query_as::<_, ScheduledChangeRow>(&sql)
            .bind(id)
            .fetch_optional(&self.pool)
            .await
            .map_err(RepositoryError::Database)?
            .ok_or_else(|| RepositoryError::NotFound { id: id.to_string() })
    }

    /// List the run history for a scheduled change, most-recent-first.
    ///
    /// # Errors
    /// [`RepositoryError::Database`] on SQL failure.
    pub async fn list_runs(
        &self,
        scheduled_change_id: Uuid,
    ) -> Result<Vec<ScheduledChangeRunRow>, RepositoryError> {
        sqlx::query_as::<_, ScheduledChangeRunRow>(
            r"
            SELECT id, scheduled_change_id, fired_at, outcome, detail
            FROM scheduled_change_runs
            WHERE scheduled_change_id = $1
            ORDER BY fired_at DESC
            ",
        )
        .bind(scheduled_change_id)
        .fetch_all(&self.pool)
        .await
        .map_err(RepositoryError::Database)
    }

    /// List schedules targeting a specific entity (live rows only),
    /// most-recently-created first.
    ///
    /// # Errors
    /// [`RepositoryError::Database`] on SQL failure.
    pub async fn list_by_entity(
        &self,
        entity_type: &str,
        entity_id: Uuid,
    ) -> Result<Vec<ScheduledChangeRow>, RepositoryError> {
        let sql = format!(
            r"
            SELECT {SELECT_COLS} FROM scheduled_changes
            WHERE entity_type = $1 AND entity_id = $2 AND deleted_at IS NULL
            ORDER BY created_at DESC
            "
        );
        sqlx::query_as::<_, ScheduledChangeRow>(&sql)
            .bind(entity_type)
            .bind(entity_id)
            .fetch_all(&self.pool)
            .await
            .map_err(RepositoryError::Database)
    }

    /// List schedules in an environment (live rows only), optionally filtered by
    /// status, most-recently-created first.
    ///
    /// # Errors
    /// [`RepositoryError::Database`] on SQL failure.
    pub async fn list_by_env(
        &self,
        env_id: Uuid,
        status: Option<ScheduleStatus>,
    ) -> Result<Vec<ScheduledChangeRow>, RepositoryError> {
        let sql = format!(
            r"
            SELECT {SELECT_COLS} FROM scheduled_changes
            WHERE env_id = $1 AND deleted_at IS NULL
              AND ($2::text IS NULL OR status = $2)
            ORDER BY created_at DESC
            "
        );
        sqlx::query_as::<_, ScheduledChangeRow>(&sql)
            .bind(env_id)
            .bind(status.map(ScheduleStatus::as_str))
            .fetch_all(&self.pool)
            .await
            .map_err(RepositoryError::Database)
    }

    /// **Restart-safe due-claim.** Within a caller-supplied transaction, select
    /// every change whose `next_run_at <= as_of` and whose status is firable
    /// (`pending` or `active`), locking the rows with `FOR UPDATE SKIP LOCKED`
    /// so a concurrent scheduler replica (or an overlapping restart) never
    /// claims the same row. The caller must advance `status`/`next_run_at` and
    /// commit to release the locks; if the process dies mid-transaction the
    /// locks are released by Postgres on connection drop and the next tick
    /// re-claims — a missed tick catches up.
    ///
    /// # Errors
    /// [`RepositoryError::Database`] on SQL failure.
    pub async fn claim_due(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        as_of: DateTime<Utc>,
        limit: i64,
    ) -> Result<Vec<ScheduledChangeRow>, RepositoryError> {
        let sql = format!(
            r"
            SELECT {SELECT_COLS} FROM scheduled_changes
            WHERE status IN ('pending', 'active')
              AND deleted_at IS NULL
              AND next_run_at IS NOT NULL
              AND next_run_at <= $1
            ORDER BY next_run_at ASC
            LIMIT $2
            FOR UPDATE SKIP LOCKED
            "
        );
        sqlx::query_as::<_, ScheduledChangeRow>(&sql)
            .bind(as_of)
            .bind(limit)
            .fetch_all(&mut **tx)
            .await
            .map_err(RepositoryError::Database)
    }

    /// Begin a transaction (for a `claim_due` → advance → commit cycle).
    ///
    /// # Errors
    /// [`RepositoryError::Database`] if the pool cannot start a transaction.
    pub async fn begin(&self) -> Result<Transaction<'static, Postgres>, RepositoryError> {
        self.pool.begin().await.map_err(RepositoryError::Database)
    }

    /// Advance a recurring change to its next window inside the claiming
    /// transaction: set `last_run_at`, bump `next_run_at`, bump `version`.
    /// When `next_run_at` is `None` the recurrence is exhausted and the row is
    /// marked `applied` (no further fires).
    ///
    /// # Errors
    /// [`RepositoryError::Database`] on SQL failure.
    pub async fn advance_recurring(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        id: Uuid,
        last_run_at: DateTime<Utc>,
        next_run_at: Option<DateTime<Utc>>,
    ) -> Result<(), RepositoryError> {
        sqlx::query(
            r"
            UPDATE scheduled_changes
            SET last_run_at = $2,
                next_run_at = $3,
                status      = CASE WHEN $3::timestamptz IS NULL THEN 'applied' ELSE status END,
                version     = version + 1,
                updated_at  = NOW()
            WHERE id = $1
            ",
        )
        .bind(id)
        .bind(last_run_at)
        .bind(next_run_at)
        .execute(&mut **tx)
        .await
        .map_err(RepositoryError::Database)?;
        Ok(())
    }

    /// Mark a one-shot change as fired/terminal inside the claiming
    /// transaction: set `status` + `last_run_at`, clear `next_run_at`, bump
    /// `version`.
    ///
    /// # Errors
    /// [`RepositoryError::Database`] on SQL failure.
    pub async fn finalize_one_shot(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        id: Uuid,
        status: ScheduleStatus,
        last_run_at: DateTime<Utc>,
    ) -> Result<(), RepositoryError> {
        sqlx::query(
            r"
            UPDATE scheduled_changes
            SET status      = $2,
                last_run_at = $3,
                next_run_at = NULL,
                version     = version + 1,
                updated_at  = NOW()
            WHERE id = $1
            ",
        )
        .bind(id)
        .bind(status.as_str())
        .bind(last_run_at)
        .execute(&mut **tx)
        .await
        .map_err(RepositoryError::Database)?;
        Ok(())
    }

    /// Append a run-history row inside the claiming transaction.
    ///
    /// # Errors
    /// [`RepositoryError::Database`] on SQL failure.
    pub async fn append_run_tx(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        scheduled_change_id: Uuid,
        outcome: RunOutcome,
        detail: Option<&str>,
    ) -> Result<(), RepositoryError> {
        sqlx::query(
            r"
            INSERT INTO scheduled_change_runs (scheduled_change_id, outcome, detail)
            VALUES ($1, $2, $3)
            ",
        )
        .bind(scheduled_change_id)
        .bind(outcome.as_str())
        .bind(detail)
        .execute(&mut **tx)
        .await
        .map_err(RepositoryError::Database)?;
        Ok(())
    }

    /// Append a run-history row outside any transaction (pool-level).
    ///
    /// # Errors
    /// [`RepositoryError::Database`] on SQL failure.
    pub async fn append_run(
        &self,
        scheduled_change_id: Uuid,
        outcome: RunOutcome,
        detail: Option<&str>,
    ) -> Result<(), RepositoryError> {
        sqlx::query(
            r"
            INSERT INTO scheduled_change_runs (scheduled_change_id, outcome, detail)
            VALUES ($1, $2, $3)
            ",
        )
        .bind(scheduled_change_id)
        .bind(outcome.as_str())
        .bind(detail)
        .execute(&self.pool)
        .await
        .map_err(RepositoryError::Database)?;
        Ok(())
    }

    /// Cancel a pending one-shot change (version-checked, optimistic). Only a
    /// `pending` change can be cancelled.
    ///
    /// # Errors
    /// [`RepositoryError::NotFound`] if no matching live row;
    /// [`RepositoryError::VersionConflict`] on a stale `expected_version`;
    /// [`RepositoryError::InvalidState`] if the change is not `pending`.
    pub async fn cancel(
        &self,
        id: Uuid,
        expected_version: i64,
    ) -> Result<ScheduledChangeRow, RepositoryError> {
        self.transition(id, expected_version, "pending", ScheduleStatus::Cancelled)
            .await
    }

    /// Pause an active recurring change (version-checked). Only an `active`
    /// change can be paused.
    ///
    /// # Errors
    /// See [`Self::cancel`].
    pub async fn pause(
        &self,
        id: Uuid,
        expected_version: i64,
    ) -> Result<ScheduledChangeRow, RepositoryError> {
        self.transition(id, expected_version, "active", ScheduleStatus::Paused)
            .await
    }

    /// Resume a paused recurring change (version-checked). Only a `paused`
    /// change can be resumed. `next_run_at` is left unchanged here; the
    /// scheduler recomputes the next window on the following tick.
    ///
    /// # Errors
    /// See [`Self::cancel`].
    pub async fn resume(
        &self,
        id: Uuid,
        expected_version: i64,
    ) -> Result<ScheduledChangeRow, RepositoryError> {
        self.transition(id, expected_version, "paused", ScheduleStatus::Active)
            .await
    }

    /// Soft-delete a scheduled change (version-checked). Sets `deleted_at`;
    /// the row stops appearing in listings and due-claims.
    ///
    /// # Errors
    /// [`RepositoryError::NotFound`] if no matching live row;
    /// [`RepositoryError::VersionConflict`] on a stale `expected_version`.
    pub async fn soft_delete(
        &self,
        id: Uuid,
        expected_version: i64,
    ) -> Result<(), RepositoryError> {
        // First check the row exists + version matches (distinguish NotFound
        // from VersionConflict like the status transitions do).
        let current: Option<(i64,)> = sqlx::query_as(
            r"SELECT version FROM scheduled_changes WHERE id = $1 AND deleted_at IS NULL",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(RepositoryError::Database)?;
        let Some((actual,)) = current else {
            return Err(RepositoryError::NotFound { id: id.to_string() });
        };
        if actual != expected_version {
            return Err(RepositoryError::VersionConflict {
                expected: expected_version,
                actual,
            });
        }
        sqlx::query(
            r"
            UPDATE scheduled_changes
            SET deleted_at = NOW(), version = version + 1, updated_at = NOW()
            WHERE id = $1 AND version = $2 AND deleted_at IS NULL
            ",
        )
        .bind(id)
        .bind(expected_version)
        .execute(&self.pool)
        .await
        .map_err(RepositoryError::Database)?;
        Ok(())
    }

    /// Shared optimistic, status-guarded transition helper. `required_status`
    /// is the only status the change may currently be in for the transition to
    /// be valid; otherwise [`RepositoryError::InvalidState`] is returned.
    async fn transition(
        &self,
        id: Uuid,
        expected_version: i64,
        required_status: &str,
        new_status: ScheduleStatus,
    ) -> Result<ScheduledChangeRow, RepositoryError> {
        // Fetch current state to give precise NotFound / VersionConflict /
        // InvalidState errors rather than a silent no-op.
        let sql = format!(
            r"SELECT {SELECT_COLS} FROM scheduled_changes WHERE id = $1 AND deleted_at IS NULL"
        );
        let current = sqlx::query_as::<_, ScheduledChangeRow>(&sql)
            .bind(id)
            .fetch_optional(&self.pool)
            .await
            .map_err(RepositoryError::Database)?
            .ok_or_else(|| RepositoryError::NotFound { id: id.to_string() })?;

        if current.version != expected_version {
            return Err(RepositoryError::VersionConflict {
                expected: expected_version,
                actual: current.version,
            });
        }
        if current.status.as_str() != required_status {
            return Err(RepositoryError::InvalidState {
                reason: format!(
                    "scheduled change {id} is '{}', expected '{required_status}' for this transition",
                    current.status.as_str()
                ),
            });
        }

        let update_sql = format!(
            r"
            UPDATE scheduled_changes
            SET status = $3, version = version + 1, updated_at = NOW()
            WHERE id = $1 AND version = $2 AND deleted_at IS NULL
            RETURNING {SELECT_COLS}
            "
        );
        sqlx::query_as::<_, ScheduledChangeRow>(&update_sql)
            .bind(id)
            .bind(expected_version)
            .bind(new_status.as_str())
            .fetch_one(&self.pool)
            .await
            .map_err(RepositoryError::Database)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn one_shot_input(at: DateTime<Utc>) -> NewScheduledChange {
        NewScheduledChange {
            entity_type: "flag".to_string(),
            entity_id: Uuid::new_v4(),
            env_id: Uuid::new_v4(),
            mutation_payload: serde_json::json!({"kind": "disable"}),
            schedule_kind: ScheduleKind::OneShot,
            scheduled_at: Some(at),
            rrule: None,
            tz: None,
            next_run_at: Some(at),
            created_by: None,
        }
    }

    fn recurring_input(next: DateTime<Utc>) -> NewScheduledChange {
        NewScheduledChange {
            entity_type: "flag".to_string(),
            entity_id: Uuid::new_v4(),
            env_id: Uuid::new_v4(),
            mutation_payload: serde_json::json!({"kind": "enable"}),
            schedule_kind: ScheduleKind::Recurring,
            scheduled_at: None,
            rrule: Some("DTSTART;TZID=UTC:20260101T090000\nRRULE:FREQ=DAILY".to_string()),
            tz: Some("UTC".to_string()),
            next_run_at: Some(next),
            created_by: None,
        }
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn create_one_shot_defaults_pending(pool: PgPool) {
        let repo = ScheduledChangeRepository::new(pool);
        let at = Utc::now();
        let row = repo.create(&one_shot_input(at)).await.unwrap();
        assert_eq!(row.status, ScheduleStatus::Pending);
        assert_eq!(row.schedule_kind, "one_shot");
        assert_eq!(row.version, 1);
        assert_eq!(row.next_run_at, Some(at));
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn create_recurring_defaults_active(pool: PgPool) {
        let repo = ScheduledChangeRepository::new(pool);
        let row = repo.create(&recurring_input(Utc::now())).await.unwrap();
        assert_eq!(row.status, ScheduleStatus::Active);
        assert_eq!(row.schedule_kind, "recurring");
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn get_roundtrips_and_missing_is_not_found(pool: PgPool) {
        let repo = ScheduledChangeRepository::new(pool);
        let created = repo.create(&one_shot_input(Utc::now())).await.unwrap();
        let fetched = repo.get(created.id).await.unwrap();
        assert_eq!(fetched.id, created.id);

        let err = repo.get(Uuid::new_v4()).await.unwrap_err();
        assert!(matches!(err, RepositoryError::NotFound { .. }));
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn list_by_entity_and_env(pool: PgPool) {
        let repo = ScheduledChangeRepository::new(pool);
        let mut input = one_shot_input(Utc::now());
        let entity_id = input.entity_id;
        let env_id = input.env_id;
        repo.create(&input).await.unwrap();
        // Same entity, second schedule.
        input.mutation_payload = serde_json::json!({"kind": "enable"});
        repo.create(&input).await.unwrap();

        let by_entity = repo.list_by_entity("flag", entity_id).await.unwrap();
        assert_eq!(by_entity.len(), 2);

        let by_env = repo.list_by_env(env_id, None).await.unwrap();
        assert_eq!(by_env.len(), 2);

        let by_env_pending = repo
            .list_by_env(env_id, Some(ScheduleStatus::Pending))
            .await
            .unwrap();
        assert_eq!(by_env_pending.len(), 2);
        let by_env_active = repo
            .list_by_env(env_id, Some(ScheduleStatus::Active))
            .await
            .unwrap();
        assert!(by_env_active.is_empty());
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn claim_due_returns_only_due_firable_rows(pool: PgPool) {
        let repo = ScheduledChangeRepository::new(pool);
        let past = Utc::now() - chrono::Duration::hours(1);
        let future = Utc::now() + chrono::Duration::hours(1);

        let due = repo.create(&one_shot_input(past)).await.unwrap();
        let _not_yet = repo.create(&one_shot_input(future)).await.unwrap();

        let mut tx = repo.begin().await.unwrap();
        let claimed = repo.claim_due(&mut tx, Utc::now(), 100).await.unwrap();
        tx.commit().await.unwrap();

        assert_eq!(claimed.len(), 1);
        assert_eq!(claimed[0].id, due.id);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn claim_due_skips_locked_rows(pool: PgPool) {
        // Two scheduler "replicas" sharing the pool. The first claims inside an
        // open transaction; the second must SKIP LOCKED that row and see none.
        let repo = ScheduledChangeRepository::new(pool);
        let past = Utc::now() - chrono::Duration::hours(1);
        repo.create(&one_shot_input(past)).await.unwrap();

        let mut tx1 = repo.begin().await.unwrap();
        let claimed1 = repo.claim_due(&mut tx1, Utc::now(), 100).await.unwrap();
        assert_eq!(claimed1.len(), 1, "first claimer takes the row");

        // Second claimer runs while tx1 still holds the row lock.
        let mut tx2 = repo.begin().await.unwrap();
        let claimed2 = repo.claim_due(&mut tx2, Utc::now(), 100).await.unwrap();
        assert!(claimed2.is_empty(), "locked row must be skipped");
        tx2.commit().await.unwrap();
        tx1.commit().await.unwrap();
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn finalize_one_shot_marks_applied_and_appends_run(pool: PgPool) {
        let repo = ScheduledChangeRepository::new(pool);
        let past = Utc::now() - chrono::Duration::hours(1);
        let row = repo.create(&one_shot_input(past)).await.unwrap();

        let mut tx = repo.begin().await.unwrap();
        let claimed = repo.claim_due(&mut tx, Utc::now(), 100).await.unwrap();
        assert_eq!(claimed.len(), 1);
        repo.finalize_one_shot(&mut tx, row.id, ScheduleStatus::Applied, Utc::now())
            .await
            .unwrap();
        repo.append_run_tx(&mut tx, row.id, RunOutcome::Applied, Some("ok"))
            .await
            .unwrap();
        tx.commit().await.unwrap();

        let after = repo.get(row.id).await.unwrap();
        assert_eq!(after.status, ScheduleStatus::Applied);
        assert!(after.next_run_at.is_none());
        assert!(after.last_run_at.is_some());
        assert_eq!(after.version, 2, "finalize bumps version");

        let runs = repo.list_runs(row.id).await.unwrap();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].outcome, RunOutcome::Applied);
        assert_eq!(runs[0].detail.as_deref(), Some("ok"));

        // Already-applied row is no longer due.
        let mut tx2 = repo.begin().await.unwrap();
        let again = repo.claim_due(&mut tx2, Utc::now(), 100).await.unwrap();
        tx2.commit().await.unwrap();
        assert!(again.is_empty(), "applied one-shot is not re-claimed");
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn advance_recurring_bumps_next_run(pool: PgPool) {
        let repo = ScheduledChangeRepository::new(pool);
        let past = Utc::now() - chrono::Duration::hours(1);
        let row = repo.create(&recurring_input(past)).await.unwrap();

        let next = Utc::now() + chrono::Duration::days(1);
        let mut tx = repo.begin().await.unwrap();
        repo.advance_recurring(&mut tx, row.id, Utc::now(), Some(next))
            .await
            .unwrap();
        repo.append_run_tx(&mut tx, row.id, RunOutcome::Applied, None)
            .await
            .unwrap();
        tx.commit().await.unwrap();

        let after = repo.get(row.id).await.unwrap();
        assert_eq!(after.status, ScheduleStatus::Active, "still active");
        assert_eq!(
            after.next_run_at.map(|t| t.timestamp()),
            Some(next.timestamp())
        );
        assert!(after.last_run_at.is_some());
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn advance_recurring_exhausted_marks_applied(pool: PgPool) {
        let repo = ScheduledChangeRepository::new(pool);
        let past = Utc::now() - chrono::Duration::hours(1);
        let row = repo.create(&recurring_input(past)).await.unwrap();

        let mut tx = repo.begin().await.unwrap();
        repo.advance_recurring(&mut tx, row.id, Utc::now(), None)
            .await
            .unwrap();
        tx.commit().await.unwrap();

        let after = repo.get(row.id).await.unwrap();
        assert_eq!(after.status, ScheduleStatus::Applied);
        assert!(after.next_run_at.is_none());
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn cancel_requires_pending_and_correct_version(pool: PgPool) {
        let repo = ScheduledChangeRepository::new(pool);
        let row = repo.create(&one_shot_input(Utc::now())).await.unwrap();

        // Wrong version → conflict.
        let err = repo.cancel(row.id, 99).await.unwrap_err();
        assert!(matches!(err, RepositoryError::VersionConflict { .. }));

        let cancelled = repo.cancel(row.id, 1).await.unwrap();
        assert_eq!(cancelled.status, ScheduleStatus::Cancelled);

        // Cancelling again (now cancelled, not pending) → invalid state.
        let err = repo.cancel(row.id, 2).await.unwrap_err();
        assert!(matches!(err, RepositoryError::InvalidState { .. }));
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn pause_resume_recurring(pool: PgPool) {
        let repo = ScheduledChangeRepository::new(pool);
        let row = repo.create(&recurring_input(Utc::now())).await.unwrap();

        let paused = repo.pause(row.id, 1).await.unwrap();
        assert_eq!(paused.status, ScheduleStatus::Paused);

        // A paused row is not due-claimable.
        let mut tx = repo.begin().await.unwrap();
        let claimed = repo
            .claim_due(&mut tx, Utc::now() + chrono::Duration::days(1), 100)
            .await
            .unwrap();
        tx.commit().await.unwrap();
        assert!(claimed.is_empty(), "paused rows are not claimed");

        let resumed = repo.resume(row.id, 2).await.unwrap();
        assert_eq!(resumed.status, ScheduleStatus::Active);

        // Cannot pause something that is not active.
        let err = repo.pause(row.id, 99).await.unwrap_err();
        assert!(matches!(err, RepositoryError::VersionConflict { .. }));
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn soft_delete_hides_row(pool: PgPool) {
        let repo = ScheduledChangeRepository::new(pool);
        let row = repo.create(&one_shot_input(Utc::now())).await.unwrap();

        let err = repo.soft_delete(row.id, 99).await.unwrap_err();
        assert!(matches!(err, RepositoryError::VersionConflict { .. }));

        repo.soft_delete(row.id, 1).await.unwrap();

        let err = repo.get(row.id).await.unwrap_err();
        assert!(matches!(err, RepositoryError::NotFound { .. }));

        // Deleting again → NotFound (already gone).
        let err = repo.soft_delete(row.id, 2).await.unwrap_err();
        assert!(matches!(err, RepositoryError::NotFound { .. }));
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn append_run_pool_level(pool: PgPool) {
        let repo = ScheduledChangeRepository::new(pool);
        let row = repo.create(&one_shot_input(Utc::now())).await.unwrap();
        repo.append_run(
            row.id,
            RunOutcome::Skipped,
            Some("flag_locked_by_experiment:abc"),
        )
        .await
        .unwrap();
        let runs = repo.list_runs(row.id).await.unwrap();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].outcome, RunOutcome::Skipped);
    }
}
