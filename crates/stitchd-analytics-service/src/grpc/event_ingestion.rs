//! Event ingestion logic — handles event ingestion for the analytics service.
//!
//! # Protocol
//! - The SDK key is read from the `x-sdk-key` gRPC metadata header.
//! - It is hashed (SHA-256 → hex) and looked up in the `sdk_keys` table to
//!   resolve the `environment_id`.
//! - Each event's `metric_key` is validated against the pre-registered event
//!   definitions for that environment.
//! - Unknown keys (and type-mismatched keys) are rejected; the rest are written
//!   to ClickHouse via the `stitchd-event-writer` crate.

use std::sync::Arc;

use sha2::{Digest, Sha256};
use tonic::{Request, Response, Status};
use tracing::instrument;

use stitchd_core::event::EventValueType;
use stitchd_db::{EventDefinitionRepository, SdkKeyRepository};
use stitchd_event_writer::writer::EventWriter;
use stitchd_proto::analytics::v1::{IngestEventRequest, IngestEventResponse};

/// State needed exclusively for event ingestion.
pub struct EventIngestionState {
    pub event_def_repo: Arc<dyn EventDefinitionRepository>,
    pub sdk_key_repo: Arc<dyn SdkKeyRepository>,
    pub event_writer: EventWriter,
}

/// Hash a raw SDK key with SHA-256 → lowercase hex, matching the stored hash.
#[must_use]
pub fn hash_sdk_key(raw: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(raw.as_bytes());
    hex::encode(hasher.finalize())
}

/// Resolve credentials from gRPC metadata to an environment_id.
///
/// Accepts either:
/// - `x-sdk-key`: validated against DB (SDK clients)
/// - `x-env-id`: trusted gateway bypass for JWT-authenticated management calls
pub async fn authenticate(
    state: &EventIngestionState,
    metadata: &tonic::metadata::MetadataMap,
) -> Result<stitchd_core::id::EnvironmentId, Status> {
    // Prefer SDK key when present (validates ownership)
    if let Some(raw_key) = metadata.get("x-sdk-key").and_then(|v| v.to_str().ok()) {
        let key_hash = hash_sdk_key(raw_key);
        let sdk_key = state
            .sdk_key_repo
            .find_active_by_hash(&key_hash)
            .await
            .map_err(|_| Status::unauthenticated("invalid or revoked SDK key"))?;
        return Ok(sdk_key.environment_id);
    }

    // Trusted gateway bypass: JWT-authed management calls forward x-env-id
    if let Some(env_id_str) = metadata.get("x-env-id").and_then(|v| v.to_str().ok()) {
        let uuid = env_id_str
            .parse::<::uuid::Uuid>()
            .map_err(|_| Status::unauthenticated("x-env-id is not a valid UUID"))?;
        return Ok(stitchd_core::id::EnvironmentId::from_uuid(uuid));
    }

    Err(Status::unauthenticated(
        "missing x-sdk-key or x-env-id metadata",
    ))
}

/// Handle an IngestEvent RPC — validates, accepts, and writes to ClickHouse.
#[instrument(skip(state, request), name = "analytics.ingest_event")]
pub async fn handle_ingest_event(
    state: &EventIngestionState,
    request: Request<IngestEventRequest>,
) -> Result<Response<IngestEventResponse>, Status> {
    let env_id = authenticate(state, request.metadata()).await?;

    let definitions = state
        .event_def_repo
        .list_by_environment(env_id)
        .await
        .map_err(|e| Status::internal(e.to_string()))?;

    let registry: std::collections::HashMap<String, EventValueType> = definitions
        .into_iter()
        .map(|d| (d.key, d.value_type))
        .collect();

    let inner = request.into_inner();

    let mut accepted_count: u32 = 0;
    let mut rejected_keys: Vec<String> = Vec::new();
    let mut ch_rows: Vec<stitchd_event_writer::writer::EventRow> = Vec::new();

    for event in &inner.events {
        let Some(expected_type) = registry.get(&event.metric_key) else {
            rejected_keys.push(event.metric_key.clone());
            continue;
        };

        let value = event.value.as_ref().and_then(|v| v.value.as_ref());
        let type_ok = matches!(
            (expected_type, value),
            (
                EventValueType::Bool,
                Some(stitchd_proto::analytics::v1::metric_value::Value::BoolValue(_))
            ) | (
                EventValueType::Int,
                Some(stitchd_proto::analytics::v1::metric_value::Value::IntValue(
                    _
                ))
            ) | (
                EventValueType::Double,
                Some(stitchd_proto::analytics::v1::metric_value::Value::DoubleValue(_))
            )
        );

        if !type_ok {
            rejected_keys.push(event.metric_key.clone());
            continue;
        }

        let (value_bool, value_int, value_double) = match value {
            Some(stitchd_proto::analytics::v1::metric_value::Value::BoolValue(b)) => {
                (Some(*b), None, None)
            }
            Some(stitchd_proto::analytics::v1::metric_value::Value::IntValue(i)) => {
                (None, Some(*i), None)
            }
            Some(stitchd_proto::analytics::v1::metric_value::Value::DoubleValue(d)) => {
                (None, None, Some(*d))
            }
            _ => unreachable!("type_ok guarantees one branch"),
        };

        ch_rows.push(stitchd_event_writer::writer::EventRow {
            env_id: env_id.as_uuid(),
            contexts: vec![(event.context_type.clone(), event.context_key.clone())],
            metric_key: event.metric_key.clone(),
            value_bool,
            value_int,
            value_double,
            timestamp: event.timestamp_ms,
            // The legacy `MetricEvent` wire type carries neither a properties
            // map nor a distinct client wall-clock; default to empty metadata
            // and reuse the event timestamp as `occurred_at`.
            properties: Vec::new(),
            occurred_at: event.timestamp_ms,
        });

        accepted_count += 1;
    }

    if !ch_rows.is_empty() {
        let writer = state.event_writer.clone();
        tokio::spawn(async move {
            if let Err(e) = writer.write_rows(ch_rows).await {
                tracing::error!("ClickHouse write failed: {e}");
            }
        });
    }

    metrics::counter!("analytics_service.events.accepted").increment(u64::from(accepted_count));
    metrics::counter!("analytics_service.events.rejected").increment(rejected_keys.len() as u64);

    Ok(Response::new(IngestEventResponse {
        accepted_count,
        rejected_keys,
    }))
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
    use tonic::metadata::MetadataValue;

    use stitchd_core::{
        event::{EventDefinition, EventValueType},
        id::{EnvironmentId, EventDefinitionId, SdkKeyId},
        tenant::SdkKey,
    };
    use stitchd_db::{EventDefinitionRepository, RepositoryError, SdkKeyRepository};
    use stitchd_proto::analytics::v1::{
        IngestEventRequest, MetricEvent, MetricValue, metric_value::Value,
    };

    use super::*;

    // -----------------------------------------------------------------------
    // Mock: SdkKeyRepository
    // -----------------------------------------------------------------------

    struct MockSdkKeyRepo {
        keys: HashMap<String, SdkKey>,
    }

    impl MockSdkKeyRepo {
        fn new_with_key(key_hash: String, env_id: EnvironmentId) -> Self {
            let mut keys = HashMap::new();
            let now = Utc::now();
            keys.insert(
                key_hash.clone(),
                SdkKey {
                    id: SdkKeyId::new(),
                    environment_id: env_id,
                    key_hash,
                    name: String::new(),
                    created_at: now,
                    revoked_at: None,
                    is_active: true,
                },
            );
            Self { keys }
        }
    }

    #[async_trait]
    impl SdkKeyRepository for MockSdkKeyRepo {
        async fn find_by_id(&self, _id: SdkKeyId) -> Result<SdkKey, RepositoryError> {
            Err(RepositoryError::NotFound { id: "mock".into() })
        }

        async fn list_by_environment(
            &self,
            _environment_id: EnvironmentId,
        ) -> Result<Vec<SdkKey>, RepositoryError> {
            Ok(vec![])
        }

        async fn create(&self, _key: &SdkKey) -> Result<(), RepositoryError> {
            Ok(())
        }

        async fn revoke(&self, _id: SdkKeyId) -> Result<(), RepositoryError> {
            Ok(())
        }

        async fn find_active_by_environment(
            &self,
            _environment_id: EnvironmentId,
        ) -> Result<Vec<SdkKey>, RepositoryError> {
            Ok(vec![])
        }

        async fn list_by_environment_paginated(
            &self,
            _environment_id: EnvironmentId,
            _offset: u64,
            _limit: u64,
        ) -> Result<(Vec<SdkKey>, u64), RepositoryError> {
            Ok((vec![], 0))
        }

        async fn find_active_by_hash(&self, key_hash: &str) -> Result<SdkKey, RepositoryError> {
            self.keys
                .get(key_hash)
                .cloned()
                .ok_or_else(|| RepositoryError::NotFound {
                    id: key_hash.to_string(),
                })
        }
    }

    // -----------------------------------------------------------------------
    // Mock: EventDefinitionRepository
    // -----------------------------------------------------------------------

    struct MockEventDefRepo {
        defs: Mutex<HashMap<String, Vec<EventDefinition>>>,
    }

    impl MockEventDefRepo {
        fn new(env_id: EnvironmentId, defs: Vec<(String, EventValueType)>) -> Self {
            let now = Utc::now();
            let mut map = HashMap::new();
            let event_defs: Vec<EventDefinition> = defs
                .into_iter()
                .map(|(key, value_type)| EventDefinition {
                    id: EventDefinitionId::new(),
                    environment_id: env_id,
                    name: key.clone(),
                    key,
                    description: None,
                    value_type,
                    metric_type: stitchd_core::event::MetricType::Count,
                    schema: None,
                    created_at: now,
                    updated_at: now,
                    deleted_at: None,
                    version: 1,
                })
                .collect();
            map.insert(env_id.as_uuid().to_string(), event_defs);
            Self {
                defs: Mutex::new(map),
            }
        }
    }

    #[async_trait]
    impl EventDefinitionRepository for MockEventDefRepo {
        async fn find_by_id(
            &self,
            _id: EventDefinitionId,
        ) -> Result<EventDefinition, RepositoryError> {
            Err(RepositoryError::NotFound { id: "mock".into() })
        }

        async fn find_by_key(
            &self,
            key: &str,
            environment_id: EnvironmentId,
        ) -> Result<EventDefinition, RepositoryError> {
            let guard = self.defs.lock().unwrap();
            guard
                .get(&environment_id.as_uuid().to_string())
                .and_then(|v| v.iter().find(|d| d.key == key))
                .cloned()
                .ok_or_else(|| RepositoryError::NotFound {
                    id: key.to_string(),
                })
        }

        async fn list_by_environment(
            &self,
            environment_id: EnvironmentId,
        ) -> Result<Vec<EventDefinition>, RepositoryError> {
            let guard = self.defs.lock().unwrap();
            Ok(guard
                .get(&environment_id.as_uuid().to_string())
                .cloned()
                .unwrap_or_default())
        }

        async fn list_by_environment_paginated(
            &self,
            _environment_id: EnvironmentId,
            _offset: u64,
            _limit: u64,
            _include_archived: bool,
        ) -> Result<(Vec<EventDefinition>, u64), RepositoryError> {
            unimplemented!("not used in these tests")
        }

        async fn create(&self, def: &EventDefinition) -> Result<(), RepositoryError> {
            let mut guard = self.defs.lock().unwrap();
            guard
                .entry(def.environment_id.as_uuid().to_string())
                .or_default()
                .push(def.clone());
            Ok(())
        }

        async fn update(&self, def: &EventDefinition) -> Result<EventDefinition, RepositoryError> {
            Ok(def.clone())
        }

        async fn soft_delete(&self, id: EventDefinitionId) -> Result<(), RepositoryError> {
            let mut guard = self.defs.lock().unwrap();
            for defs in guard.values_mut() {
                defs.retain(|d| d.id != id);
            }
            Ok(())
        }
    }

    // -----------------------------------------------------------------------
    // Helpers
    // -----------------------------------------------------------------------

    fn noop_event_writer() -> EventWriter {
        let client = clickhouse::Client::default().with_url("http://127.0.0.1:1");
        EventWriter::new(client)
    }

    fn make_state(
        env_id: EnvironmentId,
        defs: Vec<(String, EventValueType)>,
    ) -> EventIngestionState {
        let raw_key = "sk_test_key";
        let key_hash = hash_sdk_key(raw_key);

        EventIngestionState {
            sdk_key_repo: Arc::new(MockSdkKeyRepo::new_with_key(key_hash, env_id)),
            event_def_repo: Arc::new(MockEventDefRepo::new(env_id, defs)),
            event_writer: noop_event_writer(),
        }
    }

    fn make_request_with_sdk_key(
        events: Vec<MetricEvent>,
        raw_key: &str,
    ) -> Request<IngestEventRequest> {
        let mut req = Request::new(IngestEventRequest { events });
        req.metadata_mut().insert(
            "x-sdk-key",
            MetadataValue::try_from(raw_key).expect("valid ascii"),
        );
        req
    }

    // -----------------------------------------------------------------------
    // hash_sdk_key
    // -----------------------------------------------------------------------

    #[test]
    fn hash_sdk_key_is_deterministic() {
        let h1 = hash_sdk_key("sk_test_abc");
        let h2 = hash_sdk_key("sk_test_abc");
        assert_eq!(h1, h2);
        assert_eq!(h1.len(), 64, "SHA-256 hex is 64 chars");
    }

    #[test]
    fn hash_sdk_key_differs_for_different_keys() {
        assert_ne!(hash_sdk_key("sk_a"), hash_sdk_key("sk_b"));
    }

    // -----------------------------------------------------------------------
    // Authentication
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn rejects_missing_sdk_key_header() {
        let state = make_state(EnvironmentId::new(), vec![]);
        let req = Request::new(IngestEventRequest { events: vec![] });
        let err = handle_ingest_event(&state, req).await.unwrap_err();
        assert_eq!(err.code(), tonic::Code::Unauthenticated);
        assert!(err.message().contains("missing x-sdk-key"));
    }

    #[tokio::test]
    async fn rejects_invalid_sdk_key() {
        let state = make_state(EnvironmentId::new(), vec![]);
        let req = make_request_with_sdk_key(vec![], "sk_bad_key");
        let err = handle_ingest_event(&state, req).await.unwrap_err();
        assert_eq!(err.code(), tonic::Code::Unauthenticated);
        assert!(err.message().contains("invalid or revoked"));
    }

    // -----------------------------------------------------------------------
    // Rejection
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn rejects_unknown_metric_key() {
        let env_id = EnvironmentId::new();
        let state = make_state(env_id, vec![("click_count".into(), EventValueType::Int)]);
        let events = vec![MetricEvent {
            metric_key: "unknown_metric".into(),
            context_type: "user".into(),
            context_key: "u1".into(),
            value: Some(MetricValue {
                value: Some(Value::IntValue(1)),
            }),
            timestamp_ms: 0,
        }];
        let req = make_request_with_sdk_key(events, "sk_test_key");
        let body = handle_ingest_event(&state, req).await.unwrap().into_inner();
        assert_eq!(body.accepted_count, 0);
        assert_eq!(body.rejected_keys, vec!["unknown_metric"]);
    }

    #[tokio::test]
    async fn rejects_type_mismatch_bool_sent_int() {
        let env_id = EnvironmentId::new();
        let state = make_state(env_id, vec![("converted".into(), EventValueType::Bool)]);
        let events = vec![MetricEvent {
            metric_key: "converted".into(),
            context_type: "user".into(),
            context_key: "u1".into(),
            value: Some(MetricValue {
                value: Some(Value::IntValue(1)),
            }),
            timestamp_ms: 0,
        }];
        let req = make_request_with_sdk_key(events, "sk_test_key");
        let body = handle_ingest_event(&state, req).await.unwrap().into_inner();
        assert_eq!(body.accepted_count, 0);
        assert_eq!(body.rejected_keys, vec!["converted"]);
    }

    #[tokio::test]
    async fn rejects_missing_value() {
        let env_id = EnvironmentId::new();
        let state = make_state(env_id, vec![("click_count".into(), EventValueType::Int)]);
        let events = vec![MetricEvent {
            metric_key: "click_count".into(),
            context_type: "user".into(),
            context_key: "u1".into(),
            value: None,
            timestamp_ms: 0,
        }];
        let req = make_request_with_sdk_key(events, "sk_test_key");
        let body = handle_ingest_event(&state, req).await.unwrap().into_inner();
        assert_eq!(body.accepted_count, 0);
    }

    // -----------------------------------------------------------------------
    // Acceptance
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn accepts_valid_bool_event() {
        let env_id = EnvironmentId::new();
        let state = make_state(env_id, vec![("converted".into(), EventValueType::Bool)]);
        let events = vec![MetricEvent {
            metric_key: "converted".into(),
            context_type: "user".into(),
            context_key: "u1".into(),
            value: Some(MetricValue {
                value: Some(Value::BoolValue(true)),
            }),
            timestamp_ms: 1_000,
        }];
        let req = make_request_with_sdk_key(events, "sk_test_key");
        let body = handle_ingest_event(&state, req).await.unwrap().into_inner();
        assert_eq!(body.accepted_count, 1);
        assert!(body.rejected_keys.is_empty());
    }

    #[tokio::test]
    async fn accepts_valid_int_event() {
        let env_id = EnvironmentId::new();
        let state = make_state(env_id, vec![("click_count".into(), EventValueType::Int)]);
        let events = vec![MetricEvent {
            metric_key: "click_count".into(),
            context_type: "user".into(),
            context_key: "u42".into(),
            value: Some(MetricValue {
                value: Some(Value::IntValue(7)),
            }),
            timestamp_ms: 2_000,
        }];
        let req = make_request_with_sdk_key(events, "sk_test_key");
        let body = handle_ingest_event(&state, req).await.unwrap().into_inner();
        assert_eq!(body.accepted_count, 1);
        assert!(body.rejected_keys.is_empty());
    }

    #[tokio::test]
    async fn accepts_valid_double_event() {
        let env_id = EnvironmentId::new();
        let state = make_state(env_id, vec![("revenue".into(), EventValueType::Double)]);
        let events = vec![MetricEvent {
            metric_key: "revenue".into(),
            context_type: "session".into(),
            context_key: "s1".into(),
            value: Some(MetricValue {
                value: Some(Value::DoubleValue(9.99)),
            }),
            timestamp_ms: 3_000,
        }];
        let req = make_request_with_sdk_key(events, "sk_test_key");
        let body = handle_ingest_event(&state, req).await.unwrap().into_inner();
        assert_eq!(body.accepted_count, 1);
        assert!(body.rejected_keys.is_empty());
    }

    // -----------------------------------------------------------------------
    // Mixed batch
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn partial_batch_accepted_and_rejected() {
        let env_id = EnvironmentId::new();
        let state = make_state(
            env_id,
            vec![
                ("converted".into(), EventValueType::Bool),
                ("click_count".into(), EventValueType::Int),
            ],
        );
        let events = vec![
            MetricEvent {
                metric_key: "converted".into(),
                context_type: "user".into(),
                context_key: "u1".into(),
                value: Some(MetricValue {
                    value: Some(Value::BoolValue(true)),
                }),
                timestamp_ms: 1_000,
            },
            MetricEvent {
                metric_key: "unknown".into(),
                context_type: "user".into(),
                context_key: "u2".into(),
                value: Some(MetricValue {
                    value: Some(Value::IntValue(1)),
                }),
                timestamp_ms: 1_000,
            },
            MetricEvent {
                metric_key: "click_count".into(),
                context_type: "user".into(),
                context_key: "u3".into(),
                value: Some(MetricValue {
                    value: Some(Value::DoubleValue(1.0)),
                }),
                timestamp_ms: 1_000,
            },
            MetricEvent {
                metric_key: "click_count".into(),
                context_type: "user".into(),
                context_key: "u4".into(),
                value: Some(MetricValue {
                    value: Some(Value::IntValue(5)),
                }),
                timestamp_ms: 2_000,
            },
        ];
        let req = make_request_with_sdk_key(events, "sk_test_key");
        let body = handle_ingest_event(&state, req).await.unwrap().into_inner();
        assert_eq!(body.accepted_count, 2);
        assert_eq!(body.rejected_keys.len(), 2);
        assert!(body.rejected_keys.contains(&"unknown".to_string()));
        assert!(body.rejected_keys.contains(&"click_count".to_string()));
    }

    #[tokio::test]
    async fn empty_batch_returns_zero_counts() {
        let env_id = EnvironmentId::new();
        let state = make_state(env_id, vec![]);
        let req = make_request_with_sdk_key(vec![], "sk_test_key");
        let body = handle_ingest_event(&state, req).await.unwrap().into_inner();
        assert_eq!(body.accepted_count, 0);
        assert!(body.rejected_keys.is_empty());
    }

    // -----------------------------------------------------------------------
    // Database error
    // -----------------------------------------------------------------------

    struct AlwaysFailingEventDefRepo;

    #[async_trait]
    impl EventDefinitionRepository for AlwaysFailingEventDefRepo {
        async fn find_by_id(
            &self,
            _id: EventDefinitionId,
        ) -> Result<EventDefinition, RepositoryError> {
            Err(RepositoryError::NotFound { id: "mock".into() })
        }

        async fn find_by_key(
            &self,
            _key: &str,
            _environment_id: EnvironmentId,
        ) -> Result<EventDefinition, RepositoryError> {
            Err(RepositoryError::NotFound { id: "mock".into() })
        }

        async fn list_by_environment(
            &self,
            _environment_id: EnvironmentId,
        ) -> Result<Vec<EventDefinition>, RepositoryError> {
            Err(RepositoryError::Unexpected(anyhow::anyhow!(
                "db unavailable"
            )))
        }

        async fn list_by_environment_paginated(
            &self,
            _environment_id: EnvironmentId,
            _offset: u64,
            _limit: u64,
            _include_archived: bool,
        ) -> Result<(Vec<EventDefinition>, u64), RepositoryError> {
            Err(RepositoryError::Unexpected(anyhow::anyhow!(
                "db unavailable"
            )))
        }

        async fn create(&self, _def: &EventDefinition) -> Result<(), RepositoryError> {
            Ok(())
        }

        async fn update(&self, def: &EventDefinition) -> Result<EventDefinition, RepositoryError> {
            Ok(def.clone())
        }

        async fn soft_delete(&self, _id: EventDefinitionId) -> Result<(), RepositoryError> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn returns_internal_error_when_db_fails() {
        let env_id = EnvironmentId::new();
        let key_hash = hash_sdk_key("sk_test_key");
        let state = EventIngestionState {
            sdk_key_repo: Arc::new(MockSdkKeyRepo::new_with_key(key_hash, env_id)),
            event_def_repo: Arc::new(AlwaysFailingEventDefRepo),
            event_writer: noop_event_writer(),
        };
        let req = make_request_with_sdk_key(vec![], "sk_test_key");
        let err = handle_ingest_event(&state, req).await.unwrap_err();
        assert_eq!(err.code(), tonic::Code::Internal);
        assert!(err.message().contains("db unavailable"));
    }
}
