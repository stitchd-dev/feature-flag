//! Error taxonomy for the SDK.
//!
//! Mirrors the language-agnostic taxonomy in `sdks/spec/docs/06-errors.md`:
//!
//! - **Class A — Config:** invalid `SdkConfig` at init time (fail fast).
//! - **Class B — Auth:** SDK key rejected at init time. (Runtime auth
//!   failures stay internal — they NEVER surface to `evaluate()` callers.)
//! - **Class C — Network:** init-time connection failures. (Runtime network
//!   failures are handled via backoff inside the polling tasks.)
//! - **Class E — State:** programming errors (post-shutdown calls,
//!   double-init).
//!
//! Class D — Snapshot inconsistency — is per-evaluation and surfaces in the
//! `EvalResult.outcome` field, not through `SdkError`.

use std::fmt;

/// Public error type returned from `SdkClient::init` and other fallible
/// SDK methods. Implementations of other Stitchd SDKs (JS/Python/Go) must
/// expose these same four categories at minimum.
#[derive(Debug)]
pub enum SdkError {
    /// Class A — invalid configuration supplied to `init`.
    Config(String),
    /// Class B — SDK key invalid / revoked / unknown at init time.
    Auth(String),
    /// Class C — couldn't reach the gateway during first sync.
    Network(String),
    /// Class E — caller misuse (double-init, post-shutdown call).
    State(String),
}

impl fmt::Display for SdkError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Config(m) => write!(f, "sdk config error: {m}"),
            Self::Auth(m) => write!(f, "sdk auth error: {m}"),
            Self::Network(m) => write!(f, "sdk network error: {m}"),
            Self::State(m) => write!(f, "sdk state error: {m}"),
        }
    }
}

impl std::error::Error for SdkError {}

// ============================================================================
// TrackError — surface for `Client::track()` failures
// ============================================================================

/// Errors returned by [`crate::SdkClient::track`], [`crate::SdkClient::flush`],
/// and [`crate::SdkClient::shutdown`].
///
/// Note: `track()` is fire-and-forget per spec F2 — validation failures
/// (unknown event_key, value-type mismatch) DO NOT propagate as `Err`.
/// They warn + skip + return `Ok(())`. Only programming errors surface
/// here.
///
/// `flush()` and `shutdown()` (Phase 5 Task 5.3), in contrast, DO surface
/// transport failures via the [`TrackError::Network`] /
/// [`TrackError::Permanent`] variants so callers (typically integration
/// tests, batch jobs, graceful-shutdown handlers) can observe them.
#[derive(Debug)]
pub enum TrackError {
    /// Caller invoked `track()` after `shutdown()`. This is a programming
    /// error — the client must not be used after it's been shut down.
    State(String),
    /// Surfaced by `flush()` / `shutdown()` when network or 5xx retries
    /// were exhausted while delivering buffered events. `count` is the
    /// number of events that were dropped.
    Network { count: u32, last_error: String },
    /// Surfaced by `flush()` / `shutdown()` when the gateway permanently
    /// rejected a batch (4xx other than the retryable 408/429).
    Permanent { status: u16, message: String },
}

impl fmt::Display for TrackError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::State(m) => write!(f, "track state error: {m}"),
            Self::Network { count, last_error } => write!(
                f,
                "track network error: dropped {count} events ({last_error})"
            ),
            Self::Permanent { status, message } => {
                write!(f, "track permanent error: HTTP {status} — {message}")
            }
        }
    }
}

impl std::error::Error for TrackError {}

impl From<crate::event_buffer::FlushError> for TrackError {
    /// Map an internal [`crate::event_buffer::FlushError`] to the public
    /// [`TrackError`] surface so `Client::flush()` / `Client::shutdown()`
    /// can use the `?` operator on `EventBuffer::flush()`.
    fn from(e: crate::event_buffer::FlushError) -> Self {
        match e {
            crate::event_buffer::FlushError::Network { count, last_error } => {
                Self::Network { count, last_error }
            }
            crate::event_buffer::FlushError::Permanent { status, message } => {
                Self::Permanent { status, message }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_includes_class_prefix() {
        assert!(
            SdkError::Config("a".into())
                .to_string()
                .starts_with("sdk config error:")
        );
        assert!(
            SdkError::Auth("a".into())
                .to_string()
                .starts_with("sdk auth error:")
        );
        assert!(
            SdkError::Network("a".into())
                .to_string()
                .starts_with("sdk network error:")
        );
        assert!(
            SdkError::State("a".into())
                .to_string()
                .starts_with("sdk state error:")
        );
    }

    #[test]
    fn implements_std_error() {
        // Smoke check — the trait bound matters for callers using `?` and
        // boxed-error workflows.
        fn assert_error<E: std::error::Error>() {}
        assert_error::<SdkError>();
    }

    #[test]
    fn track_error_display_includes_class_prefix() {
        assert!(
            TrackError::State("shut down".into())
                .to_string()
                .starts_with("track state error:")
        );
    }

    #[test]
    fn track_error_implements_std_error() {
        fn assert_error<E: std::error::Error>() {}
        assert_error::<TrackError>();
    }

    #[test]
    fn test_track_error_from_flush_error() {
        // FlushError → TrackError must preserve count/last_error and
        // status/message exactly. `flush()` / `shutdown()` rely on `?`
        // round-tripping for the public API.
        use crate::event_buffer::FlushError;

        let net = FlushError::Network {
            count: 7,
            last_error: "connection reset".into(),
        };
        match TrackError::from(net) {
            TrackError::Network { count, last_error } => {
                assert_eq!(count, 7);
                assert_eq!(last_error, "connection reset");
            }
            other => panic!("expected TrackError::Network, got {other:?}"),
        }

        let perm = FlushError::Permanent {
            status: 400,
            message: "bad request".into(),
        };
        match TrackError::from(perm) {
            TrackError::Permanent { status, message } => {
                assert_eq!(status, 400);
                assert_eq!(message, "bad request");
            }
            other => panic!("expected TrackError::Permanent, got {other:?}"),
        }
    }
}
