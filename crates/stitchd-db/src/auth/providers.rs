//! Auth provider repository — trait definition and Postgres implementation.
//!
//! Manages the `auth_providers` table that stores external authentication
//! provider configurations per organisation.

use async_trait::async_trait;
use chrono::Utc;
use sqlx::PgPool;
use stitchd_core::{
    auth::{AuthProvider, ProviderType},
    id::{AuthProviderId, OrganisationId},
};

use crate::RepositoryError;

// ---------------------------------------------------------------------------
// Trait
// ---------------------------------------------------------------------------

/// Operations on the `auth_providers` table.
#[async_trait]
pub trait AuthProviderRepository: Send + Sync {
    /// Create a new auth provider for the given organisation.
    async fn create(
        &self,
        org_id: OrganisationId,
        provider_type: ProviderType,
        display_name: &str,
        config_encrypted: serde_json::Value,
        enabled: bool,
    ) -> Result<AuthProvider, RepositoryError>;

    /// Find an auth provider by its ID. Returns `None` if absent.
    async fn find_by_id(
        &self,
        id: AuthProviderId,
    ) -> Result<Option<AuthProvider>, RepositoryError>;

    /// List all auth providers for the given organisation.
    async fn list_for_org(
        &self,
        org_id: OrganisationId,
    ) -> Result<Vec<AuthProvider>, RepositoryError>;

    /// Update an existing auth provider.
    async fn update(
        &self,
        id: AuthProviderId,
        display_name: &str,
        config_encrypted: serde_json::Value,
        enabled: bool,
    ) -> Result<AuthProvider, RepositoryError>;

    /// Delete an auth provider.
    ///
    /// Enforces at-least-one-enabled constraint: returns an error if this is
    /// the last enabled provider for the organisation.
    async fn delete(&self, id: AuthProviderId) -> Result<(), RepositoryError>;
}

// ---------------------------------------------------------------------------
// Postgres implementation
// ---------------------------------------------------------------------------

/// Postgres-backed [`AuthProviderRepository`].
pub struct PgAuthProviderRepository {
    /// Shared Postgres connection pool.
    pub pool: PgPool,
}

impl PgAuthProviderRepository {
    /// Construct a new repository bound to `pool`.
    #[must_use]
    pub const fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

/// Parse provider_type TEXT -> ProviderType
fn parse_provider_type(s: &str) -> Result<ProviderType, RepositoryError> {
    match s {
        "password" => Ok(ProviderType::Password),
        "oidc" => Ok(ProviderType::Oidc),
        "saml" => Ok(ProviderType::Saml),
        other => Err(RepositoryError::Unexpected(anyhow::anyhow!(
            "unknown provider_type: {other}"
        ))),
    }
}

/// Convert a DB row to [`AuthProvider`].
macro_rules! map_row {
    ($r:expr) => {
        AuthProvider {
            id: AuthProviderId::from_uuid($r.id),
            org_id: OrganisationId::from_uuid($r.org_id),
            provider_type: parse_provider_type(&$r.provider_type)?,
            display_name: $r.display_name,
            config: $r.config,
            enabled: $r.enabled,
            created_at: $r.created_at,
            updated_at: $r.updated_at,
        }
    };
}

#[async_trait]
impl AuthProviderRepository for PgAuthProviderRepository {
    async fn create(
        &self,
        org_id: OrganisationId,
        provider_type: ProviderType,
        display_name: &str,
        config_encrypted: serde_json::Value,
        enabled: bool,
    ) -> Result<AuthProvider, RepositoryError> {
        let now = Utc::now();
        let pt_str = match provider_type {
            ProviderType::Password => "password",
            ProviderType::Oidc => "oidc",
            ProviderType::Saml => "saml",
        };
        let row = sqlx::query!(
            r#"
            INSERT INTO auth_providers
                (org_id, provider_type, display_name, config, enabled, created_at, updated_at)
            VALUES ($1, $2, $3, $4, $5, $6, $6)
            RETURNING
                id,
                org_id,
                provider_type,
                display_name,
                config,
                enabled,
                created_at,
                updated_at
            "#,
            org_id.as_uuid(),
            pt_str,
            display_name,
            config_encrypted,
            enabled,
            now,
        )
        .fetch_one(&self.pool)
        .await
        .map_err(RepositoryError::Database)?;

        Ok(map_row!(row))
    }

    async fn find_by_id(
        &self,
        id: AuthProviderId,
    ) -> Result<Option<AuthProvider>, RepositoryError> {
        let row = sqlx::query!(
            r#"
            SELECT
                id,
                org_id,
                provider_type,
                display_name,
                config,
                enabled,
                created_at,
                updated_at
            FROM auth_providers
            WHERE id = $1
            "#,
            id.as_uuid(),
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(RepositoryError::Database)?;

        row.map(|r| -> Result<AuthProvider, RepositoryError> { Ok(map_row!(r)) })
            .transpose()
    }

    async fn list_for_org(
        &self,
        org_id: OrganisationId,
    ) -> Result<Vec<AuthProvider>, RepositoryError> {
        let rows = sqlx::query!(
            r#"
            SELECT
                id,
                org_id,
                provider_type,
                display_name,
                config,
                enabled,
                created_at,
                updated_at
            FROM auth_providers
            WHERE org_id = $1
            ORDER BY created_at ASC
            "#,
            org_id.as_uuid(),
        )
        .fetch_all(&self.pool)
        .await
        .map_err(RepositoryError::Database)?;

        rows.into_iter()
            .map(|r| -> Result<AuthProvider, RepositoryError> { Ok(map_row!(r)) })
            .collect()
    }

    async fn update(
        &self,
        id: AuthProviderId,
        display_name: &str,
        config_encrypted: serde_json::Value,
        enabled: bool,
    ) -> Result<AuthProvider, RepositoryError> {
        let row = sqlx::query!(
            r#"
            UPDATE auth_providers
            SET
                display_name = $1,
                config = $2,
                enabled = $3,
                updated_at = now()
            WHERE id = $4
            RETURNING
                id,
                org_id,
                provider_type,
                display_name,
                config,
                enabled,
                created_at,
                updated_at
            "#,
            display_name,
            config_encrypted,
            enabled,
            id.as_uuid(),
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(RepositoryError::Database)?;

        row.map(|r| -> Result<AuthProvider, RepositoryError> { Ok(map_row!(r)) })
            .transpose()?
            .ok_or_else(|| RepositoryError::NotFound {
                id: id.to_string(),
            })
    }

    async fn delete(&self, id: AuthProviderId) -> Result<(), RepositoryError> {
        // Find the provider to get org_id
        let row = sqlx::query!(
            r#"
            SELECT org_id, enabled FROM auth_providers WHERE id = $1
            "#,
            id.as_uuid(),
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(RepositoryError::Database)?;

        let row = row.ok_or_else(|| RepositoryError::NotFound {
            id: id.to_string(),
        })?;

        // Enforce at-least-one-enabled constraint
        if row.enabled {
            let count: i64 = sqlx::query_scalar!(
                r#"
                SELECT COUNT(*) FROM auth_providers
                WHERE org_id = $1 AND enabled = true
                "#,
                row.org_id,
            )
            .fetch_one(&self.pool)
            .await
            .map_err(RepositoryError::Database)?
            .unwrap_or(0);

            if count <= 1 {
                return Err(RepositoryError::Unexpected(anyhow::anyhow!(
                    "Cannot delete the last enabled auth provider for the organisation. \
                     At least one enabled provider must remain."
                )));
            }
        }

        sqlx::query!(
            r#"DELETE FROM auth_providers WHERE id = $1"#,
            id.as_uuid(),
        )
        .execute(&self.pool)
        .await
        .map_err(RepositoryError::Database)?;

        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Integration tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::PgPool;

    async fn seed_org(pool: &PgPool) -> OrganisationId {
        let org_id = OrganisationId::new();
        sqlx::query!(
            "INSERT INTO organisations (id, name) VALUES ($1, $2)",
            org_id.as_uuid(),
            "test-org"
        )
        .execute(pool)
        .await
        .unwrap();
        org_id
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn create_returns_provider(pool: PgPool) {
        let repo = PgAuthProviderRepository::new(pool.clone());
        let org_id = seed_org(&pool).await;

        let config = serde_json::json!({ "client_id": "abc", "client_secret_enc": "xyz" });
        let provider = repo
            .create(org_id, ProviderType::Oidc, "Google OIDC", config.clone(), true)
            .await
            .unwrap();

        assert_eq!(provider.org_id, org_id);
        assert_eq!(provider.provider_type, ProviderType::Oidc);
        assert_eq!(provider.display_name, "Google OIDC");
        assert_eq!(provider.config, config);
        assert!(provider.enabled);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn find_by_id_existing(pool: PgPool) {
        let repo = PgAuthProviderRepository::new(pool.clone());
        let org_id = seed_org(&pool).await;

        let created = repo
            .create(org_id, ProviderType::Oidc, "Test Provider", serde_json::json!({}), true)
            .await
            .unwrap();

        let found = repo.find_by_id(created.id).await.unwrap();
        assert!(found.is_some());
        assert_eq!(found.unwrap().id, created.id);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn find_by_id_missing_returns_none(pool: PgPool) {
        let repo = PgAuthProviderRepository::new(pool);
        let found = repo.find_by_id(AuthProviderId::new()).await.unwrap();
        assert!(found.is_none());
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn list_for_org_multiple(pool: PgPool) {
        let repo = PgAuthProviderRepository::new(pool.clone());
        let org_id = seed_org(&pool).await;

        repo.create(org_id, ProviderType::Oidc, "Provider A", serde_json::json!({}), true)
            .await
            .unwrap();
        repo.create(org_id, ProviderType::Saml, "Provider B", serde_json::json!({}), true)
            .await
            .unwrap();

        let list = repo.list_for_org(org_id).await.unwrap();
        assert_eq!(list.len(), 2);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn update_provider(pool: PgPool) {
        let repo = PgAuthProviderRepository::new(pool.clone());
        let org_id = seed_org(&pool).await;

        let created = repo
            .create(org_id, ProviderType::Oidc, "Original", serde_json::json!({}), true)
            .await
            .unwrap();

        let updated = repo
            .update(
                created.id,
                "Updated Name",
                serde_json::json!({ "key": "value" }),
                false,
            )
            .await
            .unwrap();

        assert_eq!(updated.display_name, "Updated Name");
        assert!(!updated.enabled);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn delete_success_when_multiple_enabled(pool: PgPool) {
        let repo = PgAuthProviderRepository::new(pool.clone());
        let org_id = seed_org(&pool).await;

        let p1 = repo
            .create(org_id, ProviderType::Oidc, "Provider 1", serde_json::json!({}), true)
            .await
            .unwrap();
        repo.create(org_id, ProviderType::Saml, "Provider 2", serde_json::json!({}), true)
            .await
            .unwrap();

        repo.delete(p1.id).await.unwrap();
        let list = repo.list_for_org(org_id).await.unwrap();
        assert_eq!(list.len(), 1);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn delete_last_enabled_returns_error(pool: PgPool) {
        let repo = PgAuthProviderRepository::new(pool.clone());
        let org_id = seed_org(&pool).await;

        let provider = repo
            .create(org_id, ProviderType::Oidc, "Only Provider", serde_json::json!({}), true)
            .await
            .unwrap();

        let err = repo.delete(provider.id).await.unwrap_err();
        assert!(
            matches!(err, RepositoryError::Unexpected(_)),
            "expected Unexpected error for at-least-one-enabled, got {err:?}"
        );
    }
}
