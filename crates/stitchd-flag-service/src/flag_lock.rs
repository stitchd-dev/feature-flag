//! Flag-lock derivation helper.
//!
//! The flag-lock model replaced the former per-rule `frozen` boolean (dropped
//! in PROP-001, domain_boundaries_20260530) with a
//! **whole-flag** lock derived from "is there an experiment in
//! `running` or `paused` status on this flag?". When such an experiment
//! exists, every admin mutation on the flag (variants, rules, default-rule
//! distribution, enable/disable, archive) must return HTTP 409
//! `FLAG_LOCKED_BY_EXPERIMENT` so the operator stops the experiment first.
//!
//! This module owns:
//! - [`is_flag_locked`] — the read-through derivation helper used by every
//!   flag mutation path.
//! - [`FlagLockCache`] — a small `moka::future::Cache` keyed on `FlagId`
//!   with a 30s TTL. The cache lives in `FlagServiceImpl` and is
//!   `invalidate`d on experiment lifecycle transitions to keep the lock
//!   state fresh without hammering Postgres on every PATCH.
//!
//! See the experimentation-full track spec §2 "Whole-flag freeze" for the
//! end-to-end invariant this helper enforces.

use std::sync::Arc;
use std::time::Duration;

use moka::future::Cache;
use stitchd_core::id::{ExperimentId, FlagId};
use stitchd_db::repository::ExperimentRepository;
use tonic::Status;

/// 30-second TTL on the per-flag lock-state cache. Lock state may stay
/// stale for at most this long after an experiment transition that
/// forgets to invalidate. The TTL caps the staleness window so a missed
/// invalidation is self-healing — a paused/stopped experiment cannot
/// lock the flag forever.
const FLAG_LOCK_TTL: Duration = Duration::from_secs(30);

/// In-process cache of "this flag is locked by experiment X (or not)".
///
/// Keyed on [`FlagId`]; entries expire after [`FLAG_LOCK_TTL`]. Concurrent
/// callers for the same flag coalesce through `moka`'s
/// `get_with` semantics.
#[derive(Clone, Debug)]
pub struct FlagLockCache {
    inner: Cache<FlagId, Option<ExperimentId>>,
}

impl FlagLockCache {
    /// Construct a fresh cache with the default 30-second TTL.
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: Cache::builder()
                .time_to_live(FLAG_LOCK_TTL)
                .max_capacity(10_000)
                .build(),
        }
    }

    /// Invalidate the cached lock state for a single flag.
    ///
    /// Called by the experimentation-service after every `apply_transition`
    /// (running ↔ paused ↔ stopped) so the next mutation guard sees the
    /// up-to-date lock state immediately rather than after the TTL elapses.
    pub async fn invalidate(&self, flag_id: FlagId) {
        self.inner.invalidate(&flag_id).await;
    }

    /// Direct read of the cached entry (test-only convenience).
    ///
    /// Returns `Some(value)` when the entry is in cache, else `None`.
    /// Avoids touching the loader.
    #[cfg(test)]
    pub(crate) async fn peek(&self, flag_id: FlagId) -> Option<Option<ExperimentId>> {
        self.inner.get(&flag_id).await
    }
}

impl Default for FlagLockCache {
    fn default() -> Self {
        Self::new()
    }
}

/// Returns `Some(experiment_id)` when an experiment with `status IN
/// ('running','paused')` is currently bound to `flag_id`, else `None`.
///
/// Reads through [`FlagLockCache`] on hit; falls through to the
/// experiment repository (`find_active_experiment_for_flag`) on miss and
/// populates the cache. The per-flag uniqueness index guarantees at most
/// one such experiment, so returning a single ID is sound.
///
/// # Errors
/// Returns a `tonic::Status::internal` when the underlying repository
/// lookup fails.
pub async fn is_flag_locked(
    repo: &dyn ExperimentRepository,
    cache: &FlagLockCache,
    flag_id: FlagId,
) -> Result<Option<ExperimentId>, Status> {
    // moka's `try_get_with` coalesces concurrent loads for the same key.
    let result: Result<Option<ExperimentId>, Arc<stitchd_db::RepositoryError>> = cache
        .inner
        .try_get_with(flag_id, async {
            repo.find_active_experiment_for_flag(flag_id).await
        })
        .await;

    result.map_err(|e| Status::internal(format!("flag lock lookup failed: {e}")))
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::atomic::{AtomicUsize, Ordering};

    use async_trait::async_trait;
    use stitchd_core::experimentation::{Experiment, ExperimentIteration, ExperimentStatus};
    use stitchd_core::id::{EnvironmentId, ExperimentIterationId, MetricId, RuleId, UserId};
    use stitchd_db::RepositoryError;
    use stitchd_db::repository::ExperimentRepository;

    // ── Test-only mock ExperimentRepository ───────────────────────────────────
    //
    // Returns a pre-configured "active experiment" for any flag and counts
    // how many times `find_active_experiment_for_flag` was actually invoked
    // so we can assert cache hits.
    struct MockExperimentRepo {
        active: Option<ExperimentId>,
        calls: AtomicUsize,
    }

    impl MockExperimentRepo {
        fn new(active: Option<ExperimentId>) -> Self {
            Self {
                active,
                calls: AtomicUsize::new(0),
            }
        }

        fn call_count(&self) -> usize {
            self.calls.load(Ordering::SeqCst)
        }
    }

    #[async_trait]
    impl ExperimentRepository for MockExperimentRepo {
        async fn find_by_id(&self, id: ExperimentId) -> Result<Experiment, RepositoryError> {
            Err(RepositoryError::NotFound { id: id.to_string() })
        }
        async fn list_by_environment(
            &self,
            _env_id: EnvironmentId,
            _status_filter: Option<ExperimentStatus>,
        ) -> Result<Vec<Experiment>, RepositoryError> {
            Ok(vec![])
        }
        async fn list_by_environment_keyset(
            &self,
            _env_id: EnvironmentId,
            _after: Option<stitchd_db::KeysetCursor>,
            _limit: u64,
        ) -> Result<(Vec<Experiment>, Option<String>), RepositoryError> {
            Ok((vec![], None))
        }
        async fn create(&self, _experiment: &Experiment) -> Result<(), RepositoryError> {
            Ok(())
        }
        async fn update(&self, _experiment: &Experiment) -> Result<Experiment, RepositoryError> {
            unimplemented!()
        }
        async fn soft_delete(&self, _id: ExperimentId) -> Result<(), RepositoryError> {
            Ok(())
        }
        async fn list_iterations(
            &self,
            _experiment_id: ExperimentId,
        ) -> Result<Vec<ExperimentIteration>, RepositoryError> {
            Ok(vec![])
        }
        async fn apply_transition(
            &self,
            id: ExperimentId,
            to: ExperimentStatus,
            _actor_id: Option<UserId>,
        ) -> Result<Experiment, RepositoryError> {
            let _ = (id, to);
            unimplemented!()
        }
        async fn list_all_running(&self) -> Result<Vec<Experiment>, RepositoryError> {
            Ok(vec![])
        }
        async fn find_active_experiment_for_flag(
            &self,
            _flag_id: FlagId,
        ) -> Result<Option<ExperimentId>, RepositoryError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(self.active)
        }
        async fn find_iteration_by_id(
            &self,
            iteration_id: ExperimentIterationId,
        ) -> Result<ExperimentIteration, RepositoryError> {
            Err(RepositoryError::NotFound {
                id: iteration_id.to_string(),
            })
        }
    }

    // Silence unused-import warnings inside test scope.
    #[allow(dead_code)]
    fn _force_imports() {
        let _ = MetricId::new();
        let _ = RuleId::new();
    }

    #[tokio::test]
    async fn returns_some_for_running_experiment() {
        let exp_id = ExperimentId::new();
        let repo = MockExperimentRepo::new(Some(exp_id));
        let cache = FlagLockCache::new();
        let flag_id = FlagId::new();

        let got = is_flag_locked(&repo, &cache, flag_id).await.unwrap();
        assert_eq!(got, Some(exp_id));
    }

    #[tokio::test]
    async fn returns_some_for_paused_experiment() {
        // The repository is responsible for the running/paused filter — from
        // the helper's perspective both states surface as `Some(exp_id)`.
        let exp_id = ExperimentId::new();
        let repo = MockExperimentRepo::new(Some(exp_id));
        let cache = FlagLockCache::new();
        let flag_id = FlagId::new();

        let got = is_flag_locked(&repo, &cache, flag_id).await.unwrap();
        assert_eq!(got, Some(exp_id));
    }

    #[tokio::test]
    async fn returns_none_for_draft_stopped_or_deleted() {
        // The repository excludes draft/stopped/deleted rows via its `WHERE
        // status IN ('running','paused') AND deleted_at IS NULL` clause — so
        // from the helper's perspective those cases collapse into `None`.
        let repo = MockExperimentRepo::new(None);
        let cache = FlagLockCache::new();
        let flag_id = FlagId::new();

        let got = is_flag_locked(&repo, &cache, flag_id).await.unwrap();
        assert_eq!(got, None);
    }

    #[tokio::test]
    async fn second_call_within_ttl_hits_cache() {
        let exp_id = ExperimentId::new();
        let repo = MockExperimentRepo::new(Some(exp_id));
        let cache = FlagLockCache::new();
        let flag_id = FlagId::new();

        let _ = is_flag_locked(&repo, &cache, flag_id).await.unwrap();
        let _ = is_flag_locked(&repo, &cache, flag_id).await.unwrap();
        let _ = is_flag_locked(&repo, &cache, flag_id).await.unwrap();

        // moka may briefly count concurrent loads; we asserted at most one
        // round-trip to the repo within the TTL.
        assert_eq!(
            repo.call_count(),
            1,
            "expected single repo lookup within TTL"
        );
    }

    #[tokio::test]
    async fn invalidate_clears_cache_entry() {
        let exp_id = ExperimentId::new();
        let repo = MockExperimentRepo::new(Some(exp_id));
        let cache = FlagLockCache::new();
        let flag_id = FlagId::new();

        // Prime the cache.
        let _ = is_flag_locked(&repo, &cache, flag_id).await.unwrap();
        assert_eq!(repo.call_count(), 1);

        // Invalidate; next call must hit the repo again.
        cache.invalidate(flag_id).await;
        let _ = is_flag_locked(&repo, &cache, flag_id).await.unwrap();
        assert_eq!(
            repo.call_count(),
            2,
            "expected second repo lookup after invalidate"
        );
    }

    #[tokio::test]
    async fn cache_default_eq_new() {
        let c = FlagLockCache::default();
        // peek on a fresh cache returns None — nothing populated yet.
        assert!(c.peek(FlagId::new()).await.is_none());
    }
}
