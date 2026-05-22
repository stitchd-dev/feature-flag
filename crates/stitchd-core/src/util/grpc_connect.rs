//! Generic retry-with-exponential-backoff helper for service boot-time
//! gRPC connections.
//!
//! ## Why this exists
//!
//! When the dev/prod stack is brought up in parallel (e.g. `docker compose up`
//! or a `tilt` graph), a service may attempt to dial a dependency before that
//! dependency has bound its listening socket. `tonic` surfaces that race as a
//! `transport error` on the very first `connect().await` call and the caller
//! exits non-zero. We previously masked this by adding manual `sleep` calls
//! to the start scripts; this helper replaces that with disciplined,
//! observable retry-with-backoff.
//!
//! ## Behaviour
//!
//! The helper invokes a caller-supplied async `connect` closure up to
//! `max_attempts` times, doubling the delay between attempts and capping it
//! at `max_delay`. Each failure is logged at `info` (so an operator watching
//! `RUST_LOG=info` sees a clear "still waiting for X" trail) and the final
//! failure returns the *last* error verbatim so a genuine misconfiguration
//! (bad URI, TLS mismatch, etc.) still surfaces in the boot log instead of
//! being silently retried forever.
//!
//! Total wall-clock budget with the default `connect_with_retry_default`
//! parameters is roughly 60 seconds — long enough to absorb a slow Postgres
//! migration in front of the dependency, short enough that a real outage
//! still fails fast.

use std::fmt::Display;
use std::future::Future;
use std::time::Duration;

use tracing::{info, warn};

/// Default initial backoff between connection attempts.
pub const DEFAULT_INITIAL_DELAY: Duration = Duration::from_millis(200);

/// Default cap on the exponential backoff between connection attempts.
pub const DEFAULT_MAX_DELAY: Duration = Duration::from_secs(5);

/// Default number of attempts (~60s total wall-clock with the defaults above).
pub const DEFAULT_MAX_ATTEMPTS: u32 = 20;

/// Retry the async `connect` closure with exponential backoff.
///
/// `service_name` is purely for log lines (e.g. `"Analytics Service"`),
/// `addr` is logged alongside the service name so an operator can confirm
/// which target is being retried.
///
/// The delay schedule starts at `initial_delay`, doubles after each failure
/// and is capped at `max_delay`. After `max_attempts` consecutive failures
/// the last error is returned to the caller — the helper does **not** wrap
/// or annotate the error, so callers keep their existing `.context(...)`
/// attachments intact.
///
/// # Errors
/// Returns the last error from `connect` once `max_attempts` is exhausted.
///
/// # Panics
/// Panics if `max_attempts` is zero (a logic bug — callers must request at
/// least one attempt).
pub async fn connect_with_retry<F, Fut, C, E>(
    service_name: &str,
    addr: &str,
    max_attempts: u32,
    initial_delay: Duration,
    max_delay: Duration,
    mut connect: F,
) -> Result<C, E>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<C, E>>,
    E: Display,
{
    assert!(
        max_attempts > 0,
        "connect_with_retry: max_attempts must be > 0"
    );

    let mut delay = initial_delay;
    let mut last_err: Option<E> = None;

    for attempt in 1..=max_attempts {
        match connect().await {
            Ok(client) => {
                if attempt > 1 {
                    info!(
                        service = service_name,
                        addr = addr,
                        attempt,
                        "connected to dependency after retry"
                    );
                }
                return Ok(client);
            }
            Err(err) => {
                if attempt == max_attempts {
                    warn!(
                        service = service_name,
                        addr = addr,
                        attempt,
                        error = %err,
                        "giving up on dependency after exhausting retries"
                    );
                    last_err = Some(err);
                    break;
                }
                info!(
                    service = service_name,
                    addr = addr,
                    attempt,
                    max_attempts,
                    next_delay_ms = u64::try_from(delay.as_millis()).unwrap_or(u64::MAX),
                    error = %err,
                    "dependency not ready yet — backing off and retrying"
                );
                tokio::time::sleep(delay).await;
                delay = (delay * 2).min(max_delay);
            }
        }
    }

    // Safety: the loop above always assigns `last_err` before breaking on the
    // final failed attempt, and on success we already returned.
    Err(last_err.expect("connect_with_retry: loop exited without an error"))
}

/// Convenience wrapper around [`connect_with_retry`] using the workspace
/// defaults (20 attempts, 200ms → 5s exponential backoff, ~60s total
/// budget). Suitable for boot-time gRPC connects to peer microservices.
///
/// # Errors
/// Returns the last error from `connect` once the default attempt budget
/// is exhausted.
pub async fn connect_with_retry_default<F, Fut, C, E>(
    service_name: &str,
    addr: &str,
    connect: F,
) -> Result<C, E>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<C, E>>,
    E: Display,
{
    connect_with_retry(
        service_name,
        addr,
        DEFAULT_MAX_ATTEMPTS,
        DEFAULT_INITIAL_DELAY,
        DEFAULT_MAX_DELAY,
        connect,
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Arc;

    #[derive(Debug)]
    struct TestError(&'static str);

    impl Display for TestError {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str(self.0)
        }
    }

    /// The happy-path: the connect closure succeeds on the very first call,
    /// so no sleep is incurred and the helper returns immediately.
    #[tokio::test(start_paused = true)]
    async fn returns_immediately_when_first_attempt_succeeds() {
        let attempts = Arc::new(AtomicU32::new(0));
        let attempts_c = attempts.clone();

        let result: Result<u32, TestError> = connect_with_retry(
            "test-svc",
            "http://localhost:9999",
            5,
            Duration::from_millis(10),
            Duration::from_millis(100),
            move || {
                let attempts = attempts_c.clone();
                async move {
                    attempts.fetch_add(1, Ordering::SeqCst);
                    Ok(42)
                }
            },
        )
        .await;

        assert_eq!(result.expect("connect_with_retry should succeed"), 42);
        assert_eq!(
            attempts.load(Ordering::SeqCst),
            1,
            "should call connect exactly once on first-try success"
        );
    }

    /// The race-recovery path: connect fails twice (mirroring the real bug
    /// where analytics-service hasn't bound its socket yet) and then
    /// succeeds. The helper must return Ok with the eventual value.
    #[tokio::test(start_paused = true)]
    async fn retries_until_success_on_third_attempt() {
        let attempts = Arc::new(AtomicU32::new(0));
        let attempts_c = attempts.clone();

        let result: Result<&'static str, TestError> = connect_with_retry(
            "flaky-svc",
            "http://localhost:9999",
            5,
            Duration::from_millis(10),
            Duration::from_millis(100),
            move || {
                let attempts = attempts_c.clone();
                async move {
                    let n = attempts.fetch_add(1, Ordering::SeqCst) + 1;
                    if n < 3 {
                        Err(TestError("transport error"))
                    } else {
                        Ok("connected")
                    }
                }
            },
        )
        .await;

        assert_eq!(result.expect("retry should eventually succeed"), "connected");
        assert_eq!(
            attempts.load(Ordering::SeqCst),
            3,
            "should retry until the third attempt succeeds"
        );
    }

    /// The permanent-failure path: a misconfigured URL or a dependency that
    /// will never come up should still surface as an error after the budget
    /// is exhausted — we must *not* silently mask real outages.
    #[tokio::test(start_paused = true)]
    async fn returns_last_error_after_exhausting_attempts() {
        let attempts = Arc::new(AtomicU32::new(0));
        let attempts_c = attempts.clone();

        let result: Result<(), TestError> = connect_with_retry(
            "broken-svc",
            "http://localhost:9999",
            3,
            Duration::from_millis(10),
            Duration::from_millis(100),
            move || {
                let attempts = attempts_c.clone();
                async move {
                    let n = attempts.fetch_add(1, Ordering::SeqCst) + 1;
                    Err(TestError(if n == 3 { "final boom" } else { "boom" }))
                }
            },
        )
        .await;

        let err = result.expect_err("should fail after exhausting retries");
        assert_eq!(err.0, "final boom", "should return the last error verbatim");
        assert_eq!(
            attempts.load(Ordering::SeqCst),
            3,
            "should call connect exactly max_attempts times"
        );
    }

    /// Exponential backoff is capped: once the computed delay would exceed
    /// `max_delay`, further attempts wait at most `max_delay`. We assert
    /// this by checking total elapsed virtual time stays within an upper
    /// bound that requires the cap to kick in.
    #[tokio::test(start_paused = true)]
    async fn caps_backoff_at_max_delay() {
        let attempts = Arc::new(AtomicU32::new(0));
        let attempts_c = attempts.clone();

        let start = tokio::time::Instant::now();
        let result: Result<(), TestError> = connect_with_retry(
            "slow-svc",
            "http://localhost:9999",
            6,
            Duration::from_millis(100),
            Duration::from_millis(250),
            move || {
                let attempts = attempts_c.clone();
                async move {
                    attempts.fetch_add(1, Ordering::SeqCst);
                    Err(TestError("nope"))
                }
            },
        )
        .await;

        assert!(result.is_err());
        let elapsed = start.elapsed();

        // Without the cap the delay sequence would be 100, 200, 400, 800,
        // 1600 ms (sum = 3100 ms). With the 250 ms cap it is
        // 100, 200, 250, 250, 250 ms (sum = 1050 ms). Allow 50 ms slack
        // for the auto-advancing test clock.
        assert!(
            elapsed <= Duration::from_millis(1100),
            "expected cap to keep elapsed ≤ 1100 ms, got {:?}",
            elapsed
        );
        assert_eq!(attempts.load(Ordering::SeqCst), 6);
    }

    #[tokio::test(start_paused = true)]
    #[should_panic(expected = "max_attempts must be > 0")]
    async fn panics_on_zero_max_attempts() {
        let _: Result<(), TestError> = connect_with_retry(
            "test-svc",
            "http://localhost:9999",
            0,
            Duration::from_millis(10),
            Duration::from_millis(100),
            || async { Ok(()) },
        )
        .await;
    }

    /// `connect_with_retry_default` is the convenience entry point used by
    /// every service's main.rs. Verify it forwards through to the generic
    /// implementation and applies the documented defaults.
    #[tokio::test(start_paused = true)]
    async fn default_wrapper_uses_documented_attempt_budget() {
        let attempts = Arc::new(AtomicU32::new(0));
        let attempts_c = attempts.clone();

        let result: Result<(), TestError> = connect_with_retry_default(
            "test-svc",
            "http://localhost:9999",
            move || {
                let attempts = attempts_c.clone();
                async move {
                    attempts.fetch_add(1, Ordering::SeqCst);
                    Err(TestError("always fails"))
                }
            },
        )
        .await;

        assert!(result.is_err());
        assert_eq!(
            attempts.load(Ordering::SeqCst),
            DEFAULT_MAX_ATTEMPTS,
            "default wrapper should attempt exactly DEFAULT_MAX_ATTEMPTS times"
        );
    }
}
