//! ClickHouse dictionary refresh hook for `experiment_iterations_active`.
//!
//! On every experiment-status transition that could change the dictionary's
//! contents (`draft → running`, `running ↔ paused`, `running|paused →
//! stopped`, `stopped → running`), we fire-and-forget a
//! `SYSTEM RELOAD DICTIONARY experiment_iterations_active` against CH so the
//! attribution MV picks up the change immediately. The CH dictionary's
//! `LIFETIME(MIN 30 MAX 60)` + `invalidate_query` provide a fallback if this
//! call fails — the staleness window is bounded at 30s either way.
//!
//! Abstracted behind the [`DictionaryRefresher`] trait so unit tests can
//! observe invocations without standing up a real CH client.

use async_trait::async_trait;
use std::sync::Arc;

/// Trait for issuing a `SYSTEM RELOAD DICTIONARY` against ClickHouse.
///
/// `reload_experiment_iterations_active` is fire-and-forget — callers in the
/// service must NOT await its result on the request hot path. Errors are
/// logged via `tracing::warn!` inside the spawn so they don't propagate up.
#[async_trait]
pub trait DictionaryRefresher: Send + Sync + 'static {
    /// Issue `SYSTEM RELOAD DICTIONARY experiment_iterations_active`.
    async fn reload_experiment_iterations_active(&self) -> Result<(), clickhouse::error::Error>;
}

/// Production implementation backed by a `clickhouse::Client`.
pub struct ClickHouseDictionaryRefresher {
    client: Arc<clickhouse::Client>,
}

impl ClickHouseDictionaryRefresher {
    /// Wrap a shared CH client.
    #[must_use]
    pub fn new(client: Arc<clickhouse::Client>) -> Self {
        Self { client }
    }
}

#[async_trait]
impl DictionaryRefresher for ClickHouseDictionaryRefresher {
    async fn reload_experiment_iterations_active(&self) -> Result<(), clickhouse::error::Error> {
        self.client
            .query("SYSTEM RELOAD DICTIONARY experiment_iterations_active")
            .execute()
            .await
    }
}

/// Spawn a fire-and-forget dictionary refresh.
///
/// Logs `tracing::warn!` on error; never panics, never blocks the caller.
pub fn spawn_refresh(refresher: Arc<dyn DictionaryRefresher>) {
    tokio::spawn(async move {
        if let Err(err) = refresher.reload_experiment_iterations_active().await {
            tracing::warn!(
                error = %err,
                "SYSTEM RELOAD DICTIONARY experiment_iterations_active failed; falling back to LIFETIME refresh"
            );
        } else {
            tracing::debug!("experiment_iterations_active dictionary reloaded after transition");
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct CountingRefresher {
        count: Arc<AtomicUsize>,
        fail: bool,
    }

    #[async_trait]
    impl DictionaryRefresher for CountingRefresher {
        async fn reload_experiment_iterations_active(
            &self,
        ) -> Result<(), clickhouse::error::Error> {
            self.count.fetch_add(1, Ordering::SeqCst);
            if self.fail {
                Err(clickhouse::error::Error::Custom("test failure".into()))
            } else {
                Ok(())
            }
        }
    }

    #[tokio::test]
    async fn spawn_refresh_invokes_trait() {
        let count = Arc::new(AtomicUsize::new(0));
        let refresher: Arc<dyn DictionaryRefresher> = Arc::new(CountingRefresher {
            count: count.clone(),
            fail: false,
        });
        spawn_refresh(refresher);

        // Yield to the spawned task — small loop so we don't race on slow CI.
        for _ in 0..20 {
            if count.load(Ordering::SeqCst) > 0 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        assert_eq!(count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn spawn_refresh_swallows_errors() {
        let count = Arc::new(AtomicUsize::new(0));
        let refresher: Arc<dyn DictionaryRefresher> = Arc::new(CountingRefresher {
            count: count.clone(),
            fail: true,
        });
        spawn_refresh(refresher);

        for _ in 0..20 {
            if count.load(Ordering::SeqCst) > 0 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        assert_eq!(count.load(Ordering::SeqCst), 1);
        // Spawn returns immediately and the test does not panic — assertion above is enough.
    }
}
