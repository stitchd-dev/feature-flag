use std::time::Duration;

/// Configuration for the SDK client.
pub struct SdkConfig {
    /// gRPC endpoint for flag definition sync (e.g. `http://localhost:9090`).
    pub grpc_url: String,
    /// Base URL for REST list-check calls (e.g. `http://localhost:8080`).
    pub http_url: String,
    /// Raw SDK key (hashed by the server on each request).
    pub sdk_key: String,
    /// How often to poll for updated flag definitions.
    pub poll_interval: Duration,
    /// Optional LFU cache for list-segment membership.
    pub lfu: Option<LfuConfig>,
}

impl SdkConfig {
    pub fn new(grpc_url: impl Into<String>, http_url: impl Into<String>, sdk_key: impl Into<String>) -> Self {
        Self {
            grpc_url: grpc_url.into(),
            http_url: http_url.into(),
            sdk_key: sdk_key.into(),
            poll_interval: Duration::from_secs(30),
            lfu: None,
        }
    }
}

/// Configuration for the optional LFU segment membership cache.
pub struct LfuConfig {
    /// Maximum number of contexts to keep in the hot set.
    pub capacity: usize,
    /// Frequency counting window.
    pub window: Duration,
}
