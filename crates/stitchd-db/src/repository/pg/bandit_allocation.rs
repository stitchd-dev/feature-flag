//! Postgres reads over `bandit_allocation_runs` + the experiments convergence
//! columns, for the FR7 surfacing layer (`bandit_20260608`, Phase 11).
//!
//! These are pure reads — the WRITE path (the per-tick reallocation/commit/
//! rollout rows) lives in stats-service (`PgRunRecorder`). The reads here power
//! the Admin UI's Bandit Results view:
//!   * `latest_reallocation` — the current per-arm allocation + per-objective
//!     posteriors (the most recent `reallocate` row's `new_allocation` JSONB).
//!   * `list_runs` — the full allocation timeline (newest first), the data
//!     behind the allocation-over-time chart + lifecycle-action timeline.
//!   * `find_convergence` — the persisted convergence state
//!     (`experiments.bandit_converged_variant` / `_prob`).
//!
//! All queries use raw `sqlx::query` (not the `query!` macro) to avoid
//! offline-cache coupling, matching the project's repository pattern.

use async_trait::async_trait;
use sqlx::{PgPool, Row};
use uuid::Uuid;

use crate::RepositoryError;

/// One `bandit_allocation_runs` row, surfaced for the timeline.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BanditAllocationRunRow {
    /// When the action fired, as milliseconds since the Unix epoch.
    pub fired_at_ms: i64,
    /// `reallocate` | `commit` | `rollout` | `spawn_iteration` | `skip`.
    pub action: String,
    /// `applied` | `skipped` | `failed`.
    pub outcome: String,
    /// The prior allocation JSON (`None` when the column was NULL).
    pub old_allocation: Option<serde_json::Value>,
    /// The new allocation JSON, incl. the `bandit_objectives` key for a
    /// `reallocate` row (`None` when the column was NULL).
    pub new_allocation: Option<serde_json::Value>,
    /// Free-text reason / error detail (`None` when the column was NULL).
    pub detail: Option<String>,
}

/// The persisted convergence state for a bandit experiment.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct BanditConvergence {
    /// The winning variant key (`None` until the experiment first converges).
    pub variant: Option<String>,
    /// Its posterior probability-to-be-best in `[0, 1]` (`None` until converged).
    pub prob: Option<f64>,
}

/// Read-only operations over `bandit_allocation_runs` + the convergence columns.
#[async_trait]
pub trait BanditAllocationRepository: Send + Sync {
    /// The most recent `reallocate` row for an experiment (the current
    /// allocation + per-objective posteriors). `None` when no reallocation has
    /// fired yet.
    async fn latest_reallocation(
        &self,
        experiment_id: Uuid,
    ) -> Result<Option<BanditAllocationRunRow>, RepositoryError>;

    /// The allocation-run timeline for an experiment, newest first, capped at
    /// `limit` rows.
    async fn list_runs(
        &self,
        experiment_id: Uuid,
        limit: i64,
    ) -> Result<Vec<BanditAllocationRunRow>, RepositoryError>;

    /// The persisted convergence state (`bandit_converged_variant` / `_prob`).
    /// Returns `BanditConvergence { variant: None, prob: None }` when the
    /// experiment exists but has not converged, and `NotFound` when the
    /// experiment row is absent.
    async fn find_convergence(
        &self,
        experiment_id: Uuid,
    ) -> Result<BanditConvergence, RepositoryError>;
}

/// Postgres-backed [`BanditAllocationRepository`].
pub struct PgBanditAllocationRepository {
    pool: PgPool,
}

impl PgBanditAllocationRepository {
    /// Construct bound to `pool`.
    #[must_use]
    pub const fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

/// Map a `bandit_allocation_runs` row to [`BanditAllocationRunRow`].
fn row_to_run(row: &sqlx::postgres::PgRow) -> BanditAllocationRunRow {
    let fired_at: chrono::DateTime<chrono::Utc> = row.get("fired_at");
    BanditAllocationRunRow {
        fired_at_ms: fired_at.timestamp_millis(),
        action: row.get("action"),
        outcome: row.get("outcome"),
        old_allocation: row.get("old_allocation"),
        new_allocation: row.get("new_allocation"),
        detail: row.get("detail"),
    }
}

#[async_trait]
impl BanditAllocationRepository for PgBanditAllocationRepository {
    async fn latest_reallocation(
        &self,
        experiment_id: Uuid,
    ) -> Result<Option<BanditAllocationRunRow>, RepositoryError> {
        let row = sqlx::query(
            "SELECT fired_at, action, outcome, old_allocation, new_allocation, detail \
             FROM bandit_allocation_runs \
             WHERE experiment_id = $1 AND action = 'reallocate' AND outcome = 'applied' \
             ORDER BY fired_at DESC \
             LIMIT 1",
        )
        .bind(experiment_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(RepositoryError::Database)?;
        Ok(row.as_ref().map(row_to_run))
    }

    async fn list_runs(
        &self,
        experiment_id: Uuid,
        limit: i64,
    ) -> Result<Vec<BanditAllocationRunRow>, RepositoryError> {
        let rows = sqlx::query(
            "SELECT fired_at, action, outcome, old_allocation, new_allocation, detail \
             FROM bandit_allocation_runs \
             WHERE experiment_id = $1 \
             ORDER BY fired_at DESC \
             LIMIT $2",
        )
        .bind(experiment_id)
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .map_err(RepositoryError::Database)?;
        Ok(rows.iter().map(row_to_run).collect())
    }

    async fn find_convergence(
        &self,
        experiment_id: Uuid,
    ) -> Result<BanditConvergence, RepositoryError> {
        let row = sqlx::query(
            "SELECT bandit_converged_variant, bandit_converged_prob \
             FROM experiments WHERE id = $1 AND deleted_at IS NULL",
        )
        .bind(experiment_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(RepositoryError::Database)?
        .ok_or(RepositoryError::NotFound {
            id: experiment_id.to_string(),
        })?;
        Ok(BanditConvergence {
            variant: row.get("bandit_converged_variant"),
            prob: row.get("bandit_converged_prob"),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Seed the org→project→env→flag→experiment FK chain a
    /// `bandit_allocation_runs` row needs. Returns the experiment id.
    async fn seed_experiment(pool: &PgPool) -> Uuid {
        let org = Uuid::new_v4();
        let project = Uuid::new_v4();
        let env = Uuid::new_v4();
        let flag = Uuid::new_v4();
        let exp = Uuid::new_v4();
        sqlx::query("INSERT INTO organisations (id, name) VALUES ($1, $2)")
            .bind(org)
            .bind("alloc-org")
            .execute(pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO projects (id, organisation_id, name) VALUES ($1, $2, $3)")
            .bind(project)
            .bind(org)
            .bind("alloc-proj")
            .execute(pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO environments (id, project_id, name) VALUES ($1, $2, $3)")
            .bind(env)
            .bind(project)
            .bind("alloc-env")
            .execute(pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO feature_flags (id, project_id, key, name, value_type, enabled) \
             VALUES ($1, $2, $3, $4, $5, $6)",
        )
        .bind(flag)
        .bind(project)
        .bind(format!("alloc_flag_{}", &flag.to_string()[..8]))
        .bind("alloc flag")
        .bind("boolean")
        .bind(true)
        .execute(pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO experiments \
             (id, env_id, flag_id, name, status, targets_default_rule) \
             VALUES ($1, $2, $3, $4, 'running', true)",
        )
        .bind(exp)
        .bind(env)
        .bind(flag)
        .bind("alloc exp")
        .execute(pool)
        .await
        .unwrap();
        exp
    }

    async fn insert_run(
        pool: &PgPool,
        experiment_id: Uuid,
        fired_at: &str,
        action: &str,
        outcome: &str,
        new_allocation: Option<serde_json::Value>,
        detail: Option<&str>,
    ) {
        sqlx::query(
            "INSERT INTO bandit_allocation_runs \
             (experiment_id, fired_at, action, outcome, new_allocation, detail) \
             VALUES ($1, $2::timestamptz, $3, $4, $5, $6)",
        )
        .bind(experiment_id)
        .bind(fired_at)
        .bind(action)
        .bind(outcome)
        .bind(new_allocation)
        .bind(detail)
        .execute(pool)
        .await
        .unwrap();
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn latest_reallocation_returns_most_recent_applied(pool: PgPool) {
        let exp = seed_experiment(&pool).await;
        let repo = PgBanditAllocationRepository::new(pool.clone());

        // No runs yet → None.
        assert!(repo.latest_reallocation(exp).await.unwrap().is_none());

        insert_run(
            &pool,
            exp,
            "2026-06-01T00:00:00Z",
            "reallocate",
            "applied",
            Some(serde_json::json!({"control": 5000, "treatment": 5000})),
            None,
        )
        .await;
        insert_run(
            &pool,
            exp,
            "2026-06-02T00:00:00Z",
            "reallocate",
            "applied",
            Some(serde_json::json!({
                "control": 3000,
                "treatment": 7000,
                "bandit_objectives": {"objectives": []}
            })),
            None,
        )
        .await;
        // A skip row must NOT shadow the latest applied reallocate.
        insert_run(
            &pool,
            exp,
            "2026-06-03T00:00:00Z",
            "skip",
            "skipped",
            None,
            Some("flag locked"),
        )
        .await;

        let latest = repo.latest_reallocation(exp).await.unwrap().unwrap();
        assert_eq!(latest.action, "reallocate");
        assert_eq!(latest.outcome, "applied");
        let alloc = latest.new_allocation.unwrap();
        assert_eq!(alloc["treatment"], 7000);
        assert!(alloc.get("bandit_objectives").is_some());
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn list_runs_orders_desc_and_respects_limit(pool: PgPool) {
        let exp = seed_experiment(&pool).await;
        let repo = PgBanditAllocationRepository::new(pool.clone());

        insert_run(
            &pool,
            exp,
            "2026-06-01T00:00:00Z",
            "reallocate",
            "applied",
            Some(serde_json::json!({"a": 5000, "b": 5000})),
            None,
        )
        .await;
        insert_run(
            &pool,
            exp,
            "2026-06-02T00:00:00Z",
            "commit",
            "applied",
            Some(serde_json::json!({"b": 10000})),
            None,
        )
        .await;
        insert_run(
            &pool,
            exp,
            "2026-06-03T00:00:00Z",
            "rollout",
            "applied",
            None,
            Some("auto-rollout"),
        )
        .await;

        let all = repo.list_runs(exp, 50).await.unwrap();
        assert_eq!(all.len(), 3);
        // Newest first.
        assert_eq!(all[0].action, "rollout");
        assert_eq!(all[1].action, "commit");
        assert_eq!(all[2].action, "reallocate");
        assert!(all[2].fired_at_ms < all[1].fired_at_ms);

        let limited = repo.list_runs(exp, 2).await.unwrap();
        assert_eq!(limited.len(), 2);
        assert_eq!(limited[0].action, "rollout");
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn find_convergence_reads_columns(pool: PgPool) {
        let exp = seed_experiment(&pool).await;
        let repo = PgBanditAllocationRepository::new(pool.clone());

        // Not converged yet → both None.
        let c = repo.find_convergence(exp).await.unwrap();
        assert_eq!(c.variant, None);
        assert_eq!(c.prob, None);

        sqlx::query(
            "UPDATE experiments \
             SET bandit_converged_variant = $2, bandit_converged_prob = $3 WHERE id = $1",
        )
        .bind(exp)
        .bind("treatment")
        .bind(0.97_f64)
        .execute(&pool)
        .await
        .unwrap();

        let c = repo.find_convergence(exp).await.unwrap();
        assert_eq!(c.variant.as_deref(), Some("treatment"));
        assert!((c.prob.unwrap() - 0.97).abs() < 1e-9);

        // Missing experiment → NotFound.
        let missing = repo.find_convergence(Uuid::new_v4()).await.unwrap_err();
        assert!(matches!(missing, RepositoryError::NotFound { .. }));
    }
}
