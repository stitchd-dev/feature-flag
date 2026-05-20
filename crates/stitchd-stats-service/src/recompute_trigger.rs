//! Event-driven recompute trigger for stats-service.
//!
//! When a metric definition is updated (`PATCH /v1/metrics/{id}` →
//! `update_metric` gRPC), we want to refresh all running experiments
//! that reference that metric — otherwise their cached stats reflect
//! the *old* metric definition until the next scheduled run.
//!
//! [`trigger_recompute_for_metric`] is a pure async function that:
//! 1. Lists all running experiments in the environment.
//! 2. Filters those whose `metric_ids` contains the given metric ID.
//!    (Phase 7 cutover replaced the previous `metric_keys` filter — we
//!    now match on UUIDs end-to-end.)
//! 3. Fires a `TriggerRecompute` request per experiment via the
//!    supplied [`RecomputeTrigger`] implementation.
//!
//! Errors are logged via `tracing::warn!` but **never propagated to
//! the caller** — the function returns a count of successful triggers.
//! This matches the "Fire-and-forget gRPC for analytics" pattern
//! (`conductor/patterns.md`): a metric update RPC must not be
//! blocked by — or fail because of — a recompute side effect.
//!
//! # Why a trait, not a `StatsServiceClient` directly?
//! The real production wiring uses
//! `stitchd_proto::stats::v1::stats_service_client::StatsServiceClient`
//! (a tonic-generated client). But mocking a tonic client in unit
//! tests requires spinning up a real channel. By gating on the
//! [`RecomputeTrigger`] trait we keep the orchestration logic
//! testable with a hand-rolled in-memory mock — no `mockall`, no
//! tokio runtime juggling beyond the basics.

use async_trait::async_trait;
use stitchd_core::experimentation::{Experiment, ExperimentStatus};
use stitchd_core::id::{EnvironmentId, ExperimentId, MetricId};
use stitchd_db::{ExperimentRepository, RepositoryError};

/// Error type for the recompute trigger flow.
///
/// All variants are *logged* in the fire-and-forget path; we only
/// surface them in tests / library callers that want the detail.
#[derive(Debug, thiserror::Error)]
pub enum RecomputeError {
    /// Listing running experiments from the repo failed.
    #[error("failed to list running experiments: {0}")]
    Repository(#[from] RepositoryError),
}

/// Abstraction over the `TriggerRecompute` RPC.
///
/// The production impl ([`GrpcRecomputeTrigger`]) wraps a real
/// `StatsServiceClient<Channel>`. Tests use [`CapturedTrigger`]
/// (defined in this module's `#[cfg(test)]` block) to record calls.
#[async_trait]
pub trait RecomputeTrigger: Send + Sync {
    /// Fire-and-forget request to trigger a recompute for `experiment_id`.
    ///
    /// Errors should be reported as `Err(String)` and will be logged
    /// at WARN — they do **not** abort the dispatch loop. The next
    /// experiment in the list will still be triggered.
    async fn trigger(&self, experiment_id: ExperimentId) -> Result<(), String>;
}

/// Find all running experiments in `env_id` that reference `metric_id`
/// and fire a `TriggerRecompute` RPC for each.
///
/// Returns the number of triggers that **succeeded**. Failures are
/// logged via `tracing::warn!` but do not stop the loop.
pub async fn trigger_recompute_for_metric(
    trigger: &dyn RecomputeTrigger,
    experiment_repo: &dyn ExperimentRepository,
    metric_id: MetricId,
    env_id: EnvironmentId,
) -> Result<usize, RecomputeError> {
    let experiments = experiment_repo
        .list_by_environment(env_id, Some(ExperimentStatus::Running))
        .await?;

    let mut fired = 0usize;
    for exp in experiments.iter().filter(|e| references_metric(e, metric_id)) {
        match trigger.trigger(exp.id).await {
            Ok(()) => fired += 1,
            Err(e) => {
                tracing::warn!(
                    experiment_id = %exp.id,
                    metric_id = %metric_id,
                    "trigger_recompute_for_metric: TriggerRecompute RPC failed: {e}",
                );
            }
        }
    }

    Ok(fired)
}

fn references_metric(exp: &Experiment, metric_id: MetricId) -> bool {
    exp.metric_ids.contains(&metric_id)
}

// ---------------------------------------------------------------------------
// Production impl: real gRPC client over tonic
// ---------------------------------------------------------------------------

/// Production [`RecomputeTrigger`] backed by the tonic-generated
/// `StatsServiceClient`. Each `trigger` call clones the underlying
/// `Channel` (cheap — connection pool is shared) and issues one RPC.
pub struct GrpcRecomputeTrigger {
    client: stitchd_proto::stats::v1::stats_service_client::StatsServiceClient<
        tonic::transport::Channel,
    >,
}

impl GrpcRecomputeTrigger {
    #[must_use]
    pub fn new(
        client: stitchd_proto::stats::v1::stats_service_client::StatsServiceClient<
            tonic::transport::Channel,
        >,
    ) -> Self {
        Self { client }
    }
}

#[async_trait]
impl RecomputeTrigger for GrpcRecomputeTrigger {
    async fn trigger(&self, experiment_id: ExperimentId) -> Result<(), String> {
        let mut client = self.client.clone();
        let req = stitchd_proto::stats::v1::TriggerRecomputeRequest {
            experiment_id: experiment_id.to_string(),
        };
        client
            .trigger_recompute(req)
            .await
            .map(|_| ())
            .map_err(|e| format!("{e}"))
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use chrono::Utc;
    use std::sync::Mutex;
    use stitchd_core::experimentation::{Experiment, ExperimentIteration, ExperimentStatus};
    use stitchd_core::id::{EnvironmentId, ExperimentId, MetricId, RuleId};
    use stitchd_db::{ExperimentRepository, RepositoryError};

    // ---------------------------------------------------------------
    // Hand-rolled mocks (no mockall)
    // ---------------------------------------------------------------

    /// Capturing trigger — records every experiment_id passed to
    /// `trigger()`. Lets the test assert on call count + order.
    #[derive(Default)]
    struct CapturedTrigger {
        calls: Mutex<Vec<ExperimentId>>,
        /// If set, every call returns `Err(this)` instead of `Ok(())`.
        force_error: Option<String>,
    }

    impl CapturedTrigger {
        fn calls(&self) -> Vec<ExperimentId> {
            self.calls.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl RecomputeTrigger for CapturedTrigger {
        async fn trigger(&self, experiment_id: ExperimentId) -> Result<(), String> {
            self.calls.lock().unwrap().push(experiment_id);
            match &self.force_error {
                Some(msg) => Err(msg.clone()),
                None => Ok(()),
            }
        }
    }

    /// In-memory ExperimentRepository — only `list_by_environment` is
    /// real; everything else panics (unused in these tests).
    struct InMemoryExperimentRepo {
        rows: Vec<Experiment>,
        fail_with: Option<RepositoryError>,
    }

    impl InMemoryExperimentRepo {
        fn new(rows: Vec<Experiment>) -> Self {
            Self {
                rows,
                fail_with: None,
            }
        }
    }

    #[async_trait]
    impl ExperimentRepository for InMemoryExperimentRepo {
        async fn find_by_id(
            &self,
            _id: ExperimentId,
        ) -> Result<Experiment, RepositoryError> {
            unimplemented!("not used in recompute_trigger tests")
        }
        async fn list_by_environment(
            &self,
            env_id: EnvironmentId,
            status_filter: Option<ExperimentStatus>,
        ) -> Result<Vec<Experiment>, RepositoryError> {
            if let Some(e) = &self.fail_with {
                // RepositoryError isn't Clone — synthesize a fresh
                // Database error matching the simplest case the
                // test triggers.
                return Err(match e {
                    RepositoryError::NotFound { id } => {
                        RepositoryError::NotFound { id: id.clone() }
                    }
                    _ => RepositoryError::Unexpected(anyhow::anyhow!("forced")),
                });
            }
            Ok(self
                .rows
                .iter()
                .filter(|r| r.environment_id == env_id)
                .filter(|r| status_filter.is_none_or(|s| r.status == s))
                .cloned()
                .collect())
        }
        async fn list_by_environment_paginated(
            &self,
            _env_id: EnvironmentId,
            _offset: u64,
            _limit: u64,
        ) -> Result<(Vec<Experiment>, u64), RepositoryError> {
            unimplemented!()
        }
        async fn create(&self, _experiment: &Experiment) -> Result<(), RepositoryError> {
            unimplemented!()
        }
        async fn update(
            &self,
            _experiment: &Experiment,
        ) -> Result<Experiment, RepositoryError> {
            unimplemented!()
        }
        async fn soft_delete(&self, _id: ExperimentId) -> Result<(), RepositoryError> {
            unimplemented!()
        }
        async fn list_iterations(
            &self,
            _experiment_id: ExperimentId,
        ) -> Result<Vec<ExperimentIteration>, RepositoryError> {
            unimplemented!()
        }
        async fn apply_transition(
            &self,
            _id: ExperimentId,
            _to: ExperimentStatus,
            _actor_id: Option<stitchd_core::id::UserId>,
        ) -> Result<Experiment, RepositoryError> {
            unimplemented!()
        }
        async fn list_all_running(&self) -> Result<Vec<Experiment>, RepositoryError> {
            unimplemented!()
        }
        async fn find_iteration_by_id(
            &self,
            _iteration_id: stitchd_db::ExperimentIterationId,
        ) -> Result<ExperimentIteration, RepositoryError> {
            unimplemented!()
        }
    }

    fn mk_exp(
        env_id: EnvironmentId,
        status: ExperimentStatus,
        metric_ids: Vec<MetricId>,
    ) -> Experiment {
        Experiment {
            id: ExperimentId::new(),
            environment_id: env_id,
            flag_rule_id: RuleId::new(),
            name: "exp".into(),
            description: None,
            hypothesis: None,
            metric_ids,
            traffic_allocation: 100.0,
            min_sample_size: None,
            scheduled_start_at: None,
            scheduled_end_at: None,
            status,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            deleted_at: None,
            version: 1,
        }
    }

    // ---------------------------------------------------------------
    // Behaviour tests
    // ---------------------------------------------------------------

    #[tokio::test]
    async fn test_trigger_recompute_finds_experiments_by_metric_id() {
        let env = EnvironmentId::new();
        let target = MetricId::new();
        let signup = MetricId::new();
        let unrelated = MetricId::new();
        let exp1 = mk_exp(env, ExperimentStatus::Running, vec![target]);
        let exp2 = mk_exp(env, ExperimentStatus::Running, vec![target, signup]);
        let exp3 = mk_exp(env, ExperimentStatus::Running, vec![unrelated]);
        let repo = InMemoryExperimentRepo::new(vec![exp1.clone(), exp2.clone(), exp3]);
        let trigger = CapturedTrigger::default();

        let n = trigger_recompute_for_metric(&trigger, &repo, target, env)
            .await
            .unwrap();
        assert_eq!(n, 2);

        let got = trigger.calls();
        assert!(got.contains(&exp1.id));
        assert!(got.contains(&exp2.id));
    }

    #[tokio::test]
    async fn test_trigger_recompute_skips_completed_experiments() {
        let env = EnvironmentId::new();
        let rev = MetricId::new();
        let live = mk_exp(env, ExperimentStatus::Running, vec![rev]);
        let paused = mk_exp(env, ExperimentStatus::Paused, vec![rev]);
        let stopped = mk_exp(env, ExperimentStatus::Stopped, vec![rev]);
        let draft = mk_exp(env, ExperimentStatus::Draft, vec![rev]);
        let repo = InMemoryExperimentRepo::new(vec![live.clone(), paused, stopped, draft]);
        let trigger = CapturedTrigger::default();

        let n = trigger_recompute_for_metric(&trigger, &repo, rev, env)
            .await
            .unwrap();
        assert_eq!(n, 1, "only Running experiments should trigger");
        assert_eq!(trigger.calls(), vec![live.id]);
    }

    #[tokio::test]
    async fn test_trigger_recompute_no_matching_experiments_returns_zero() {
        let env = EnvironmentId::new();
        let other_env = EnvironmentId::new();
        let foo = MetricId::new();
        let bar = MetricId::new();
        let unrelated = mk_exp(env, ExperimentStatus::Running, vec![foo]);
        let wrong_env = mk_exp(other_env, ExperimentStatus::Running, vec![bar]);
        let repo = InMemoryExperimentRepo::new(vec![unrelated, wrong_env]);
        let trigger = CapturedTrigger::default();

        let n = trigger_recompute_for_metric(&trigger, &repo, bar, env)
            .await
            .unwrap();
        assert_eq!(n, 0);
        assert!(trigger.calls().is_empty());
    }

    #[tokio::test]
    async fn test_trigger_recompute_logs_and_continues_on_rpc_failure() {
        // Fire-and-forget: if the RPC fails for one experiment, the
        // function still attempts the rest. Failed ones don't count
        // toward the success total.
        let env = EnvironmentId::new();
        let m = MetricId::new();
        let exp1 = mk_exp(env, ExperimentStatus::Running, vec![m]);
        let exp2 = mk_exp(env, ExperimentStatus::Running, vec![m]);
        let repo = InMemoryExperimentRepo::new(vec![exp1.clone(), exp2.clone()]);
        let trigger = CapturedTrigger {
            calls: Mutex::new(Vec::new()),
            force_error: Some("simulated transport error".into()),
        };

        let n = trigger_recompute_for_metric(&trigger, &repo, m, env)
            .await
            .unwrap();
        assert_eq!(n, 0, "all triggers failed → success count is 0");
        // But both were attempted — proves we don't short-circuit.
        assert_eq!(trigger.calls().len(), 2);
    }

    #[tokio::test]
    async fn test_trigger_recompute_surfaces_repo_error() {
        // Repo-level failures DO bubble up — this is the only failure
        // mode caller can observe. (The caller-side handler still
        // logs+swallows under tokio::spawn — see analytics handler.)
        let env = EnvironmentId::new();
        let mut repo = InMemoryExperimentRepo::new(vec![]);
        repo.fail_with = Some(RepositoryError::Unexpected(anyhow::anyhow!("db gone")));
        let trigger = CapturedTrigger::default();

        let err = trigger_recompute_for_metric(&trigger, &repo, MetricId::new(), env)
            .await
            .unwrap_err();
        assert!(matches!(err, RecomputeError::Repository(_)));
    }

    #[tokio::test]
    async fn test_references_metric_matches_exact_id_only() {
        let env = EnvironmentId::new();
        let target = MetricId::new();
        let other = MetricId::new();
        let exp = mk_exp(env, ExperimentStatus::Running, vec![target]);
        assert!(references_metric(&exp, target));
        assert!(!references_metric(&exp, other));
    }

    /// `mk_exp` should construct a valid Experiment used by the suite — a
    /// soft smoke test to guard against silent struct drift.
    #[test]
    fn test_mk_exp_helper_constructs_running_experiment() {
        let env = EnvironmentId::new();
        let a = MetricId::new();
        let b = MetricId::new();
        let e = mk_exp(env, ExperimentStatus::Running, vec![a, b]);
        assert_eq!(e.status, ExperimentStatus::Running);
        assert_eq!(e.metric_ids, vec![a, b]);
        assert_eq!(e.environment_id, env);
    }
}
