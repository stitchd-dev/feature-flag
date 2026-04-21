//! Cryptographic primitives for auth.
//!
//! Provides:
//! - [`CryptoKey`]: AES-256-GCM symmetric encryption/decryption
//! - [`hash_password`] / [`verify_password`]: Argon2id password hashing
//! - [`generate_opaque_token`]: 32-byte random token with SHA-256 hash
//! - [`generate_otp`]: 6-digit OTP with Argon2id hash

use aes_gcm::{
    Aes256Gcm, Key, Nonce,
    aead::{Aead, KeyInit, OsRng as AeadOsRng},
};
use argon2::{
    Argon2,
    password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString, rand_core::OsRng},
};
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use rand::RngCore;
use sha2::{Digest, Sha256};
use thiserror::Error;

/// Errors that can arise from cryptographic operations.
#[derive(Debug, Error)]
pub enum CryptoError {
    /// The `AUTH_ENCRYPTION_KEY` environment variable is missing.
    #[error("missing AUTH_ENCRYPTION_KEY environment variable")]
    MissingEnvVar,

    /// The key bytes could not be decoded (wrong length or bad base64).
    #[error("invalid key: {0}")]
    InvalidKey(String),

    /// AES-GCM encryption failed.
    #[error("encryption failed")]
    EncryptionFailed,

    /// AES-GCM decryption failed (wrong key or corrupted ciphertext).
    #[error("decryption failed")]
    DecryptionFailed,

    /// The ciphertext is too short to contain a nonce prefix.
    #[error("ciphertext too short")]
    CiphertextTooShort,

    /// Argon2 hashing or verification failed.
    #[error("argon2 error: {0}")]
    Argon2(String),
}

/// A 256-bit symmetric key for AES-GCM encryption.
///
/// Loaded from the `AUTH_ENCRYPTION_KEY` environment variable (base64-encoded).
pub struct CryptoKey([u8; 32]);

impl CryptoKey {
    /// Read and base64-decode `AUTH_ENCRYPTION_KEY` from the environment.
    ///
    /// # Errors
    /// Returns [`CryptoError::MissingEnvVar`] if the variable is absent, or
    /// [`CryptoError::InvalidKey`] if the decoded bytes are not exactly 32 bytes.
    pub fn from_env() -> Result<Self, CryptoError> {
        let encoded = std::env::var("AUTH_ENCRYPTION_KEY")
            .map_err(|_| CryptoError::MissingEnvVar)?;
        let bytes = BASE64
            .decode(encoded.trim())
            .map_err(|e| CryptoError::InvalidKey(e.to_string()))?;
        if bytes.len() != 32 {
            return Err(CryptoError::InvalidKey(format!(
                "expected 32 bytes, got {}",
                bytes.len()
            )));
        }
        let mut key = [0u8; 32];
        key.copy_from_slice(&bytes);
        Ok(Self(key))
    }

    /// Encrypt `plaintext` with AES-256-GCM.
    ///
    /// The output is `[12-byte nonce || ciphertext+tag]`.
    ///
    /// # Errors
    /// Returns [`CryptoError::EncryptionFailed`] on AES-GCM failure.
    pub fn encrypt(&self, plaintext: &[u8]) -> Result<Vec<u8>, CryptoError> {
        let key = Key::<Aes256Gcm>::from_slice(&self.0);
        let cipher = Aes256Gcm::new(key);

        let mut nonce_bytes = [0u8; 12];
        AeadOsRng.fill_bytes(&mut nonce_bytes);
        let nonce = Nonce::from_slice(&nonce_bytes);

        let ciphertext = cipher
            .encrypt(nonce, plaintext)
            .map_err(|_| CryptoError::EncryptionFailed)?;

        let mut out = Vec::with_capacity(12 + ciphertext.len());
        out.extend_from_slice(&nonce_bytes);
        out.extend_from_slice(&ciphertext);
        Ok(out)
    }

    /// Decrypt bytes previously produced by [`encrypt`](Self::encrypt).
    ///
    /// The first 12 bytes are interpreted as the nonce.
    ///
    /// # Errors
    /// Returns [`CryptoError::CiphertextTooShort`] or [`CryptoError::DecryptionFailed`].
    pub fn decrypt(&self, ciphertext: &[u8]) -> Result<Vec<u8>, CryptoError> {
        if ciphertext.len() < 12 {
            return Err(CryptoError::CiphertextTooShort);
        }
        let (nonce_bytes, body) = ciphertext.split_at(12);
        let key = Key::<Aes256Gcm>::from_slice(&self.0);
        let cipher = Aes256Gcm::new(key);
        let nonce = Nonce::from_slice(nonce_bytes);

        cipher
            .decrypt(nonce, body)
            .map_err(|_| CryptoError::DecryptionFailed)
    }
}

/// Hash `password` with Argon2id using a random salt.
///
/// # Errors
/// Returns [`CryptoError::Argon2`] on failure.
pub fn hash_password(password: &str) -> Result<String, CryptoError> {
    let salt = SaltString::generate(&mut OsRng);
    let argon2 = Argon2::default();
    argon2
        .hash_password(password.as_bytes(), &salt)
        .map(|h| h.to_string())
        .map_err(|e| CryptoError::Argon2(e.to_string()))
}

/// Verify `password` against an Argon2id `hash` previously produced by [`hash_password`].
///
/// # Errors
/// Returns [`CryptoError::Argon2`] if the hash string is malformed.
pub fn verify_password(password: &str, hash: &str) -> Result<bool, CryptoError> {
    let parsed = PasswordHash::new(hash).map_err(|e| CryptoError::Argon2(e.to_string()))?;
    Ok(Argon2::default()
        .verify_password(password.as_bytes(), &parsed)
        .is_ok())
}

/// Generate a cryptographically-random opaque token.
///
/// Returns `(raw_hex, sha256_hex_hash)`:
/// - `raw_hex` — 64-character lowercase hex string (32 random bytes). Returned to the caller once.
/// - `sha256_hex_hash` — SHA-256 of the raw bytes, stored in the database.
#[must_use]
pub fn generate_opaque_token() -> (String, String) {
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    let raw_hex = hex::encode(bytes);
    let hash_bytes = Sha256::digest(&bytes);
    let hash_hex = hex::encode(hash_bytes);
    (raw_hex, hash_hex)
}

/// Generate a 6-digit OTP and its Argon2id hash.
///
/// Returns `(otp_string, argon2_hash)`:
/// - `otp_string` — exactly 6 decimal digits, zero-padded.
/// - `argon2_hash` — Argon2id hash suitable for storage.
///
/// # Panics
/// Panics if the Argon2 hasher fails (should never happen with valid inputs).
#[must_use]
pub fn generate_otp() -> (String, String) {
    use rand::Rng as _;
    let code: u32 = rand::thread_rng().gen_range(0..1_000_000);
    let otp = format!("{code:06}");
    let hash = hash_password(&otp).expect("argon2 OTP hashing should not fail");
    (otp, hash)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_key() -> CryptoKey {
        // 32 zero bytes — fine for testing
        CryptoKey([0u8; 32])
    }

    // -----------------------------------------------------------------------
    // AES-256-GCM round-trip
    // -----------------------------------------------------------------------

    #[test]
    fn aes_encrypt_decrypt_roundtrip() {
        let key = test_key();
        let plaintext = b"hello, world!";
        let ciphertext = key.encrypt(plaintext).unwrap();
        let decrypted = key.decrypt(&ciphertext).unwrap();
        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn aes_different_nonce_each_time() {
        let key = test_key();
        let ct1 = key.encrypt(b"same").unwrap();
        let ct2 = key.encrypt(b"same").unwrap();
        // Nonces should differ (probabilistically guaranteed)
        assert_ne!(&ct1[..12], &ct2[..12]);
    }

    #[test]
    fn aes_wrong_key_returns_error() {
        let key1 = test_key();
        let key2 = CryptoKey([1u8; 32]);
        let ciphertext = key1.encrypt(b"secret").unwrap();
        let result = key2.decrypt(&ciphertext);
        assert!(result.is_err());
    }

    #[test]
    fn aes_decrypt_too_short_returns_error() {
        let key = test_key();
        let result = key.decrypt(&[0u8; 5]);
        assert!(matches!(result, Err(CryptoError::CiphertextTooShort)));
    }

    // -----------------------------------------------------------------------
    // Argon2id password hashing
    // -----------------------------------------------------------------------

    #[test]
    fn hash_and_verify_correct_password() {
        let hash = hash_password("correct-horse-battery-staple").unwrap();
        let ok = verify_password("correct-horse-battery-staple", &hash).unwrap();
        assert!(ok);
    }

    #[test]
    fn verify_wrong_password_returns_false() {
        let hash = hash_password("my-password").unwrap();
        let ok = verify_password("wrong-password", &hash).unwrap();
        assert!(!ok);
    }

    // -----------------------------------------------------------------------
    // Opaque token
    // -----------------------------------------------------------------------

    #[test]
    fn opaque_token_raw_and_hash_differ() {
        let (raw, hash) = generate_opaque_token();
        assert!(!raw.is_empty());
        assert!(!hash.is_empty());
        assert_ne!(raw, hash);
    }

    #[test]
    fn opaque_token_raw_is_64_hex_chars() {
        let (raw, _) = generate_opaque_token();
        assert_eq!(raw.len(), 64);
        assert!(raw.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn opaque_token_hash_is_sha256_hex() {
        let (raw, hash) = generate_opaque_token();
        // Verify hash is correct SHA-256 of raw bytes
        let raw_bytes = hex::decode(&raw).unwrap();
        let expected = hex::encode(Sha256::digest(&raw_bytes));
        assert_eq!(hash, expected);
    }

    // -----------------------------------------------------------------------
    // OTP
    // -----------------------------------------------------------------------

    #[test]
    fn otp_is_exactly_6_digits() {
        let (otp, _) = generate_otp();
        assert_eq!(otp.len(), 6);
        assert!(otp.chars().all(|c| c.is_ascii_digit()));
    }

    #[test]
    fn otp_hash_verifies_correctly() {
        let (otp, hash) = generate_otp();
        let ok = verify_password(&otp, &hash).unwrap();
        assert!(ok);
    }

    #[test]
    fn otp_wrong_code_fails_verify() {
        let (otp, hash) = generate_otp();
        // Increment the OTP to get a wrong value
        let wrong: u64 = otp.parse::<u64>().unwrap().wrapping_add(1) % 1_000_000;
        let wrong_str = format!("{wrong:06}");
        if wrong_str != otp {
            let ok = verify_password(&wrong_str, &hash).unwrap();
            assert!(!ok);
        }
    }
}
