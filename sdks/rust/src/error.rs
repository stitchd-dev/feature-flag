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

/// Errors returned by [`crate::SdkClient::track`].
///
/// Note: `track()` is fire-and-forget per spec F2 — validation failures
/// (unknown event_key, value-type mismatch) DO NOT propagate as `Err`.
/// They warn + skip + return `Ok(())`. Only programming errors surface
/// here.
#[derive(Debug)]
pub enum TrackError {
    /// Caller invoked `track()` after `shutdown()`. This is a programming
    /// error — the client must not be used after it's been shut down.
    State(String),
}

impl fmt::Display for TrackError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::State(m) => write!(f, "track state error: {m}"),
        }
    }
}

impl std::error::Error for TrackError {}

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
}
