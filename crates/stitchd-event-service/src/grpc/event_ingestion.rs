//! gRPC implementation of `EventIngestionService`.
//!
//! # Protocol
//! - The SDK key is read from the `x-sdk-key` gRPC metadata header.
//! - It is hashed (SHA-256 → hex) and looked up in the `sdk_keys` table to
//!   resolve the `environment_id`.
//! - Each event's `metric_key` is validated against the pre-registered event
//!   definitions for that environment.
//! - Unknown keys (and type-mismatched keys) are rejected: returned in
//!   `IngestResponse::rejected_keys`; the rest are written to ClickHouse via
//!   the `stitchd-events` crate.

use std::sync::Arc;

use sha2::{Digest, Sha256};
use tonic::{Request, Response, Status};
use tracing::instrument;

use stitchd_core::event::EventValueType;
use stitchd_db::{EventDefinitionRepository, SdkKeyRepository};
use stitchd_events::writer::EventWriter;
use stitchd_proto::events::v1::{
    IngestRequest, IngestResponse,
    event_ingestion_service_server::EventIngestionService,
};

// ---------------------------------------------------------------------------
// Service state
// ---------------------------------------------------------------------------

/// Shared state for the `EventIngestionService`.
#[derive(Clone)]
pub struct ServiceState {
    /// Postgres repository for event definition lookups.
    pub event_def_repo: Arc<dyn EventDefinitionRepository>,
    /// Postgres repository for SDK key → `environment_id` resolution.
    pub sdk_key_repo: Arc<dyn SdkKeyRepository>,
    /// ClickHouse writer for persisting accepted events.
    pub event_writer: EventWriter,
}

// ---------------------------------------------------------------------------
// Service implementation
// ---------------------------------------------------------------------------

/// gRPC service implementation of `EventIngestionService`.
pub struct EventIngestionServiceImpl {
    state: ServiceState,
}

impl EventIngestionServiceImpl {
    /// Create a new service instance backed by `state`.
    #[must_use]
    pub const fn new(state: ServiceState) -> Self {
        Self { state }
    }

    /// Extract and validate the SDK key from gRPC metadata, returning the `environment_id`.
    async fn authenticate(
        &self,
        metadata: &tonic::metadata::MetadataMap,
    ) -> Result<stitchd_core::id::EnvironmentId, Status> {
        let raw_key = metadata
            .get("x-sdk-key")
            .and_then(|v| v.to_str().ok())
            .ok_or_else(|| Status::unauthenticated("missing x-sdk-key metadata"))?;

        let key_hash = hash_sdk_key(raw_key);

        let sdk_key = self
            .state
            .sdk_key_repo
            .find_active_by_hash(&key_hash)
            .await
            .map_err(|_| Status::unauthenticated("invalid or revoked SDK key"))?;

        Ok(sdk_key.environment_id)
    }
}

/// Hash a raw SDK key with SHA-256 → lowercase hex, matching the stored hash.
#[must_use]
pub fn hash_sdk_key(raw: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(raw.as_bytes());
    hex::encode(hasher.finalize())
}

// ---------------------------------------------------------------------------
// tonic trait implementation
// ---------------------------------------------------------------------------

#[tonic::async_trait]
impl EventIngestionService for EventIngestionServiceImpl {
    #[instrument(skip(self, request), name = "event_ingestion.ingest")]
    async fn ingest_event(
        &self,
        request: Request<IngestRequest>,
    ) -> Result<Response<IngestResponse>, Status> {
        let env_id = self.authenticate(request.metadata()).await?;

        // Load all registered event definitions for this environment.
        let definitions = self
            .state
            .event_def_repo
            .list_by_environment(env_id)
            .await
            .map_err(|e| Status::internal(e.to_string()))?;

        // Build a lookup map: metric_key → EventValueType.
        let registry: std::collections::HashMap<String, EventValueType> = definitions
            .into_iter()
            .map(|d| (d.key, d.value_type))
            .collect();

        let inner = request.into_inner();

        let mut accepted_count: u32 = 0;
        let mut rejected_keys: Vec<String> = Vec::new();
        let mut ch_rows: Vec<stitchd_events::writer::EventRow> = Vec::new();

        for event in &inner.events {
            // Resolve expected value type.
            let Some(expected_type) = registry.get(&event.metric_key) else {
                rejected_keys.push(event.metric_key.clone());
                continue;
            };

            // Validate value type matches registry.
            let value = event.value.as_ref().and_then(|v| v.value.as_ref());
            let type_ok = matches!(
                (expected_type, value),
                (EventValueType::Bool, Some(stitchd_proto::events::v1::metric_value::Value::BoolValue(_)))
                    | (EventValueType::Int, Some(stitchd_proto::events::v1::metric_value::Value::IntValue(_)))
                    | (EventValueType::Double, Some(stitchd_proto::events::v1::metric_value::Value::DoubleValue(_)))
            );

            if !type_ok {
                rejected_keys.push(event.metric_key.clone());
                continue;
            }

            // Build ClickHouse row.
            let (value_bool, value_int, value_double) = match value {
                Some(stitchd_proto::events::v1::metric_value::Value::BoolValue(b)) => (Some(*b), None, None),
                Some(stitchd_proto::events::v1::metric_value::Value::IntValue(i)) => (None, Some(*i), None),
                Some(stitchd_proto::events::v1::metric_value::Value::DoubleValue(d)) => (None, None, Some(*d)),
                _ => unreachable!("type_ok guarantees one branch"),
            };

            ch_rows.push(stitchd_events::writer::EventRow {
                env_id: env_id.as_uuid(),
                contexts: vec![(event.context_type.clone(), event.context_key.clone())],
                metric_key: event.metric_key.clone(),
                value_bool,
                value_int,
                value_double,
                timestamp: event.timestamp_ms,
            });

            accepted_count += 1;
        }

        // Write accepted events to ClickHouse (fire-and-forget).
        if !ch_rows.is_empty() {
            let writer = self.state.event_writer.clone();
            tokio::spawn(async move {
                if let Err(e) = writer.write_rows(ch_rows).await {
                    tracing::error!("ClickHouse write failed: {e}");
                }
            });
        }

        metrics::counter!("event_service.events.accepted").increment(u64::from(accepted_count));
        metrics::counter!("event_service.events.rejected")
            .increment(rejected_keys.len() as u64);

        Ok(Response::new(IngestResponse {
            accepted_count,
            rejected_keys,
        }))
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::hash_sdk_key;

    #[test]
    fn hash_sdk_key_is_deterministic() {
        let h1 = hash_sdk_key("sk_test_abc");
        let h2 = hash_sdk_key("sk_test_abc");
        assert_eq!(h1, h2);
        assert_eq!(h1.len(), 64, "SHA-256 hex is 64 chars");
    }

    #[test]
    fn hash_sdk_key_differs_for_different_keys() {
        let h1 = hash_sdk_key("sk_test_abc");
        let h2 = hash_sdk_key("sk_test_xyz");
        assert_ne!(h1, h2);
    }
}
