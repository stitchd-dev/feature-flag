/// CQL migration applier for bootstrapping and evolving the Scylla schema.
pub mod migrate;

/// Scylla driver metrics → Prometheus bridge.
pub mod metrics;

/// OpenTelemetry-compatible tracing helpers for Scylla operations.
///
/// Named `otel` (not `tracing`) to avoid shadowing the `tracing` crate in
/// this module's namespace, which would break `#[tracing::instrument]`.
pub mod otel;

/// ScyllaDB-backed list-segment store.
pub mod segment;

use std::{collections::HashMap, sync::Arc};

use scylla::{
    client::session::Session, errors::NewSessionError, statement::prepared::PreparedStatement,
};
use thiserror::Error;
use tokio::sync::RwLock;

/// Errors produced by `ScyllaDB` operations.
#[derive(Debug, Error)]
pub enum ScyllaError {
    /// Failed to establish a session.
    #[error("connection failed: {0}")]
    Connection(String),
    /// A CQL query execution error.
    #[error("query failed: {0}")]
    Query(String),
    /// A statement could not be prepared.
    #[error("prepare failed: {0}")]
    Prepare(String),
    /// Keyspace or schema error.
    #[error("schema error: {0}")]
    Schema(String),
}

impl From<NewSessionError> for ScyllaError {
    fn from(e: NewSessionError) -> Self {
        Self::Connection(e.to_string())
    }
}

/// Configuration for connecting to a `ScyllaDB` cluster.
#[derive(Debug, Clone)]
pub struct ScyllaConfig {
    /// CQL contact point (e.g. `"127.0.0.1:9042"`).
    pub uri: String,
    /// Keyspace to use for all queries.
    pub keyspace: String,
}

impl ScyllaConfig {
    /// Create a config from the standard env vars:
    /// `STITCHD_SCYLLA_URI` (default `127.0.0.1:9042`) and `STITCHD_SCYLLA_KEYSPACE` (default `stitchd`).
    #[must_use]
    pub fn from_env() -> Self {
        Self {
            uri: std::env::var("STITCHD_SCYLLA_URI").unwrap_or_else(|_| "127.0.0.1:9042".to_string()),
            keyspace: std::env::var("STITCHD_SCYLLA_KEYSPACE").unwrap_or_else(|_| "stitchd".to_string()),
        }
    }
}

/// A thin wrapper around a `scylla::Session` with a shared prepared-statement cache.
///
/// Prepared statements are keyed by the CQL string and built once at first use.
/// Subsequent calls to [`ScyllaClient::prepare`] for the same CQL hit the in-memory
/// cache without a round-trip to the cluster.
#[derive(Clone)]
pub struct ScyllaClient {
    session: Arc<Session>,
    prepared: Arc<RwLock<HashMap<String, PreparedStatement>>>,
    keyspace: String,
}

impl ScyllaClient {
    /// Connect to `ScyllaDB` and return a client.
    ///
    /// # Errors
    /// Returns [`ScyllaError::Connection`] if the cluster is unreachable.
    #[allow(clippy::large_futures)]
    pub async fn connect(config: &ScyllaConfig) -> Result<Self, ScyllaError> {
        let session = scylla::client::session_builder::SessionBuilder::new()
            .known_node(config.uri.as_str())
            .build()
            .await
            .map_err(|e| ScyllaError::Connection(e.to_string()))?;

        Ok(Self {
            session: Arc::new(session),
            prepared: Arc::new(RwLock::new(HashMap::new())),
            keyspace: config.keyspace.clone(),
        })
    }

    /// Return a reference to the underlying `Session`.
    #[must_use]
    pub fn session(&self) -> &Session {
        &self.session
    }

    /// Return the keyspace this client is configured for.
    #[must_use]
    pub fn keyspace(&self) -> &str {
        &self.keyspace
    }

    /// Prepare a CQL statement, returning the cached version if already prepared.
    ///
    /// Emits a `scylla.query` tracing span (OTel-compatible via `tracing-opentelemetry`)
    /// so preparation round-trips appear in distributed traces.
    ///
    /// # Errors
    /// Returns [`ScyllaError::Prepare`] if the statement cannot be parsed by the server.
    #[tracing::instrument(skip(self), fields(db.system = "scylladb", db.operation = "prepare"))]
    pub async fn prepare(&self, cql: &str) -> Result<PreparedStatement, ScyllaError> {
        {
            let cache = self.prepared.read().await;
            if let Some(stmt) = cache.get(cql) {
                return Ok(stmt.clone());
            }
        }

        let stmt = self
            .session
            .prepare(cql)
            .await
            .map_err(|e| ScyllaError::Prepare(e.to_string()))?;

        self.prepared
            .write()
            .await
            .insert(cql.to_string(), stmt.clone());
        Ok(stmt)
    }

    /// Return the number of statements currently in the prepared-statement cache.
    pub async fn cached_statement_count(&self) -> usize {
        self.prepared.read().await.len()
    }

    /// Collect a [`metrics::ScyllaMetricsSnapshot`] and emit all values to the
    /// global `metrics` recorder.
    ///
    /// Call this on a periodic background interval (e.g. every 15 s) to keep
    /// the Prometheus endpoint up to date.
    pub async fn record_metrics(&self) {
        let driver_metrics = self.session.get_metrics();
        let cache_size = self.cached_statement_count().await as u64;
        let snapshot = self::metrics::ScyllaMetricsSnapshot::collect(&driver_metrics, cache_size);
        self::metrics::emit_metrics(&snapshot);
    }
}
