//! Event Definition Registry — CRUD operations for pre-registered event definitions.
//!
//! This module owns the `events` schema in PostgreSQL. It wraps the
//! [`EventDefinitionRepository`] trait and provides a service-level API used
//! by:
//! - The gRPC ingestion handler ([`crate::grpc::event_ingestion`]) for key
//!   validation at ingest time.
//! - Future management gRPC/HTTP handlers for CRUD.
//!
//! # Migration note
//! The logic here is migrated from `stitchd-server/src/api/event_definitions/`.

use std::sync::Arc;

use chrono::Utc;

use stitchd_core::{
    event::{EventDefinition, EventValueType},
    id::{EnvironmentId, EventDefinitionId},
};
use stitchd_db::{EventDefinitionRepository, RepositoryError};

// ---------------------------------------------------------------------------
// Registry
// ---------------------------------------------------------------------------

/// Service-level wrapper around the [`EventDefinitionRepository`].
///
/// Callers interact with this struct rather than the trait object directly,
/// so business rules (e.g. generating IDs, stamping timestamps) live in one
/// place.
#[derive(Clone)]
pub struct EventDefinitionRegistry {
    repo: Arc<dyn EventDefinitionRepository>,
}

impl EventDefinitionRegistry {
    /// Create a new registry backed by `repo`.
    #[must_use]
    pub fn new(repo: Arc<dyn EventDefinitionRepository>) -> Self {
        Self { repo }
    }

    /// Register a new event definition in the given environment.
    ///
    /// # Errors
    /// - [`RegistryError::AlreadyExists`] if a definition with the same `key`
    ///   already exists in the environment.
    /// - [`RegistryError::Repository`] for any other persistence failure.
    pub async fn create(
        &self,
        environment_id: EnvironmentId,
        key: String,
        value_type: EventValueType,
    ) -> Result<EventDefinition, RegistryError> {
        let now = Utc::now();
        let def = EventDefinition {
            id: EventDefinitionId::new(),
            environment_id,
            key,
            value_type,
            created_at: now,
            updated_at: now,
            deleted_at: None,
            version: 1,
        };

        self.repo.create(&def).await.map_err(|e| match e {
            RepositoryError::UniqueViolation { field } => RegistryError::AlreadyExists(field),
            other => RegistryError::Repository(other),
        })?;

        Ok(def)
    }

    /// List all active (non-deleted) definitions for an environment.
    ///
    /// # Errors
    /// Returns [`RegistryError::Repository`] on database failure.
    pub async fn list(
        &self,
        environment_id: EnvironmentId,
    ) -> Result<Vec<EventDefinition>, RegistryError> {
        self.repo
            .list_by_environment(environment_id)
            .await
            .map_err(RegistryError::Repository)
    }

    /// Soft-delete a definition by its string key within an environment.
    ///
    /// # Errors
    /// - [`RegistryError::NotFound`] if no active definition with that key exists.
    /// - [`RegistryError::Repository`] for any other persistence failure.
    pub async fn delete_by_key(
        &self,
        environment_id: EnvironmentId,
        key: &str,
    ) -> Result<(), RegistryError> {
        let def = self
            .repo
            .find_by_key(key, environment_id)
            .await
            .map_err(|e| match e {
                RepositoryError::NotFound { .. } => RegistryError::NotFound(key.to_string()),
                other => RegistryError::Repository(other),
            })?;

        self.repo
            .soft_delete(def.id)
            .await
            .map_err(RegistryError::Repository)
    }

    /// Fetch a single definition by its unique ID.
    ///
    /// # Errors
    /// - [`RegistryError::NotFound`] if the ID does not exist.
    /// - [`RegistryError::Repository`] for any other failure.
    pub async fn find_by_id(
        &self,
        id: EventDefinitionId,
    ) -> Result<EventDefinition, RegistryError> {
        self.repo.find_by_id(id).await.map_err(|e| match e {
            RepositoryError::NotFound { .. } => RegistryError::NotFound(id.as_uuid().to_string()),
            other => RegistryError::Repository(other),
        })
    }
}

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

/// Errors that can arise from registry operations.
#[derive(Debug, thiserror::Error)]
pub enum RegistryError {
    /// A definition with this key already exists in the environment.
    #[error("event definition already exists: {0}")]
    AlreadyExists(String),

    /// The requested definition does not exist (or has been soft-deleted).
    #[error("event definition not found: {0}")]
    NotFound(String),

    /// An underlying repository/database error.
    #[error("repository error: {0}")]
    Repository(#[from] RepositoryError),
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    use async_trait::async_trait;
    use chrono::Utc;

    use stitchd_core::{
        event::{EventDefinition, EventValueType},
        id::{EnvironmentId, EventDefinitionId},
    };
    use stitchd_db::{EventDefinitionRepository, RepositoryError};

    use super::{EventDefinitionRegistry, RegistryError};

    // -----------------------------------------------------------------------
    // In-memory mock
    // -----------------------------------------------------------------------

    struct MemEventDefRepo {
        store: Mutex<HashMap<String, EventDefinition>>,
        /// When `Some`, `create` returns this error.
        create_err: Option<RepositoryError>,
    }

    impl MemEventDefRepo {
        fn new() -> Self {
            Self {
                store: Mutex::new(HashMap::new()),
                create_err: None,
            }
        }

        fn with_create_err(err: RepositoryError) -> Self {
            Self {
                store: Mutex::new(HashMap::new()),
                create_err: Some(err),
            }
        }
    }

    #[async_trait]
    impl EventDefinitionRepository for MemEventDefRepo {
        async fn find_by_id(
            &self,
            id: EventDefinitionId,
        ) -> Result<EventDefinition, RepositoryError> {
            let store = self.store.lock().unwrap();
            store
                .get(&id.as_uuid().to_string())
                .cloned()
                .ok_or_else(|| RepositoryError::NotFound {
                    id: id.as_uuid().to_string(),
                })
        }

        async fn find_by_key(
            &self,
            key: &str,
            _environment_id: EnvironmentId,
        ) -> Result<EventDefinition, RepositoryError> {
            let store = self.store.lock().unwrap();
            store
                .values()
                .find(|d| d.key == key && d.deleted_at.is_none())
                .cloned()
                .ok_or_else(|| RepositoryError::NotFound {
                    id: key.to_string(),
                })
        }

        async fn list_by_environment(
            &self,
            environment_id: EnvironmentId,
        ) -> Result<Vec<EventDefinition>, RepositoryError> {
            let store = self.store.lock().unwrap();
            Ok(store
                .values()
                .filter(|d| d.environment_id == environment_id && d.deleted_at.is_none())
                .cloned()
                .collect())
        }

        async fn create(&self, def: &EventDefinition) -> Result<(), RepositoryError> {
            if let Some(ref err) = self.create_err {
                return Err(match err {
                    RepositoryError::UniqueViolation { field } => {
                        RepositoryError::UniqueViolation {
                            field: field.clone(),
                        }
                    }
                    RepositoryError::NotFound { id } => {
                        RepositoryError::NotFound { id: id.clone() }
                    }
                    _ => RepositoryError::NotFound {
                        id: "injected".into(),
                    },
                });
            }
            {
                let mut store = self.store.lock().unwrap();
                store.insert(def.id.as_uuid().to_string(), def.clone());
            }
            Ok(())
        }

        async fn update(&self, def: &EventDefinition) -> Result<EventDefinition, RepositoryError> {
            {
                let mut store = self.store.lock().unwrap();
                store.insert(def.id.as_uuid().to_string(), def.clone());
            }
            Ok(def.clone())
        }

        async fn soft_delete(&self, id: EventDefinitionId) -> Result<(), RepositoryError> {
            let mut store = self.store.lock().unwrap();
            if let Some(def) = store.get_mut(&id.as_uuid().to_string()) {
                def.deleted_at = Some(Utc::now());
                Ok(())
            } else {
                Err(RepositoryError::NotFound {
                    id: id.as_uuid().to_string(),
                })
            }
        }
    }

    // -----------------------------------------------------------------------
    // Tests
    // -----------------------------------------------------------------------

    fn make_registry(repo: MemEventDefRepo) -> EventDefinitionRegistry {
        EventDefinitionRegistry::new(Arc::new(repo))
    }

    #[tokio::test]
    async fn create_stores_definition_with_generated_id_and_timestamps() {
        let registry = make_registry(MemEventDefRepo::new());
        let env_id = EnvironmentId::new();

        let def = registry
            .create(env_id, "click_count".to_string(), EventValueType::Int)
            .await
            .expect("create should succeed");

        assert_eq!(def.key, "click_count");
        assert_eq!(def.value_type, EventValueType::Int);
        assert_eq!(def.environment_id, env_id);
        assert_eq!(def.version, 1);
        assert!(def.deleted_at.is_none());
    }

    #[tokio::test]
    async fn create_returns_already_exists_on_unique_violation() {
        let repo = MemEventDefRepo::with_create_err(RepositoryError::UniqueViolation {
            field: "key".into(),
        });
        let registry = make_registry(repo);
        let env_id = EnvironmentId::new();

        let err = registry
            .create(env_id, "click_count".to_string(), EventValueType::Int)
            .await
            .expect_err("should fail with AlreadyExists");

        assert!(matches!(err, RegistryError::AlreadyExists(_)));
    }

    #[tokio::test]
    async fn list_returns_all_active_definitions() {
        let registry = make_registry(MemEventDefRepo::new());
        let env_id = EnvironmentId::new();

        registry
            .create(env_id, "a".to_string(), EventValueType::Bool)
            .await
            .unwrap();
        registry
            .create(env_id, "b".to_string(), EventValueType::Int)
            .await
            .unwrap();
        registry
            .create(env_id, "c".to_string(), EventValueType::Double)
            .await
            .unwrap();

        let defs = registry.list(env_id).await.expect("list should succeed");
        assert_eq!(defs.len(), 3);
    }

    #[tokio::test]
    async fn list_returns_empty_for_unknown_environment() {
        let registry = make_registry(MemEventDefRepo::new());
        let env_id = EnvironmentId::new();

        let defs = registry.list(env_id).await.expect("list should succeed");
        assert!(defs.is_empty());
    }

    #[tokio::test]
    async fn delete_by_key_removes_definition() {
        let registry = make_registry(MemEventDefRepo::new());
        let env_id = EnvironmentId::new();

        registry
            .create(env_id, "revenue".to_string(), EventValueType::Double)
            .await
            .unwrap();

        registry
            .delete_by_key(env_id, "revenue")
            .await
            .expect("delete should succeed");

        // After soft-delete, list should return empty.
        let defs = registry.list(env_id).await.unwrap();
        assert!(defs.is_empty());
    }

    #[tokio::test]
    async fn delete_by_key_returns_not_found_for_missing_key() {
        let registry = make_registry(MemEventDefRepo::new());
        let env_id = EnvironmentId::new();

        let err = registry
            .delete_by_key(env_id, "nonexistent")
            .await
            .expect_err("should fail with NotFound");

        assert!(matches!(err, RegistryError::NotFound(_)));
    }

    #[tokio::test]
    async fn find_by_id_returns_definition() {
        let registry = make_registry(MemEventDefRepo::new());
        let env_id = EnvironmentId::new();

        let created = registry
            .create(env_id, "converted".to_string(), EventValueType::Bool)
            .await
            .unwrap();

        let fetched = registry
            .find_by_id(created.id)
            .await
            .expect("find should succeed");

        assert_eq!(fetched.id, created.id);
        assert_eq!(fetched.key, "converted");
    }

    #[tokio::test]
    async fn find_by_id_returns_not_found_for_unknown_id() {
        let registry = make_registry(MemEventDefRepo::new());

        let err = registry
            .find_by_id(EventDefinitionId::new())
            .await
            .expect_err("should fail with NotFound");

        assert!(matches!(err, RegistryError::NotFound(_)));
    }

    #[tokio::test]
    async fn registry_error_display() {
        assert!(
            RegistryError::AlreadyExists("key".into())
                .to_string()
                .contains("already exists")
        );
        assert!(
            RegistryError::NotFound("key".into())
                .to_string()
                .contains("not found")
        );
    }
}
