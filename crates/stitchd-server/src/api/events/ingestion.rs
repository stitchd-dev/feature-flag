//! Ingestion validation: resolves event definitions from PostgreSQL and checks value types.
//!
//! Validation errors surface as `IngestionError::UnknownKey` (422) or
//! `IngestionError::TypeMismatch` (422). After validation the actual ClickHouse
//! write is spawned as a fire-and-forget task.

use std::sync::Arc;
use stitchd_core::{
    event::{BatchEventPayload, EventPayload},
    id::EnvironmentId,
};
use stitchd_db::{EventDefinitionRepository, RepositoryError};

use stitchd_events::writer::EventWriter;
use tracing::instrument;

/// Errors from event ingestion validation.
#[derive(Debug, thiserror::Error)]
pub enum IngestionError {
    /// The submitted `metric_key` is not registered for this environment.
    #[error("unknown metric key: {key}")]
    UnknownKey {
        /// The unregistered key.
        key: String,
    },
    /// The submitted value type does not match the registered definition.
    #[error("type mismatch for key '{key}': expected {expected}, got {got}")]
    TypeMismatch {
        /// The key whose type did not match.
        key: String,
        /// Expected type name.
        expected: String,
        /// Submitted type name.
        got: String,
    },
    /// Internal repository error.
    #[error("repository error: {0}")]
    Repository(#[from] RepositoryError),
}

/// Validate a single event payload against registered definitions.
///
/// Returns `Ok(())` if the payload is valid. Does not write to ClickHouse.
///
/// # Errors
///
/// Returns [`IngestionError`] if the key is unknown or the value type mismatches.
#[instrument(skip(repo, payload), fields(metric_key = %payload.metric_key))]
pub async fn validate_event(
    repo: &Arc<dyn EventDefinitionRepository>,
    env_id: EnvironmentId,
    payload: &EventPayload,
) -> Result<(), IngestionError> {
    let def = repo
        .find_by_key(&payload.metric_key, env_id)
        .await
        .map_err(|e| match e {
            RepositoryError::NotFound { .. } => IngestionError::UnknownKey {
                key: payload.metric_key.clone(),
            },
            other => IngestionError::Repository(other),
        })?;

    let submitted = payload.value.value_type();
    if submitted != def.value_type {
        return Err(IngestionError::TypeMismatch {
            key: payload.metric_key.clone(),
            expected: format!("{:?}", def.value_type),
            got: format!("{submitted:?}"),
        });
    }

    Ok(())
}

/// Validate and fire-and-forget write a single event.
///
/// Validation is synchronous (PostgreSQL lookup); the ClickHouse write is
/// spawned and the future resolves as soon as validation passes.
/// If `writer` is `None` the write is skipped (useful in test environments).
///
/// # Errors
///
/// Returns [`IngestionError`] if validation fails.
pub async fn ingest_event(
    repo: &Arc<dyn EventDefinitionRepository>,
    writer: Option<&EventWriter>,
    env_id: EnvironmentId,
    payload: EventPayload,
) -> Result<(), IngestionError> {
    validate_event(repo, env_id, &payload).await?;

    if let Some(w) = writer {
        let env_uuid = env_id.as_uuid();
        let w = w.clone();
        tokio::spawn(async move {
            if let Err(e) = w.write(env_uuid, &payload).await {
                tracing::error!("clickhouse write failed: {e}");
            }
        });
    }

    Ok(())
}

/// Validate and fire-and-forget write a batch of events.
///
/// All events in the batch are validated before any ClickHouse write is initiated.
/// A single unknown key or type mismatch causes the entire batch to be rejected (422).
/// If `writer` is `None` the write is skipped (useful in test environments).
///
/// # Errors
///
/// Returns [`IngestionError`] if any event in the batch fails validation.
pub async fn ingest_batch(
    repo: &Arc<dyn EventDefinitionRepository>,
    writer: Option<&EventWriter>,
    env_id: EnvironmentId,
    batch: BatchEventPayload,
) -> Result<(), IngestionError> {
    for payload in &batch.events {
        validate_event(repo, env_id, payload).await?;
    }

    if let Some(w) = writer {
        let env_uuid = env_id.as_uuid();
        let w = w.clone();
        tokio::spawn(async move {
            if let Err(e) = w.write_batch(env_uuid, &batch).await {
                tracing::error!("clickhouse batch write failed: {e}");
            }
        });
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use chrono::Utc;
    use clickhouse::Client;
    use stitchd_core::{
        event::{EventContext, EventDefinition, EventPayload, EventValue, EventValueType},
        id::{EnvironmentId, EventDefinitionId},
    };
    use stitchd_events::writer::EventWriter;

    fn make_def(env_id: EnvironmentId, key: &str, vt: EventValueType) -> EventDefinition {
        EventDefinition {
            id: EventDefinitionId::new(),
            environment_id: env_id,
            key: key.to_string(),
            value_type: vt,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            deleted_at: None,
            version: 1,
        }
    }

    fn make_payload(key: &str, value: EventValue) -> EventPayload {
        EventPayload {
            contexts: vec![EventContext {
                context_type: "user".into(),
                key: "u1".into(),
            }],
            metric_key: key.to_string(),
            value,
            timestamp: Utc::now(),
        }
    }

    struct OkRepo(EventDefinition);

    #[async_trait]
    impl EventDefinitionRepository for OkRepo {
        async fn find_by_id(
            &self,
            _id: EventDefinitionId,
        ) -> Result<EventDefinition, RepositoryError> {
            unimplemented!()
        }
        async fn find_by_key(
            &self,
            _key: &str,
            _env_id: EnvironmentId,
        ) -> Result<EventDefinition, RepositoryError> {
            Ok(self.0.clone())
        }
        async fn list_by_environment(
            &self,
            _env_id: EnvironmentId,
        ) -> Result<Vec<EventDefinition>, RepositoryError> {
            unimplemented!()
        }
        async fn create(&self, _def: &EventDefinition) -> Result<(), RepositoryError> {
            unimplemented!()
        }
        async fn update(&self, _def: &EventDefinition) -> Result<EventDefinition, RepositoryError> {
            unimplemented!()
        }
        async fn soft_delete(&self, _id: EventDefinitionId) -> Result<(), RepositoryError> {
            unimplemented!()
        }
    }

    struct ErrRepo;

    #[async_trait]
    impl EventDefinitionRepository for ErrRepo {
        async fn find_by_id(
            &self,
            _id: EventDefinitionId,
        ) -> Result<EventDefinition, RepositoryError> {
            unimplemented!()
        }
        async fn find_by_key(
            &self,
            _key: &str,
            _env_id: EnvironmentId,
        ) -> Result<EventDefinition, RepositoryError> {
            Err(RepositoryError::VersionConflict {
                expected: 1,
                actual: 2,
            })
        }
        async fn list_by_environment(
            &self,
            _env_id: EnvironmentId,
        ) -> Result<Vec<EventDefinition>, RepositoryError> {
            unimplemented!()
        }
        async fn create(&self, _def: &EventDefinition) -> Result<(), RepositoryError> {
            unimplemented!()
        }
        async fn update(&self, _def: &EventDefinition) -> Result<EventDefinition, RepositoryError> {
            unimplemented!()
        }
        async fn soft_delete(&self, _id: EventDefinitionId) -> Result<(), RepositoryError> {
            unimplemented!()
        }
    }

    fn make_writer() -> EventWriter {
        // Non-existent port → writes fail fast (connection refused), exercising the error path
        EventWriter::new(
            Client::default()
                .with_url("http://localhost:19999")
                .with_user("stitchd")
                .with_password("stitchd")
                .with_database("stitchd"),
        )
    }

    #[tokio::test]
    async fn non_not_found_repo_error_maps_to_repository_variant() {
        let repo: Arc<dyn EventDefinitionRepository> = Arc::new(ErrRepo);
        let env_id = EnvironmentId::new();
        let payload = make_payload("key", EventValue::Bool(true));

        let err = validate_event(&repo, env_id, &payload).await.unwrap_err();
        assert!(matches!(err, IngestionError::Repository(_)));
    }

    #[tokio::test]
    async fn ingest_event_with_writer_returns_ok_and_spawns_write() {
        let env_id = EnvironmentId::new();
        let def = make_def(env_id, "m", EventValueType::Bool);
        let repo: Arc<dyn EventDefinitionRepository> = Arc::new(OkRepo(def));
        let writer = make_writer();

        let result = ingest_event(
            &repo,
            Some(&writer),
            env_id,
            make_payload("m", EventValue::Bool(true)),
        )
        .await;
        assert!(result.is_ok());
        // Yield so the spawned write task runs (connection refused → logs error)
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }

    #[tokio::test]
    async fn ingest_batch_with_writer_returns_ok_and_spawns_write() {
        let env_id = EnvironmentId::new();
        let def = make_def(env_id, "m", EventValueType::Int);
        let repo: Arc<dyn EventDefinitionRepository> = Arc::new(OkRepo(def));
        let writer = make_writer();
        let batch = BatchEventPayload {
            events: vec![
                make_payload("m", EventValue::Int(1)),
                make_payload("m", EventValue::Int(2)),
            ],
        };

        let result = ingest_batch(&repo, Some(&writer), env_id, batch).await;
        assert!(result.is_ok());
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }
}
