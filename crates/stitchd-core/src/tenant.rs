//! Multi-tenancy types: `Organisation`, `Project`, `Environment`, `SdkKey`.
//!
//! Models the tenancy hierarchy: Organisation → Projects → Environments.
//! SDK keys are scoped to an environment. At least one active key must exist
//! per environment at all times (`SdkKey::has_active_key`).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::id::{EnvironmentId, OrganisationId, ProjectId, SdkKeyId};

/// A top-level tenant in the platform.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Organisation {
    /// Unique identifier.
    pub id: OrganisationId,
    /// Display name.
    pub name: String,
    /// When this record was created.
    pub created_at: DateTime<Utc>,
    /// When this record was last modified.
    pub updated_at: DateTime<Utc>,
    /// Set when the organisation is soft-deleted; `None` while active.
    pub deleted_at: Option<DateTime<Utc>>,
    /// Optimistic-concurrency version counter.
    pub version: i64,
}

/// A project within an [`Organisation`].
///
/// Feature flag and variant *definitions* are project-scoped.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Project {
    /// Unique identifier.
    pub id: ProjectId,
    /// Parent organisation.
    pub organisation_id: OrganisationId,
    /// Display name.
    pub name: String,
    /// When this record was created.
    pub created_at: DateTime<Utc>,
    /// When this record was last modified.
    pub updated_at: DateTime<Utc>,
    /// Set when the project is soft-deleted; `None` while active.
    pub deleted_at: Option<DateTime<Utc>>,
    /// Optimistic-concurrency version counter.
    pub version: i64,
}

/// A deployment environment within a [`Project`].
///
/// Rules, segments, experiments, and events are environment-scoped.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Environment {
    /// Unique identifier.
    pub id: EnvironmentId,
    /// Parent project.
    pub project_id: ProjectId,
    /// Display name (e.g. `"production"`, `"staging"`).
    pub name: String,
    /// When this record was created.
    pub created_at: DateTime<Utc>,
    /// When this record was last modified.
    pub updated_at: DateTime<Utc>,
    /// Set when the environment is soft-deleted; `None` while active.
    pub deleted_at: Option<DateTime<Utc>>,
    /// Optimistic-concurrency version counter.
    pub version: i64,
}

/// An SDK authentication key scoped to an [`Environment`].
///
/// The raw key is **never stored**; only the hash is persisted.
/// Every environment must retain at least one active key at all times —
/// enforced by `SdkKey::has_active_key` and the repository revoke logic.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SdkKey {
    /// Unique identifier.
    pub id: SdkKeyId,
    /// The environment this key authenticates.
    pub environment_id: EnvironmentId,
    /// Bcrypt/Argon2 hash of the raw key. The raw key is never stored.
    pub key_hash: String,
    /// Whether the key is currently usable for authentication.
    pub is_active: bool,
    /// When this key was created.
    pub created_at: DateTime<Utc>,
    /// Set when the key is revoked; `None` while active.
    pub revoked_at: Option<DateTime<Utc>>,
}

impl SdkKey {
    /// Returns `true` if at least one key in `keys` is active.
    ///
    /// Used to enforce the invariant that an environment always has at least
    /// one usable SDK key.
    ///
    /// # Examples
    ///
    /// ```
    /// use stitchd_core::tenant::SdkKey;
    ///
    /// assert!(!SdkKey::has_active_key(&[]));
    /// ```
    #[must_use]
    pub fn has_active_key(keys: &[Self]) -> bool {
        keys.iter().any(|k| k.is_active)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_key(is_active: bool) -> SdkKey {
        SdkKey {
            id: SdkKeyId::new(),
            environment_id: EnvironmentId::new(),
            key_hash: "hash".to_string(),
            is_active,
            created_at: Utc::now(),
            revoked_at: None,
        }
    }

    #[test]
    fn has_active_key_returns_false_for_empty_slice() {
        assert!(!SdkKey::has_active_key(&[]));
    }

    #[test]
    fn has_active_key_returns_false_when_all_revoked() {
        let keys = vec![make_key(false), make_key(false)];
        assert!(!SdkKey::has_active_key(&keys));
    }

    #[test]
    fn has_active_key_returns_true_with_one_active() {
        let keys = vec![make_key(true)];
        assert!(SdkKey::has_active_key(&keys));
    }

    #[test]
    fn has_active_key_returns_true_in_mixed_slice() {
        let keys = vec![make_key(false), make_key(true), make_key(false)];
        assert!(SdkKey::has_active_key(&keys));
    }

    #[test]
    fn has_active_key_returns_true_when_all_active() {
        let keys = vec![make_key(true), make_key(true)];
        assert!(SdkKey::has_active_key(&keys));
    }

    #[test]
    fn organisation_serializes_and_deserializes() {
        let org = Organisation {
            id: OrganisationId::new(),
            name: "Acme Corp".to_string(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            deleted_at: None,
            version: 1,
        };
        let json = serde_json::to_string(&org).unwrap();
        let back: Organisation = serde_json::from_str(&json).unwrap();
        assert_eq!(back.name, "Acme Corp");
        assert_eq!(back.version, 1);
        assert!(back.deleted_at.is_none());
    }
}
