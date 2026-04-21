//! MFA challenge repository — minimal stub for Phase 3 login flow.
//!
//! The full MFA TOTP flow (enable, verify, recovery codes) is Phase 4.
//! This module only provides enough to let `/auth/login` create a challenge
//! token when `totp_enabled = true`.

use async_trait::async_trait;
use chrono::{Duration, Utc};
use sqlx::PgPool;
use stitchd_core::{auth::crypto::generate_opaque_token, id::UserId};

use crate::RepositoryError;

// ---------------------------------------------------------------------------
// Trait
// ---------------------------------------------------------------------------

/// Minimal MFA challenge repository required by the login flow.
#[async_trait]
pub trait MfaChallengeRepository: Send + Sync {
    /// Create a short-lived challenge for `user_id`.
    ///
    /// Returns `(challenge_token_hash, raw_token)`:
    /// - `challenge_token_hash` is stored in the DB.
    /// - `raw_token` is returned to the caller once (sent to the client as `challenge_token`).
    async fn create_challenge(
        &self,
        user_id: UserId,
        ttl_secs: i64,
    ) -> Result<(String, String), RepositoryError>;
}

// ---------------------------------------------------------------------------
// Postgres implementation
// ---------------------------------------------------------------------------

/// Postgres-backed [`MfaChallengeRepository`].
pub struct PgMfaChallengeRepository {
    /// Shared Postgres connection pool.
    pub pool: PgPool,
}

impl PgMfaChallengeRepository {
    /// Construct a new repository bound to `pool`.
    #[must_use]
    pub const fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl MfaChallengeRepository for PgMfaChallengeRepository {
    async fn create_challenge(
        &self,
        user_id: UserId,
        ttl_secs: i64,
    ) -> Result<(String, String), RepositoryError> {
        let (raw_token, token_hash) = generate_opaque_token();
        let expires_at = Utc::now() + Duration::seconds(ttl_secs);

        sqlx::query!(
            r#"
            INSERT INTO mfa_challenges (user_id, challenge_token_hash, expires_at)
            VALUES ($1, $2, $3)
            "#,
            user_id.as_uuid(),
            token_hash,
            expires_at,
        )
        .execute(&self.pool)
        .await
        .map_err(RepositoryError::Database)?;

        Ok((token_hash, raw_token))
    }
}
