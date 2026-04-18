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
    /// Create a config with sensible defaults (30-second poll interval, no LFU cache).
    pub fn new(
        grpc_url: impl Into<String>,
        http_url: impl Into<String>,
        sdk_key: impl Into<String>,
    ) -> Self {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sdk_config_new_sets_defaults() {
        let cfg = SdkConfig::new("http://grpc:9090", "http://api:8080", "my-sdk-key");
        assert_eq!(cfg.grpc_url, "http://grpc:9090");
        assert_eq!(cfg.http_url, "http://api:8080");
        assert_eq!(cfg.sdk_key, "my-sdk-key");
        assert_eq!(cfg.poll_interval, Duration::from_secs(30));
        assert!(cfg.lfu.is_none());
    }

    #[test]
    fn sdk_config_new_accepts_owned_strings() {
        let grpc = String::from("http://grpc:9090");
        let http = String::from("http://api:8080");
        let key = String::from("key");
        let cfg = SdkConfig::new(grpc, http, key);
        assert_eq!(cfg.grpc_url, "http://grpc:9090");
        assert_eq!(cfg.http_url, "http://api:8080");
        assert_eq!(cfg.sdk_key, "key");
    }

    #[test]
    fn sdk_config_with_lfu_config() {
        let mut cfg = SdkConfig::new("http://grpc:9090", "http://api:8080", "key");
        cfg.lfu = Some(LfuConfig {
            capacity: 100,
            window: Duration::from_secs(60),
        });
        let lfu = cfg.lfu.as_ref().unwrap();
        assert_eq!(lfu.capacity, 100);
        assert_eq!(lfu.window, Duration::from_secs(60));
    }

    #[test]
    fn sdk_config_poll_interval_can_be_overridden() {
        let mut cfg = SdkConfig::new("http://grpc:9090", "http://api:8080", "key");
        cfg.poll_interval = Duration::from_secs(10);
        assert_eq!(cfg.poll_interval, Duration::from_secs(10));
    }

    #[test]
    fn lfu_config_fields() {
        let lfu = LfuConfig {
            capacity: 50,
            window: Duration::from_millis(500),
        };
        assert_eq!(lfu.capacity, 50);
        assert_eq!(lfu.window, Duration::from_millis(500));
    }
}
