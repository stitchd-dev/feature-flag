//! Ingestion validation: resolves event definitions from PostgreSQL and checks value types.
//!
//! Validation errors surface as `IngestionError::UnknownKey` (422) or
//! `IngestionError::TypeMismatch` (422). After validation the actual ClickHouse
//! write is spawned as a fire-and-forget task.

use stitchd_core::{
    event::{BatchEventPayload, EventPayload},
    id::EnvironmentId,
};
use stitchd_db::{EventDefinitionRepository, RepositoryError};
use std::sync::Arc;

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
            got: format!("{:?}", submitted),
        });
    }

    Ok(())
}

/// Validate and fire-and-forget write a single event.
///
/// Validation is synchronous (PostgreSQL lookup); the ClickHouse write is
/// spawned and the future resolves as soon as validation passes.
/// If `writer` is `None` the write is skipped (useful in test environments).
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
