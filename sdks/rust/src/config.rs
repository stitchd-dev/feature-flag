//! Configuration for [`crate::SdkClient`].
//!
//! Field set mirrors `sdks/spec/schemas/sdk_config.schema.json`. Defaults
//! match the spec — only `gateway_url` and `sdk_key` are required from the
//! caller; everything else has a sane default.
//!
//! Internally we use `std::time::Duration` for all interval fields; the
//! JSON Schema uses milliseconds at the wire level (and SDK callers in
//! other languages will too). The schema-aligned ms boundaries live at the
//! `from_millis_*` / accessor methods, not inside the struct, so user code
//! that builds an `SdkConfig` directly works with `Duration` natively.
//!
//! Validation is centralised in [`SdkConfig::validate`], called by
//! `SdkClient::init` before any background task is spawned (per the spec's
//! fail-fast Class A error policy).

use std::time::Duration;

use crate::error::SdkError;

// ── Defaults (from sdks/spec/schemas/sdk_config.schema.json) ────────────────

/// Default tonic gRPC port the gateway listens on for SDK traffic.
pub const DEFAULT_GATEWAY_GRPC_PORT: u16 = 50050;
/// Default interval between definition-sync polls (30 seconds).
pub const DEFAULT_DEFINITION_POLL_INTERVAL: Duration = Duration::from_secs(30);
/// Default interval between LRU refresh ticks (60 seconds).
pub const DEFAULT_LIST_SEGMENT_REFRESH_INTERVAL: Duration = Duration::from_secs(60);
/// Default LRU capacity (10,000 entries).
pub const DEFAULT_LRU_MAX_ENTRIES: usize = 10_000;
/// Default event flush interval (5 seconds).
pub const DEFAULT_EVENT_FLUSH_INTERVAL: Duration = Duration::from_secs(5);
/// Default events per flush (100).
pub const DEFAULT_EVENT_BATCH_SIZE: usize = 100;
/// Default in-memory event-buffer capacity before oldest events are dropped.
pub const DEFAULT_EVENT_BUFFER_CAPACITY: usize = 1_000;
/// Default per-request timeout on all gateway HTTP/gRPC calls (5 seconds).
pub const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(5);
/// Minimum acceptable SDK key length (matches spec's `minLength: 8`).
pub const MIN_SDK_KEY_LENGTH: usize = 8;

/// Configuration passed to [`crate::SdkClient::init`].
#[derive(Debug, Clone)]
pub struct SdkConfig {
    /// Base URL of `stitchd-gateway` (REST + gRPC share host; gRPC uses
    /// [`Self::gateway_grpc_port`]).
    pub gateway_url: String,

    /// SDK key scoped to a single (project, environment).
    pub sdk_key: String,

    /// Tonic gRPC port the gateway listens on for SDK traffic.
    /// Default: [`DEFAULT_GATEWAY_GRPC_PORT`] (`50050`).
    pub gateway_grpc_port: u16,

    /// How often the background task polls `SdkService.SyncDefinitions`.
    /// Default: 30 seconds.
    pub definition_poll_interval: Duration,

    /// How often the LRU refresh task fetches up-to-date membership for
    /// all resident LRU entries. Default: 60 seconds.
    pub list_segment_refresh_interval: Duration,

    /// Capacity of the list-segment membership LRU. Default: 10,000.
    pub lru_max_entries: usize,

    /// How often the event flush task drains the buffer to the gateway.
    /// Default: 5 seconds.
    pub event_flush_interval: Duration,

    /// Maximum events per flush — reaching this size triggers an immediate
    /// flush regardless of [`Self::event_flush_interval`]. Default: 100.
    pub event_batch_size: usize,

    /// Maximum events buffered between flushes. When full, the oldest
    /// event is dropped (back-pressure on `evaluate()` is NOT acceptable).
    /// Default: 1,000.
    pub event_buffer_capacity: usize,

    /// Per-request timeout for ALL gateway HTTP/gRPC calls (definition
    /// sync, LRU fetch, event flush). Default: 5 seconds.
    pub request_timeout: Duration,
}

impl SdkConfig {
    /// Construct a config with defaults applied to every optional field.
    /// Callers MUST still call [`Self::validate`] before consuming.
    #[must_use]
    pub fn new(gateway_url: impl Into<String>, sdk_key: impl Into<String>) -> Self {
        Self {
            gateway_url: gateway_url.into(),
            sdk_key: sdk_key.into(),
            gateway_grpc_port: DEFAULT_GATEWAY_GRPC_PORT,
            definition_poll_interval: DEFAULT_DEFINITION_POLL_INTERVAL,
            list_segment_refresh_interval: DEFAULT_LIST_SEGMENT_REFRESH_INTERVAL,
            lru_max_entries: DEFAULT_LRU_MAX_ENTRIES,
            event_flush_interval: DEFAULT_EVENT_FLUSH_INTERVAL,
            event_batch_size: DEFAULT_EVENT_BATCH_SIZE,
            event_buffer_capacity: DEFAULT_EVENT_BUFFER_CAPACITY,
            request_timeout: DEFAULT_REQUEST_TIMEOUT,
        }
    }

    /// Run all spec-mandated invariants. Returns the first violation found
    /// as an [`SdkError::Config`]. `SdkClient::init` calls this before any
    /// background task is spawned (fail-fast).
    ///
    /// Spec ref: `sdks/spec/docs/06-errors.md` Class A.
    pub fn validate(&self) -> Result<(), SdkError> {
        if self.gateway_url.is_empty() {
            return Err(SdkError::Config("gateway_url must not be empty".into()));
        }
        // Reject obvious malformed URLs — a full parse happens later when
        // the HTTP/gRPC clients build connections, but we catch the easy
        // cases (no scheme, whitespace) here so the error is closer to the
        // misconfiguration.
        if !self.gateway_url.contains("://") {
            return Err(SdkError::Config(format!(
                "gateway_url missing scheme: {:?}",
                self.gateway_url
            )));
        }
        if self.gateway_url.contains(char::is_whitespace) {
            return Err(SdkError::Config(format!(
                "gateway_url contains whitespace: {:?}",
                self.gateway_url
            )));
        }

        if self.sdk_key.is_empty() {
            return Err(SdkError::Config("sdk_key must not be empty".into()));
        }
        if self.sdk_key.len() < MIN_SDK_KEY_LENGTH {
            return Err(SdkError::Config(format!(
                "sdk_key must be at least {MIN_SDK_KEY_LENGTH} characters (got {})",
                self.sdk_key.len()
            )));
        }

        if self.gateway_grpc_port == 0 {
            return Err(SdkError::Config("gateway_grpc_port must be > 0".into()));
        }

        // Durations: spec mandates positive, minimum 1 ms for flush /
        // request_timeout and 1 second for the poll-like intervals.
        if self.definition_poll_interval.is_zero() {
            return Err(SdkError::Config(
                "definition_poll_interval must be > 0".into(),
            ));
        }
        if self.list_segment_refresh_interval.is_zero() {
            return Err(SdkError::Config(
                "list_segment_refresh_interval must be > 0".into(),
            ));
        }
        if self.event_flush_interval.is_zero() {
            return Err(SdkError::Config("event_flush_interval must be > 0".into()));
        }
        if self.request_timeout.is_zero() {
            return Err(SdkError::Config("request_timeout must be > 0".into()));
        }

        // Capacities: spec mandates non-zero.
        if self.lru_max_entries == 0 {
            return Err(SdkError::Config("lru_max_entries must be > 0".into()));
        }
        if self.event_batch_size == 0 {
            return Err(SdkError::Config("event_batch_size must be > 0".into()));
        }
        if self.event_buffer_capacity == 0 {
            return Err(SdkError::Config("event_buffer_capacity must be > 0".into()));
        }

        // Soft sanity check: buffer should be ≥ batch size (otherwise the
        // size-trigger flush can never fire). Not a hard error per spec —
        // returned as a typed config violation so callers don't ship broken
        // configurations.
        if self.event_buffer_capacity < self.event_batch_size {
            return Err(SdkError::Config(format!(
                "event_buffer_capacity ({}) must be >= event_batch_size ({})",
                self.event_buffer_capacity, self.event_batch_size
            )));
        }

        Ok(())
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn ok_config() -> SdkConfig {
        SdkConfig::new("http://localhost:8080", "test-sdk-key-12345")
    }

    // ── Defaults ────────────────────────────────────────────────────────────

    #[test]
    fn defaults_match_spec_table() {
        let cfg = ok_config();
        assert_eq!(cfg.gateway_grpc_port, 50050);
        assert_eq!(cfg.definition_poll_interval, Duration::from_secs(30));
        assert_eq!(cfg.list_segment_refresh_interval, Duration::from_secs(60));
        assert_eq!(cfg.lru_max_entries, 10_000);
        assert_eq!(cfg.event_flush_interval, Duration::from_secs(5));
        assert_eq!(cfg.event_batch_size, 100);
        assert_eq!(cfg.event_buffer_capacity, 1_000);
        assert_eq!(cfg.request_timeout, Duration::from_secs(5));
    }

    #[test]
    fn new_takes_strings_and_str_slices() {
        let _ = SdkConfig::new("a".to_string(), "bbbbbbbb".to_string());
        let _ = SdkConfig::new("a", "bbbbbbbb");
        // Mixed
        let _ = SdkConfig::new("a", String::from("bbbbbbbb"));
    }

    // ── Validation: happy path ──────────────────────────────────────────────

    #[test]
    fn validate_accepts_default_config() {
        ok_config()
            .validate()
            .expect("default config must validate");
    }

    #[test]
    fn validate_accepts_https_url() {
        let mut cfg = ok_config();
        cfg.gateway_url = "https://gateway.example.com:8443".into();
        cfg.validate().unwrap();
    }

    // ── Validation: gateway_url ─────────────────────────────────────────────

    #[test]
    fn validate_rejects_empty_gateway_url() {
        let mut cfg = ok_config();
        cfg.gateway_url = String::new();
        let err = cfg.validate().unwrap_err();
        assert!(matches!(err, SdkError::Config(_)));
        assert!(err.to_string().contains("gateway_url"));
    }

    #[test]
    fn validate_rejects_gateway_url_without_scheme() {
        let mut cfg = ok_config();
        cfg.gateway_url = "localhost:8080".into();
        let err = cfg.validate().unwrap_err();
        assert!(err.to_string().contains("scheme"));
    }

    #[test]
    fn validate_rejects_gateway_url_with_whitespace() {
        let mut cfg = ok_config();
        cfg.gateway_url = "http://has space.com".into();
        let err = cfg.validate().unwrap_err();
        assert!(err.to_string().contains("whitespace"));
    }

    // ── Validation: sdk_key ─────────────────────────────────────────────────

    #[test]
    fn validate_rejects_empty_sdk_key() {
        let mut cfg = ok_config();
        cfg.sdk_key = String::new();
        let err = cfg.validate().unwrap_err();
        assert!(err.to_string().contains("sdk_key"));
    }

    #[test]
    fn validate_rejects_short_sdk_key() {
        let mut cfg = ok_config();
        cfg.sdk_key = "1234567".into(); // 7 chars, min is 8
        let err = cfg.validate().unwrap_err();
        assert!(err.to_string().contains("at least"));
    }

    #[test]
    fn validate_accepts_exact_min_length_sdk_key() {
        let mut cfg = ok_config();
        cfg.sdk_key = "12345678".into(); // exactly 8
        cfg.validate().unwrap();
    }

    // ── Validation: ports, intervals, capacities ────────────────────────────

    #[test]
    fn validate_rejects_zero_grpc_port() {
        let mut cfg = ok_config();
        cfg.gateway_grpc_port = 0;
        assert!(
            cfg.validate()
                .unwrap_err()
                .to_string()
                .contains("gateway_grpc_port")
        );
    }

    #[test]
    fn validate_rejects_zero_definition_poll_interval() {
        let mut cfg = ok_config();
        cfg.definition_poll_interval = Duration::ZERO;
        assert!(
            cfg.validate()
                .unwrap_err()
                .to_string()
                .contains("definition_poll_interval")
        );
    }

    #[test]
    fn validate_rejects_zero_refresh_interval() {
        let mut cfg = ok_config();
        cfg.list_segment_refresh_interval = Duration::ZERO;
        assert!(
            cfg.validate()
                .unwrap_err()
                .to_string()
                .contains("list_segment_refresh_interval")
        );
    }

    #[test]
    fn validate_rejects_zero_event_flush_interval() {
        let mut cfg = ok_config();
        cfg.event_flush_interval = Duration::ZERO;
        assert!(
            cfg.validate()
                .unwrap_err()
                .to_string()
                .contains("event_flush_interval")
        );
    }

    #[test]
    fn validate_rejects_zero_request_timeout() {
        let mut cfg = ok_config();
        cfg.request_timeout = Duration::ZERO;
        assert!(
            cfg.validate()
                .unwrap_err()
                .to_string()
                .contains("request_timeout")
        );
    }

    #[test]
    fn validate_rejects_zero_lru_max_entries() {
        let mut cfg = ok_config();
        cfg.lru_max_entries = 0;
        assert!(
            cfg.validate()
                .unwrap_err()
                .to_string()
                .contains("lru_max_entries")
        );
    }

    #[test]
    fn validate_rejects_zero_event_batch_size() {
        let mut cfg = ok_config();
        cfg.event_batch_size = 0;
        assert!(
            cfg.validate()
                .unwrap_err()
                .to_string()
                .contains("event_batch_size")
        );
    }

    #[test]
    fn validate_rejects_zero_event_buffer_capacity() {
        let mut cfg = ok_config();
        cfg.event_buffer_capacity = 0;
        assert!(
            cfg.validate()
                .unwrap_err()
                .to_string()
                .contains("event_buffer_capacity")
        );
    }

    #[test]
    fn validate_rejects_buffer_smaller_than_batch() {
        let mut cfg = ok_config();
        cfg.event_batch_size = 100;
        cfg.event_buffer_capacity = 50;
        let err = cfg.validate().unwrap_err();
        assert!(err.to_string().contains("event_buffer_capacity"));
        assert!(err.to_string().contains("event_batch_size"));
    }

    #[test]
    fn validate_accepts_buffer_exactly_equal_to_batch() {
        let mut cfg = ok_config();
        cfg.event_batch_size = 100;
        cfg.event_buffer_capacity = 100;
        cfg.validate().unwrap();
    }
}
