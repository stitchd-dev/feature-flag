//! Postgres implementation for the bandit-campaign repository.
//!
//! CRUD over `bandit_campaigns` (an operator-configured-once autonomous
//! optimization campaign that auto-spawns successive experiment iterations on one
//! flag) plus the spawn-counter primitive that makes auto-spawn idempotent +
//! bounded.
//!
//! All queries use raw `sqlx::query`/`query_scalar` (not the `query!` macro) to
//! avoid offline-cache coupling, matching the project's repository pattern.

use async_trait::async_trait;
use sqlx::{PgPool, Row};
use uuid::Uuid;

use stitchd_core::experimentation::bandit::{
    BanditCampaign, BanditCampaignConfig, BanditCampaignStatus,
};

use crate::RepositoryError;

/// CRUD + bounded-spawn operations for [`BanditCampaign`] records.
#[async_trait]
pub trait BanditCampaignRepository: Send + Sync {
    /// Create a new active campaign on `flag_id`. Returns the persisted row.
    async fn create(
        &self,
        environment_id: Uuid,
        flag_id: Uuid,
        name: &str,
        config: &BanditCampaignConfig,
    ) -> Result<BanditCampaign, RepositoryError>;

    /// Fetch a campaign by id.
    async fn find_by_id(&self, id: Uuid) -> Result<BanditCampaign, RepositoryError>;

    /// List all campaigns for an environment (newest first).
    async fn list_by_environment(
        &self,
        environment_id: Uuid,
    ) -> Result<Vec<BanditCampaign>, RepositoryError>;

    /// Set a campaign's status (e.g. `Cancelled` for a `StopBanditCampaign`,
    /// `Completed` when a cap is hit). Optimistic-version bumped. Returns the
    /// updated row.
    async fn set_status(
        &self,
        id: Uuid,
        status: BanditCampaignStatus,
        expected_version: i64,
    ) -> Result<BanditCampaign, RepositoryError>;

    /// Atomically record that a campaign spawned one iteration: increment
    /// `iterations_spawned` and bump `version`, **only if** the row is still at
    /// `expected_version`, still `active`, and `iterations_spawned <
    /// max_iterations`. This is the idempotency + cap primitive: a stale-version
    /// (already-spawned) or capped caller gets `Ok(None)` and must NOT spawn.
    ///
    /// Returns `Ok(Some(updated))` when the slot was claimed (caller should spawn
    /// exactly one iteration), `Ok(None)` when it was not (idempotent no-op:
    /// already spawned by a concurrent/previous tick, or the cap is reached —
    /// in which case the caller should finalize the campaign).
    async fn try_claim_spawn(
        &self,
        id: Uuid,
        expected_version: i64,
    ) -> Result<Option<BanditCampaign>, RepositoryError>;
}

/// Postgres-backed [`BanditCampaignRepository`].
pub struct PgBanditCampaignRepository {
    pool: PgPool,
}

impl PgBanditCampaignRepository {
    /// Construct bound to `pool`.
    #[must_use]
    pub const fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

/// Map a `bandit_campaigns` row to the domain [`BanditCampaign`].
fn row_to_campaign(row: &sqlx::postgres::PgRow) -> Result<BanditCampaign, RepositoryError> {
    let config_json: serde_json::Value = row.get("config");
    let config: BanditCampaignConfig = serde_json::from_value(config_json).map_err(|e| {
        RepositoryError::Unexpected(anyhow::anyhow!("invalid campaign config JSON: {e}"))
    })?;
    let status_str: String = row.get("status");
    Ok(BanditCampaign {
        id: row.get("id"),
        environment_id: row.get("environment_id"),
        flag_id: row.get("flag_id"),
        name: row.get("name"),
        config,
        status: BanditCampaignStatus::from_str_or_active(&status_str),
        iterations_spawned: row.get("iterations_spawned"),
        version: row.get("version"),
    })
}

#[async_trait]
impl BanditCampaignRepository for PgBanditCampaignRepository {
    async fn create(
        &self,
        environment_id: Uuid,
        flag_id: Uuid,
        name: &str,
        config: &BanditCampaignConfig,
    ) -> Result<BanditCampaign, RepositoryError> {
        let config_json = serde_json::to_value(config).map_err(|e| {
            RepositoryError::Unexpected(anyhow::anyhow!("serialize campaign config: {e}"))
        })?;
        let row = sqlx::query(
            "INSERT INTO bandit_campaigns \
             (environment_id, flag_id, name, config, status, iterations_spawned, version) \
             VALUES ($1, $2, $3, $4, 'active', 0, 0) \
             RETURNING id, environment_id, flag_id, name, config, status, \
                       iterations_spawned, version",
        )
        .bind(environment_id)
        .bind(flag_id)
        .bind(name)
        .bind(config_json)
        .fetch_one(&self.pool)
        .await
        .map_err(RepositoryError::Database)?;
        row_to_campaign(&row)
    }

    async fn find_by_id(&self, id: Uuid) -> Result<BanditCampaign, RepositoryError> {
        let row = sqlx::query(
            "SELECT id, environment_id, flag_id, name, config, status, \
                    iterations_spawned, version \
             FROM bandit_campaigns WHERE id = $1",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(RepositoryError::Database)?
        .ok_or(RepositoryError::NotFound { id: id.to_string() })?;
        row_to_campaign(&row)
    }

    async fn list_by_environment(
        &self,
        environment_id: Uuid,
    ) -> Result<Vec<BanditCampaign>, RepositoryError> {
        let rows = sqlx::query(
            "SELECT id, environment_id, flag_id, name, config, status, \
                    iterations_spawned, version \
             FROM bandit_campaigns WHERE environment_id = $1 \
             ORDER BY created_at DESC",
        )
        .bind(environment_id)
        .fetch_all(&self.pool)
        .await
        .map_err(RepositoryError::Database)?;
        rows.iter().map(row_to_campaign).collect()
    }

    async fn set_status(
        &self,
        id: Uuid,
        status: BanditCampaignStatus,
        expected_version: i64,
    ) -> Result<BanditCampaign, RepositoryError> {
        let row = sqlx::query(
            "UPDATE bandit_campaigns \
             SET status = $2, version = version + 1, updated_at = now() \
             WHERE id = $1 AND version = $3 \
             RETURNING id, environment_id, flag_id, name, config, status, \
                       iterations_spawned, version",
        )
        .bind(id)
        .bind(status.as_str())
        .bind(expected_version)
        .fetch_optional(&self.pool)
        .await
        .map_err(RepositoryError::Database)?;
        if let Some(r) = row {
            row_to_campaign(&r)
        } else {
            // Either the row is gone or the version moved. Disambiguate.
            let cur = self.find_by_id(id).await?;
            Err(RepositoryError::VersionConflict {
                expected: expected_version,
                actual: cur.version,
            })
        }
    }

    async fn try_claim_spawn(
        &self,
        id: Uuid,
        expected_version: i64,
    ) -> Result<Option<BanditCampaign>, RepositoryError> {
        // Atomic, conditional increment: claims the spawn slot only if still at
        // the expected version, still active, and under the max_iterations cap.
        // The cap read is inlined from the JSONB config so the whole check is one
        // statement (no read-modify-write race).
        let row = sqlx::query(
            "UPDATE bandit_campaigns \
             SET iterations_spawned = iterations_spawned + 1, \
                 version = version + 1, updated_at = now() \
             WHERE id = $1 AND version = $2 AND status = 'active' \
               AND iterations_spawned < (config->>'max_iterations')::int \
             RETURNING id, environment_id, flag_id, name, config, status, \
                       iterations_spawned, version",
        )
        .bind(id)
        .bind(expected_version)
        .fetch_optional(&self.pool)
        .await
        .map_err(RepositoryError::Database)?;
        match row {
            Some(r) => Ok(Some(row_to_campaign(&r)?)),
            None => Ok(None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use stitchd_core::experimentation::bandit::VariantDiscoveryPolicy;

    /// Seed the org→project→env→flag FK chain the `bandit_campaigns` row needs.
    /// Returns `(environment_id, flag_id)`.
    async fn seed_chain(pool: &PgPool) -> (Uuid, Uuid) {
        let org = Uuid::new_v4();
        let project = Uuid::new_v4();
        let env = Uuid::new_v4();
        let flag = Uuid::new_v4();
        sqlx::query("INSERT INTO organisations (id, name) VALUES ($1, $2)")
            .bind(org)
            .bind("camp-org")
            .execute(pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO projects (id, organisation_id, name) VALUES ($1, $2, $3)")
            .bind(project)
            .bind(org)
            .bind("camp-proj")
            .execute(pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO environments (id, project_id, name) VALUES ($1, $2, $3)")
            .bind(env)
            .bind(project)
            .bind("camp-env")
            .execute(pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO feature_flags (id, project_id, key, name, value_type, enabled) \
             VALUES ($1, $2, $3, $4, $5, $6)",
        )
        .bind(flag)
        .bind(project)
        .bind(format!("camp_flag_{}", &flag.to_string()[..8]))
        .bind("camp flag")
        .bind("boolean")
        .bind(true)
        .execute(pool)
        .await
        .unwrap();
        (env, flag)
    }

    fn cfg(max: u32) -> BanditCampaignConfig {
        BanditCampaignConfig {
            max_iterations: max,
            drift_threshold: 0.2,
            variant_discovery: VariantDiscoveryPolicy::WinnerPlusNew,
            budget_cap: None,
        }
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn create_get_list_roundtrip(pool: PgPool) {
        let (env, flag) = seed_chain(&pool).await;
        let repo = PgBanditCampaignRepository::new(pool);
        let c = repo.create(env, flag, "camp", &cfg(3)).await.unwrap();
        assert_eq!(c.status, BanditCampaignStatus::Active);
        assert_eq!(c.iterations_spawned, 0);
        assert_eq!(c.version, 0);
        assert_eq!(c.config.max_iterations, 3);

        let got = repo.find_by_id(c.id).await.unwrap();
        assert_eq!(got, c);

        let list = repo.list_by_environment(env).await.unwrap();
        assert_eq!(list.len(), 1);

        let missing = repo.find_by_id(Uuid::new_v4()).await.unwrap_err();
        assert!(matches!(missing, RepositoryError::NotFound { .. }));
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn stop_sets_cancelled(pool: PgPool) {
        let (env, flag) = seed_chain(&pool).await;
        let repo = PgBanditCampaignRepository::new(pool);
        let c = repo.create(env, flag, "camp", &cfg(3)).await.unwrap();
        let stopped = repo
            .set_status(c.id, BanditCampaignStatus::Cancelled, c.version)
            .await
            .unwrap();
        assert_eq!(stopped.status, BanditCampaignStatus::Cancelled);
        assert_eq!(stopped.version, 1);
        // Stale version → conflict.
        let conflict = repo
            .set_status(c.id, BanditCampaignStatus::Paused, c.version)
            .await
            .unwrap_err();
        assert!(matches!(conflict, RepositoryError::VersionConflict { .. }));
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn claim_spawn_is_idempotent_and_bounded(pool: PgPool) {
        let (env, flag) = seed_chain(&pool).await;
        let repo = PgBanditCampaignRepository::new(pool);
        let c = repo.create(env, flag, "camp", &cfg(2)).await.unwrap();

        // First claim at version 0 succeeds → iterations_spawned 1, version 1.
        let first = repo.try_claim_spawn(c.id, 0).await.unwrap();
        let first = first.expect("first claim succeeds");
        assert_eq!(first.iterations_spawned, 1);
        assert_eq!(first.version, 1);

        // Re-claiming with the SAME (stale) version 0 is a no-op (idempotent):
        // the same convergence event can't double-spawn.
        assert!(repo.try_claim_spawn(c.id, 0).await.unwrap().is_none());

        // Second distinct claim at the new version 1 succeeds → spawned 2 (= cap).
        let second = repo.try_claim_spawn(c.id, 1).await.unwrap();
        assert_eq!(second.unwrap().iterations_spawned, 2);

        // Cap reached: a further claim at the current version is refused.
        let cur = repo.find_by_id(c.id).await.unwrap();
        assert!(
            repo.try_claim_spawn(c.id, cur.version)
                .await
                .unwrap()
                .is_none()
        );
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn claim_spawn_refused_when_not_active(pool: PgPool) {
        let (env, flag) = seed_chain(&pool).await;
        let repo = PgBanditCampaignRepository::new(pool);
        let c = repo.create(env, flag, "camp", &cfg(5)).await.unwrap();
        let cancelled = repo
            .set_status(c.id, BanditCampaignStatus::Cancelled, c.version)
            .await
            .unwrap();
        assert!(
            repo.try_claim_spawn(c.id, cancelled.version)
                .await
                .unwrap()
                .is_none(),
            "a cancelled campaign cannot spawn"
        );
    }
}
