//! MFA repository — challenges, TOTP enable/disable, recovery codes.
//!
//! Provides:
//! - [`MfaRepository`] trait with full MFA operations
//! - [`PgMfaRepository`] Postgres implementation

use async_trait::async_trait;
use chrono::{Duration, Utc};
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use stitchd_core::{
    auth::crypto::generate_opaque_token,
    id::{MfaChallengeId, UserId},
};

use crate::RepositoryError;

// ---------------------------------------------------------------------------
// Trait
// ---------------------------------------------------------------------------

/// Full MFA repository required by Phase 4.
#[async_trait]
pub trait MfaRepository: Send + Sync {
    /// Create a short-lived MFA challenge token.
    ///
    /// Returns `(MfaChallengeId, raw_token)`:
    /// - `raw_token` is sent to the client once (as `challenge_token`).
    /// - A SHA-256 hash of the raw token is stored in the DB.
    async fn create_challenge(
        &self,
        user_id: UserId,
        ttl_secs: i64,
    ) -> Result<(MfaChallengeId, String), RepositoryError>;

    /// Consume a challenge by its token hash.
    ///
    /// Sets `used_at = now()` and returns `Some(MfaChallengeId)` if a matching
    /// unexpired, unused challenge exists.  Returns `None` if expired or already used.
    async fn consume_challenge(
        &self,
        token_hash: &str,
    ) -> Result<Option<MfaChallengeId>, RepositoryError>;

    /// Enable TOTP for a user.
    ///
    /// Stores the AES-encrypted TOTP secret on the user row
    /// (`totp_secret = encrypted_secret`, `totp_enabled = true`) and atomically
    /// replaces all recovery code hashes for the user.
    async fn enable_totp(
        &self,
        user_id: UserId,
        encrypted_secret: Vec<u8>,
        recovery_code_hashes: Vec<String>,
    ) -> Result<(), RepositoryError>;

    /// Disable TOTP for a user.
    ///
    /// Clears `totp_secret`, sets `totp_enabled = false`, and deletes all
    /// recovery codes for the user.
    async fn disable_totp(&self, user_id: UserId) -> Result<(), RepositoryError>;

    /// Return the raw encrypted TOTP secret bytes, or `None` if the user has not
    /// stored a TOTP secret yet.
    async fn get_totp_secret(&self, user_id: UserId) -> Result<Option<Vec<u8>>, RepositoryError>;

    /// Consume a recovery code by its Argon2id hash.
    ///
    /// Sets `used_at = now()` on the matching row.  Returns `true` if the row
    /// existed and had not already been used; `false` otherwise.
    async fn consume_recovery_code(
        &self,
        user_id: UserId,
        code_hash: &str,
    ) -> Result<bool, RepositoryError>;

    /// Store an encrypted TOTP secret for `user_id` without enabling TOTP.
    ///
    /// Used by `POST /v1/users/me/mfa/setup` to persist the pending secret while
    /// keeping `totp_enabled = false` until the caller confirms ownership via
    /// `/mfa/confirm`.
    async fn store_pending_totp_secret(
        &self,
        user_id: UserId,
        encrypted_secret: Vec<u8>,
    ) -> Result<(), RepositoryError>;

    /// Look up the `user_id` associated with a challenge token hash.
    ///
    /// Returns the `user_id` even after the challenge has been consumed
    /// (i.e. `used_at IS NOT NULL`), because `POST /auth/mfa/verify` needs
    /// to identify the user after it marks the challenge as used.
    async fn get_user_id_for_challenge(
        &self,
        token_hash: &str,
    ) -> Result<Option<UserId>, RepositoryError>;
}

// ---------------------------------------------------------------------------
// Postgres implementation
// ---------------------------------------------------------------------------

/// Postgres-backed MFA repository.
pub struct PgMfaRepository {
    /// Shared Postgres connection pool.
    pub pool: PgPool,
}

impl PgMfaRepository {
    /// Construct a new repository bound to `pool`.
    #[must_use]
    pub const fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Compute the SHA-256 hex hash of a raw (hex-encoded) token string.
    ///
    /// The raw token produced by [`generate_opaque_token`] is a hex-encoded
    /// representation of 32 random bytes. We hash the *bytes*, not the string,
    /// to match the convention used in `password.rs`.
    fn token_hash(raw: &str) -> String {
        hex::decode(raw).map_or_else(
            |_| hex::encode(Sha256::digest(raw.as_bytes())),
            |bytes| hex::encode(Sha256::digest(&bytes)),
        )
    }
}

#[async_trait]
impl MfaRepository for PgMfaRepository {
    async fn create_challenge(
        &self,
        user_id: UserId,
        ttl_secs: i64,
    ) -> Result<(MfaChallengeId, String), RepositoryError> {
        let (raw_token, token_hash) = generate_opaque_token();
        let expires_at = Utc::now() + Duration::seconds(ttl_secs);

        let row = sqlx::query!(
            r#"
            INSERT INTO mfa_challenges (user_id, challenge_token_hash, expires_at)
            VALUES ($1, $2, $3)
            RETURNING id
            "#,
            user_id.as_uuid(),
            token_hash,
            expires_at,
        )
        .fetch_one(&self.pool)
        .await
        .map_err(RepositoryError::Database)?;

        Ok((MfaChallengeId::from_uuid(row.id), raw_token))
    }

    async fn consume_challenge(
        &self,
        token_hash: &str,
    ) -> Result<Option<MfaChallengeId>, RepositoryError> {
        let row = sqlx::query!(
            r#"
            UPDATE mfa_challenges
            SET    used_at = now()
            WHERE  challenge_token_hash = $1
               AND expires_at > now()
               AND used_at IS NULL
            RETURNING id
            "#,
            token_hash,
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(RepositoryError::Database)?;

        Ok(row.map(|r| MfaChallengeId::from_uuid(r.id)))
    }

    async fn enable_totp(
        &self,
        user_id: UserId,
        encrypted_secret: Vec<u8>,
        recovery_code_hashes: Vec<String>,
    ) -> Result<(), RepositoryError> {
        let mut tx = self.pool.begin().await.map_err(RepositoryError::Database)?;

        // Update user row.
        sqlx::query!(
            r#"
            UPDATE users
            SET totp_secret  = $1,
                totp_enabled = true,
                updated_at   = now()
            WHERE id = $2
            "#,
            &encrypted_secret,
            user_id.as_uuid(),
        )
        .execute(&mut *tx)
        .await
        .map_err(RepositoryError::Database)?;

        // Replace all existing recovery codes.
        sqlx::query!(
            r#"DELETE FROM mfa_recovery_codes WHERE user_id = $1"#,
            user_id.as_uuid(),
        )
        .execute(&mut *tx)
        .await
        .map_err(RepositoryError::Database)?;

        for code_hash in recovery_code_hashes {
            sqlx::query!(
                r#"
                INSERT INTO mfa_recovery_codes (user_id, code_hash)
                VALUES ($1, $2)
                "#,
                user_id.as_uuid(),
                code_hash,
            )
            .execute(&mut *tx)
            .await
            .map_err(RepositoryError::Database)?;
        }

        tx.commit().await.map_err(RepositoryError::Database)?;
        Ok(())
    }

    async fn disable_totp(&self, user_id: UserId) -> Result<(), RepositoryError> {
        let mut tx = self.pool.begin().await.map_err(RepositoryError::Database)?;

        sqlx::query!(
            r#"
            UPDATE users
            SET totp_secret  = NULL,
                totp_enabled = false,
                updated_at   = now()
            WHERE id = $1
            "#,
            user_id.as_uuid(),
        )
        .execute(&mut *tx)
        .await
        .map_err(RepositoryError::Database)?;

        sqlx::query!(
            r#"DELETE FROM mfa_recovery_codes WHERE user_id = $1"#,
            user_id.as_uuid(),
        )
        .execute(&mut *tx)
        .await
        .map_err(RepositoryError::Database)?;

        tx.commit().await.map_err(RepositoryError::Database)?;
        Ok(())
    }

    async fn get_totp_secret(&self, user_id: UserId) -> Result<Option<Vec<u8>>, RepositoryError> {
        let row = sqlx::query!(
            r#"SELECT totp_secret FROM users WHERE id = $1"#,
            user_id.as_uuid(),
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(RepositoryError::Database)?;

        Ok(row.and_then(|r| r.totp_secret))
    }

    async fn consume_recovery_code(
        &self,
        user_id: UserId,
        code_hash: &str,
    ) -> Result<bool, RepositoryError> {
        let row = sqlx::query!(
            r#"
            UPDATE mfa_recovery_codes
            SET    used_at = now()
            WHERE  user_id   = $1
               AND code_hash = $2
               AND used_at IS NULL
            RETURNING id
            "#,
            user_id.as_uuid(),
            code_hash,
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(RepositoryError::Database)?;

        Ok(row.is_some())
    }

    async fn store_pending_totp_secret(
        &self,
        user_id: UserId,
        encrypted_secret: Vec<u8>,
    ) -> Result<(), RepositoryError> {
        sqlx::query!(
            r#"UPDATE users SET totp_secret = $1, updated_at = now() WHERE id = $2"#,
            &encrypted_secret,
            user_id.as_uuid(),
        )
        .execute(&self.pool)
        .await
        .map_err(RepositoryError::Database)?;
        Ok(())
    }

    async fn get_user_id_for_challenge(
        &self,
        token_hash: &str,
    ) -> Result<Option<UserId>, RepositoryError> {
        let row = sqlx::query!(
            r#"SELECT user_id FROM mfa_challenges WHERE challenge_token_hash = $1"#,
            token_hash,
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(RepositoryError::Database)?;
        Ok(row.map(|r| UserId::from_uuid(r.user_id)))
    }
}

// ---------------------------------------------------------------------------
// Helper used by mfa.rs handler tests
// ---------------------------------------------------------------------------

/// Compute `sha256_hex(raw_token)` — same logic used in [`PgMfaRepository::token_hash`].
#[must_use]
pub fn challenge_token_hash(raw: &str) -> String {
    PgMfaRepository::token_hash(raw)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::PgPool;

    async fn seed_user(pool: &PgPool) -> UserId {
        let user_id = UserId::new();
        sqlx::query!(
            "INSERT INTO users (id, email, display_name, status, created_at, updated_at) \
             VALUES ($1, $2, $3, 'active', now(), now())",
            user_id.as_uuid(),
            format!("mfa-{}@example.com", user_id.as_uuid()),
            "MFA Test User",
        )
        .execute(pool)
        .await
        .unwrap();
        user_id
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn create_challenge_returns_id_and_raw_token(pool: PgPool) {
        let user_id = seed_user(&pool).await;
        let repo = PgMfaRepository::new(pool);
        let (challenge_id, raw_token) = repo.create_challenge(user_id, 300).await.unwrap();
        assert!(!raw_token.is_empty());
        assert_ne!(challenge_id.as_uuid(), uuid::Uuid::nil());
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn consume_challenge_returns_id_for_valid_token(pool: PgPool) {
        let user_id = seed_user(&pool).await;
        let repo = PgMfaRepository::new(pool);
        let (challenge_id, raw_token) = repo.create_challenge(user_id, 300).await.unwrap();
        let token_hash = challenge_token_hash(&raw_token);
        let consumed = repo.consume_challenge(&token_hash).await.unwrap();
        assert_eq!(consumed, Some(challenge_id));
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn consume_challenge_returns_none_for_already_used_token(pool: PgPool) {
        let user_id = seed_user(&pool).await;
        let repo = PgMfaRepository::new(pool);
        let (_, raw_token) = repo.create_challenge(user_id, 300).await.unwrap();
        let token_hash = challenge_token_hash(&raw_token);
        repo.consume_challenge(&token_hash).await.unwrap();
        let second = repo.consume_challenge(&token_hash).await.unwrap();
        assert_eq!(second, None);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn consume_challenge_returns_none_for_unknown_token(pool: PgPool) {
        let repo = PgMfaRepository::new(pool);
        let result = repo.consume_challenge("deadbeef").await.unwrap();
        assert_eq!(result, None);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn get_user_id_for_challenge_returns_user(pool: PgPool) {
        let user_id = seed_user(&pool).await;
        let repo = PgMfaRepository::new(pool);
        let (_, raw_token) = repo.create_challenge(user_id, 300).await.unwrap();
        let token_hash = challenge_token_hash(&raw_token);
        let found = repo.get_user_id_for_challenge(&token_hash).await.unwrap();
        assert_eq!(found, Some(user_id));
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn get_user_id_for_challenge_returns_none_for_unknown(pool: PgPool) {
        let repo = PgMfaRepository::new(pool);
        let found = repo.get_user_id_for_challenge("unknown").await.unwrap();
        assert_eq!(found, None);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn store_pending_totp_secret_persists_bytes(pool: PgPool) {
        let user_id = seed_user(&pool).await;
        let repo = PgMfaRepository::new(pool);
        let secret = vec![1u8, 2, 3, 4];
        repo.store_pending_totp_secret(user_id, secret.clone())
            .await
            .unwrap();
        let retrieved = repo.get_totp_secret(user_id).await.unwrap();
        assert_eq!(retrieved, Some(secret));
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn get_totp_secret_returns_none_when_not_set(pool: PgPool) {
        let user_id = seed_user(&pool).await;
        let repo = PgMfaRepository::new(pool);
        let result = repo.get_totp_secret(user_id).await.unwrap();
        assert_eq!(result, None);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn enable_totp_sets_secret_and_stores_recovery_codes(pool: PgPool) {
        let user_id = seed_user(&pool).await;
        let repo = PgMfaRepository::new(pool);
        let secret = vec![0xAA, 0xBB, 0xCC];
        let codes = vec!["hash1".to_string(), "hash2".to_string()];
        repo.enable_totp(user_id, secret.clone(), codes)
            .await
            .unwrap();
        let retrieved = repo.get_totp_secret(user_id).await.unwrap();
        assert_eq!(retrieved, Some(secret));
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn disable_totp_clears_secret(pool: PgPool) {
        let user_id = seed_user(&pool).await;
        let repo = PgMfaRepository::new(pool);
        repo.enable_totp(user_id, vec![1, 2, 3], vec!["h1".to_string()])
            .await
            .unwrap();
        repo.disable_totp(user_id).await.unwrap();
        let result = repo.get_totp_secret(user_id).await.unwrap();
        assert_eq!(result, None);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn consume_recovery_code_returns_true_for_unused_code(pool: PgPool) {
        let user_id = seed_user(&pool).await;
        let repo = PgMfaRepository::new(pool);
        repo.enable_totp(user_id, vec![1], vec!["recovhash".to_string()])
            .await
            .unwrap();
        let consumed = repo
            .consume_recovery_code(user_id, "recovhash")
            .await
            .unwrap();
        assert!(consumed);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn consume_recovery_code_returns_false_when_already_used(pool: PgPool) {
        let user_id = seed_user(&pool).await;
        let repo = PgMfaRepository::new(pool);
        repo.enable_totp(user_id, vec![1], vec!["h".to_string()])
            .await
            .unwrap();
        repo.consume_recovery_code(user_id, "h").await.unwrap();
        let second = repo.consume_recovery_code(user_id, "h").await.unwrap();
        assert!(!second);
    }

    #[test]
    fn token_hash_is_deterministic() {
        let raw = "aabbccdd";
        assert_eq!(challenge_token_hash(raw), challenge_token_hash(raw));
    }

    #[test]
    fn token_hash_of_non_hex_falls_back_to_string_bytes() {
        let raw = "not-hex!";
        let hash = challenge_token_hash(raw);
        assert!(!hash.is_empty());
        assert_eq!(hash.len(), 64); // sha256 hex is 64 chars
    }
}
