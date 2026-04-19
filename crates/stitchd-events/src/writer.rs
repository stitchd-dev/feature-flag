//! Async ClickHouse event writer with OpenTelemetry instrumentation.
//!
//! `EventWriter::write` and `EventWriter::write_batch` do the actual insert.
//! Callers should `tokio::spawn` the returned future to achieve fire-and-forget
//! semantics — REST handlers return 202 immediately while the write proceeds in
//! the background.

use clickhouse::Client;
use serde::Serialize;
use stitchd_core::event::{BatchEventPayload, EventPayload, EventValue};
use tracing::instrument;
use uuid::Uuid;

/// Error type for ClickHouse write operations.
#[derive(Debug, thiserror::Error)]
pub enum WriteError {
    /// ClickHouse client error.
    #[error("clickhouse insert failed: {0}")]
    Clickhouse(#[from] clickhouse::error::Error),
}

/// A row in the `events` ClickHouse table.
///
/// Fields map 1-to-1 with the table DDL. Nullable value columns are `Option`
/// so exactly one is `Some` depending on the event's value type.
#[derive(Debug, Serialize, clickhouse::Row)]
pub struct EventRow {
    /// Environment UUID.
    #[serde(with = "clickhouse::serde::uuid")]
    pub env_id: Uuid,
    /// Evaluation-time context pairs: (type, key).
    pub contexts: Vec<(String, String)>,
    /// Pre-registered metric key.
    pub metric_key: String,
    /// Boolean value, set when the event carries a `Bool` metric.
    pub value_bool: Option<bool>,
    /// Integer value, set when the event carries an `Int` metric.
    pub value_int: Option<i64>,
    /// Double value, set when the event carries a `Double` metric.
    pub value_double: Option<f64>,
    /// Client-supplied event timestamp as Unix milliseconds.
    pub timestamp: i64,
}

impl EventRow {
    fn from_payload(env_id: Uuid, payload: &EventPayload) -> Self {
        let contexts = payload
            .contexts
            .iter()
            .map(|c| (c.context_type.clone(), c.key.clone()))
            .collect();

        let (value_bool, value_int, value_double) = match payload.value {
            EventValue::Bool(v) => (Some(v), None, None),
            EventValue::Int(v) => (None, Some(v), None),
            EventValue::Double(v) => (None, None, Some(v)),
        };

        Self {
            env_id,
            contexts,
            metric_key: payload.metric_key.clone(),
            value_bool,
            value_int,
            value_double,
            timestamp: payload.timestamp.timestamp_millis(),
        }
    }
}

/// Writes events to ClickHouse.
#[derive(Clone)]
pub struct EventWriter {
    client: Client,
}

impl EventWriter {
    /// Create a new `EventWriter` backed by `client`.
    #[must_use]
    pub fn new(client: Client) -> Self {
        Self { client }
    }

    /// Write a single event to the `events` table.
    ///
    /// This is `async` — wrap in `tokio::spawn` for fire-and-forget.
    #[instrument(skip(self, payload), fields(metric_key = %payload.metric_key))]
    pub async fn write(&self, env_id: Uuid, payload: &EventPayload) -> Result<(), WriteError> {
        let row = EventRow::from_payload(env_id, payload);
        let mut insert = self.client.insert("events")?;
        insert.write(&row).await?;
        insert.end().await?;
        Ok(())
    }

    /// Write a batch of events to the `events` table.
    ///
    /// All events are inserted in a single ClickHouse insert statement.
    /// Wrap in `tokio::spawn` for fire-and-forget.
    #[instrument(skip(self, batch), fields(count = batch.events.len()))]
    pub async fn write_batch(
        &self,
        env_id: Uuid,
        batch: &BatchEventPayload,
    ) -> Result<(), WriteError> {
        let mut insert = self.client.insert("events")?;
        for payload in &batch.events {
            let row = EventRow::from_payload(env_id, payload);
            insert.write(&row).await?;
        }
        insert.end().await?;
        Ok(())
    }
}
