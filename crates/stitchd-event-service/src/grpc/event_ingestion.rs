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
    IngestRequest, IngestResponse, event_ingestion_service_server::EventIngestionService,
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
                (
                    EventValueType::Bool,
                    Some(stitchd_proto::events::v1::metric_value::Value::BoolValue(_))
                ) | (
                    EventValueType::Int,
                    Some(stitchd_proto::events::v1::metric_value::Value::IntValue(_))
                ) | (
                    EventValueType::Double,
                    Some(stitchd_proto::events::v1::metric_value::Value::DoubleValue(
                        _
                    ))
                )
            );

            if !type_ok {
                rejected_keys.push(event.metric_key.clone());
                continue;
            }

            // Build ClickHouse row.
            let (value_bool, value_int, value_double) = match value {
                Some(stitchd_proto::events::v1::metric_value::Value::BoolValue(b)) => {
                    (Some(*b), None, None)
                }
                Some(stitchd_proto::events::v1::metric_value::Value::IntValue(i)) => {
                    (None, Some(*i), None)
                }
                Some(stitchd_proto::events::v1::metric_value::Value::DoubleValue(d)) => {
                    (None, None, Some(*d))
                }
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
        metrics::counter!("event_service.events.rejected").increment(rejected_keys.len() as u64);

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
    use stitchd_proto::events::v1::{Event, IngestRequest, MetricValue, metric_value::Value};

    use super::*;

    // -----------------------------------------------------------------------
    // Mock: SdkKeyRepository
    // -----------------------------------------------------------------------

    /// Simple mock SDK key repository.
    /// `find_active_by_hash` returns the pre-loaded key if the hash matches.
    struct MockSdkKeyRepo {
        /// `key_hash` → `SdkKey`
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
        /// `env_id` string → list of `EventDefinition`
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
                    key,
                    value_type,
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

        async fn create(&self, def: &EventDefinition) -> Result<(), RepositoryError> {
            {
                let mut guard = self.defs.lock().unwrap();
                guard
                    .entry(def.environment_id.as_uuid().to_string())
                    .or_default()
                    .push(def.clone());
            }
            Ok(())
        }

        async fn update(&self, def: &EventDefinition) -> Result<EventDefinition, RepositoryError> {
            Ok(def.clone())
        }

        async fn soft_delete(&self, id: EventDefinitionId) -> Result<(), RepositoryError> {
            {
                let mut guard = self.defs.lock().unwrap();
                for defs in guard.values_mut() {
                    defs.retain(|d| d.id != id);
                }
            }
            Ok(())
        }
    }

    // -----------------------------------------------------------------------
    // Helpers
    // -----------------------------------------------------------------------

    /// Build a minimal `EventWriter` backed by a no-op ClickHouse client.
    ///
    /// Tests never actually write to ClickHouse; the fire-and-forget spawn
    /// will fail silently, which is acceptable for unit tests.
    fn noop_event_writer() -> EventWriter {
        let client = clickhouse::Client::default().with_url("http://127.0.0.1:1");
        EventWriter::new(client)
    }

    fn make_service(
        env_id: EnvironmentId,
        defs: Vec<(String, EventValueType)>,
    ) -> EventIngestionServiceImpl {
        let raw_key = "sk_test_key";
        let key_hash = hash_sdk_key(raw_key);

        let sdk_key_repo: Arc<dyn SdkKeyRepository> =
            Arc::new(MockSdkKeyRepo::new_with_key(key_hash, env_id));
        let event_def_repo: Arc<dyn EventDefinitionRepository> =
            Arc::new(MockEventDefRepo::new(env_id, defs));

        EventIngestionServiceImpl::new(ServiceState {
            event_def_repo,
            sdk_key_repo,
            event_writer: noop_event_writer(),
        })
    }

    fn make_request_with_sdk_key(events: Vec<Event>, raw_key: &str) -> Request<IngestRequest> {
        let mut req = Request::new(IngestRequest { events });
        req.metadata_mut().insert(
            "x-sdk-key",
            MetadataValue::try_from(raw_key).expect("valid ascii"),
        );
        req
    }

    // -----------------------------------------------------------------------
    // hash_sdk_key unit tests
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
        let h1 = hash_sdk_key("sk_test_abc");
        let h2 = hash_sdk_key("sk_test_xyz");
        assert_ne!(h1, h2);
    }

    // -----------------------------------------------------------------------
    // Authentication tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn rejects_request_missing_sdk_key_header() {
        let env_id = EnvironmentId::new();
        let svc = make_service(env_id, vec![]);

        // No x-sdk-key header
        let req = Request::new(IngestRequest { events: vec![] });
        let resp = svc.ingest_event(req).await;
        assert!(resp.is_err());
        let status = resp.unwrap_err();
        assert_eq!(status.code(), tonic::Code::Unauthenticated);
        assert!(status.message().contains("missing x-sdk-key"));
    }

    #[tokio::test]
    async fn rejects_request_with_invalid_sdk_key() {
        let env_id = EnvironmentId::new();
        let svc = make_service(env_id, vec![]);

        // SDK key that is not in the repository
        let req = make_request_with_sdk_key(vec![], "sk_bad_key");
        let resp = svc.ingest_event(req).await;
        assert!(resp.is_err());
        let status = resp.unwrap_err();
        assert_eq!(status.code(), tonic::Code::Unauthenticated);
        assert!(status.message().contains("invalid or revoked"));
    }

    // -----------------------------------------------------------------------
    // Unknown key rejection tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn rejects_event_with_unknown_metric_key() {
        let env_id = EnvironmentId::new();
        // Registry has "click_count" (Int) only — "unknown_metric" is not registered.
        let svc = make_service(
            env_id,
            vec![("click_count".to_string(), EventValueType::Int)],
        );

        let events = vec![Event {
            metric_key: "unknown_metric".to_string(),
            context_type: "user".to_string(),
            context_key: "u1".to_string(),
            value: Some(MetricValue {
                value: Some(Value::IntValue(1)),
            }),
            timestamp_ms: 0,
        }];

        let req = make_request_with_sdk_key(events, "sk_test_key");
        let resp = svc
            .ingest_event(req)
            .await
            .expect("handler should not error");
        let body = resp.into_inner();
        assert_eq!(body.accepted_count, 0);
        assert_eq!(body.rejected_keys, vec!["unknown_metric"]);
    }

    #[tokio::test]
    async fn rejects_all_events_when_none_registered() {
        let env_id = EnvironmentId::new();
        // Empty registry — all events must be rejected.
        let svc = make_service(env_id, vec![]);

        let events = vec![
            Event {
                metric_key: "a".to_string(),
                context_type: "user".to_string(),
                context_key: "u1".to_string(),
                value: Some(MetricValue {
                    value: Some(Value::BoolValue(true)),
                }),
                timestamp_ms: 0,
            },
            Event {
                metric_key: "b".to_string(),
                context_type: "user".to_string(),
                context_key: "u2".to_string(),
                value: Some(MetricValue {
                    value: Some(Value::IntValue(42)),
                }),
                timestamp_ms: 0,
            },
        ];

        let req = make_request_with_sdk_key(events, "sk_test_key");
        let resp = svc
            .ingest_event(req)
            .await
            .expect("handler should not error");
        let body = resp.into_inner();
        assert_eq!(body.accepted_count, 0);
        assert_eq!(body.rejected_keys.len(), 2);
        assert!(body.rejected_keys.contains(&"a".to_string()));
        assert!(body.rejected_keys.contains(&"b".to_string()));
    }

    // -----------------------------------------------------------------------
    // Type mismatch rejection tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn rejects_bool_key_with_int_value() {
        let env_id = EnvironmentId::new();
        // "converted" is registered as Bool, but we send an Int value.
        let svc = make_service(
            env_id,
            vec![("converted".to_string(), EventValueType::Bool)],
        );

        let events = vec![Event {
            metric_key: "converted".to_string(),
            context_type: "user".to_string(),
            context_key: "u1".to_string(),
            value: Some(MetricValue {
                value: Some(Value::IntValue(1)),
            }),
            timestamp_ms: 0,
        }];

        let req = make_request_with_sdk_key(events, "sk_test_key");
        let resp = svc
            .ingest_event(req)
            .await
            .expect("handler should not error");
        let body = resp.into_inner();
        assert_eq!(body.accepted_count, 0);
        assert_eq!(body.rejected_keys, vec!["converted"]);
    }

    #[tokio::test]
    async fn rejects_int_key_with_double_value() {
        let env_id = EnvironmentId::new();
        let svc = make_service(
            env_id,
            vec![("click_count".to_string(), EventValueType::Int)],
        );

        let events = vec![Event {
            metric_key: "click_count".to_string(),
            context_type: "user".to_string(),
            context_key: "u1".to_string(),
            value: Some(MetricValue {
                value: Some(Value::DoubleValue(1.5)),
            }),
            timestamp_ms: 0,
        }];

        let req = make_request_with_sdk_key(events, "sk_test_key");
        let resp = svc
            .ingest_event(req)
            .await
            .expect("handler should not error");
        let body = resp.into_inner();
        assert_eq!(body.accepted_count, 0);
        assert_eq!(body.rejected_keys, vec!["click_count"]);
    }

    #[tokio::test]
    async fn rejects_double_key_with_bool_value() {
        let env_id = EnvironmentId::new();
        let svc = make_service(
            env_id,
            vec![("revenue".to_string(), EventValueType::Double)],
        );

        let events = vec![Event {
            metric_key: "revenue".to_string(),
            context_type: "user".to_string(),
            context_key: "u1".to_string(),
            value: Some(MetricValue {
                value: Some(Value::BoolValue(false)),
            }),
            timestamp_ms: 0,
        }];

        let req = make_request_with_sdk_key(events, "sk_test_key");
        let resp = svc
            .ingest_event(req)
            .await
            .expect("handler should not error");
        let body = resp.into_inner();
        assert_eq!(body.accepted_count, 0);
        assert_eq!(body.rejected_keys, vec!["revenue"]);
    }

    #[tokio::test]
    async fn rejects_event_with_missing_value() {
        let env_id = EnvironmentId::new();
        let svc = make_service(
            env_id,
            vec![("click_count".to_string(), EventValueType::Int)],
        );

        // value field is None (missing)
        let events = vec![Event {
            metric_key: "click_count".to_string(),
            context_type: "user".to_string(),
            context_key: "u1".to_string(),
            value: None,
            timestamp_ms: 0,
        }];

        let req = make_request_with_sdk_key(events, "sk_test_key");
        let resp = svc
            .ingest_event(req)
            .await
            .expect("handler should not error");
        let body = resp.into_inner();
        assert_eq!(body.accepted_count, 0);
        assert_eq!(body.rejected_keys, vec!["click_count"]);
    }

    #[tokio::test]
    async fn rejects_event_with_empty_metric_value_oneof() {
        let env_id = EnvironmentId::new();
        let svc = make_service(
            env_id,
            vec![("click_count".to_string(), EventValueType::Int)],
        );

        // MetricValue present but inner oneof is None
        let events = vec![Event {
            metric_key: "click_count".to_string(),
            context_type: "user".to_string(),
            context_key: "u1".to_string(),
            value: Some(MetricValue { value: None }),
            timestamp_ms: 0,
        }];

        let req = make_request_with_sdk_key(events, "sk_test_key");
        let resp = svc
            .ingest_event(req)
            .await
            .expect("handler should not error");
        let body = resp.into_inner();
        assert_eq!(body.accepted_count, 0);
        assert_eq!(body.rejected_keys, vec!["click_count"]);
    }

    // -----------------------------------------------------------------------
    // Successful ingestion tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn accepts_valid_bool_event() {
        let env_id = EnvironmentId::new();
        let svc = make_service(
            env_id,
            vec![("converted".to_string(), EventValueType::Bool)],
        );

        let events = vec![Event {
            metric_key: "converted".to_string(),
            context_type: "user".to_string(),
            context_key: "u1".to_string(),
            value: Some(MetricValue {
                value: Some(Value::BoolValue(true)),
            }),
            timestamp_ms: 1_000,
        }];

        let req = make_request_with_sdk_key(events, "sk_test_key");
        let resp = svc
            .ingest_event(req)
            .await
            .expect("handler should not error");
        let body = resp.into_inner();
        assert_eq!(body.accepted_count, 1);
        assert!(body.rejected_keys.is_empty());
    }

    #[tokio::test]
    async fn accepts_valid_int_event() {
        let env_id = EnvironmentId::new();
        let svc = make_service(
            env_id,
            vec![("click_count".to_string(), EventValueType::Int)],
        );

        let events = vec![Event {
            metric_key: "click_count".to_string(),
            context_type: "user".to_string(),
            context_key: "u42".to_string(),
            value: Some(MetricValue {
                value: Some(Value::IntValue(7)),
            }),
            timestamp_ms: 2_000,
        }];

        let req = make_request_with_sdk_key(events, "sk_test_key");
        let resp = svc
            .ingest_event(req)
            .await
            .expect("handler should not error");
        let body = resp.into_inner();
        assert_eq!(body.accepted_count, 1);
        assert!(body.rejected_keys.is_empty());
    }

    #[tokio::test]
    async fn accepts_valid_double_event() {
        let env_id = EnvironmentId::new();
        let svc = make_service(
            env_id,
            vec![("revenue".to_string(), EventValueType::Double)],
        );

        let events = vec![Event {
            metric_key: "revenue".to_string(),
            context_type: "session".to_string(),
            context_key: "s1".to_string(),
            value: Some(MetricValue {
                value: Some(Value::DoubleValue(9.99)),
            }),
            timestamp_ms: 3_000,
        }];

        let req = make_request_with_sdk_key(events, "sk_test_key");
        let resp = svc
            .ingest_event(req)
            .await
            .expect("handler should not error");
        let body = resp.into_inner();
        assert_eq!(body.accepted_count, 1);
        assert!(body.rejected_keys.is_empty());
    }

    // -----------------------------------------------------------------------
    // Mixed batch: some accepted, some rejected
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn partial_batch_accepted_and_rejected() {
        let env_id = EnvironmentId::new();
        let svc = make_service(
            env_id,
            vec![
                ("converted".to_string(), EventValueType::Bool),
                ("click_count".to_string(), EventValueType::Int),
            ],
        );

        let events = vec![
            // ✓ valid bool
            Event {
                metric_key: "converted".to_string(),
                context_type: "user".to_string(),
                context_key: "u1".to_string(),
                value: Some(MetricValue {
                    value: Some(Value::BoolValue(true)),
                }),
                timestamp_ms: 1_000,
            },
            // ✗ unknown key
            Event {
                metric_key: "unknown".to_string(),
                context_type: "user".to_string(),
                context_key: "u2".to_string(),
                value: Some(MetricValue {
                    value: Some(Value::IntValue(1)),
                }),
                timestamp_ms: 1_000,
            },
            // ✗ type mismatch (Int key, Double value)
            Event {
                metric_key: "click_count".to_string(),
                context_type: "user".to_string(),
                context_key: "u3".to_string(),
                value: Some(MetricValue {
                    value: Some(Value::DoubleValue(1.0)),
                }),
                timestamp_ms: 1_000,
            },
            // ✓ valid int
            Event {
                metric_key: "click_count".to_string(),
                context_type: "user".to_string(),
                context_key: "u4".to_string(),
                value: Some(MetricValue {
                    value: Some(Value::IntValue(5)),
                }),
                timestamp_ms: 2_000,
            },
        ];

        let req = make_request_with_sdk_key(events, "sk_test_key");
        let resp = svc
            .ingest_event(req)
            .await
            .expect("handler should not error");
        let body = resp.into_inner();
        assert_eq!(body.accepted_count, 2);
        assert_eq!(body.rejected_keys.len(), 2);
        assert!(body.rejected_keys.contains(&"unknown".to_string()));
        assert!(body.rejected_keys.contains(&"click_count".to_string()));
    }

    #[tokio::test]
    async fn empty_batch_returns_zero_counts() {
        let env_id = EnvironmentId::new();
        let svc = make_service(env_id, vec![]);

        let req = make_request_with_sdk_key(vec![], "sk_test_key");
        let resp = svc
            .ingest_event(req)
            .await
            .expect("handler should not error");
        let body = resp.into_inner();
        assert_eq!(body.accepted_count, 0);
        assert!(body.rejected_keys.is_empty());
    }

    // -----------------------------------------------------------------------
    // Database error path
    // -----------------------------------------------------------------------

    /// An `EventDefinitionRepository` that always fails `list_by_environment`.
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
    async fn returns_internal_error_when_event_def_repo_fails() {
        let env_id = EnvironmentId::new();
        let raw_key = "sk_test_key";
        let key_hash = hash_sdk_key(raw_key);

        let sdk_key_repo: Arc<dyn SdkKeyRepository> =
            Arc::new(MockSdkKeyRepo::new_with_key(key_hash, env_id));
        let event_def_repo: Arc<dyn EventDefinitionRepository> =
            Arc::new(AlwaysFailingEventDefRepo);

        let svc = EventIngestionServiceImpl::new(ServiceState {
            event_def_repo,
            sdk_key_repo,
            event_writer: noop_event_writer(),
        });

        let req = make_request_with_sdk_key(vec![], raw_key);
        let err = svc
            .ingest_event(req)
            .await
            .expect_err("should return internal error when db fails");

        assert_eq!(err.code(), tonic::Code::Internal);
        assert!(err.message().contains("db unavailable"));
    }
}
