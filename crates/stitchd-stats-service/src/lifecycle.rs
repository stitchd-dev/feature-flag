//! Autonomous bandit lifecycle executor (FR5).
//!
//! After the per-tick static reallocation (see [`crate::bandit`]), a bandit
//! experiment may be ready to *commit* to a winner or *roll out* and stop. This
//! module runs the pure convergence detector
//! ([`stitchd_core::experimentation::bandit::detect_convergence`]) over the
//! experiment's objective posteriors and applies the operator-selected
//! [`LifecyclePolicy`]:
//!
//! * **Advisory** (default) — record the converged winner + probability onto the
//!   experiment row (`bandit_converged_variant` / `bandit_converged_prob`) so the
//!   Phase-11 surfacing layer can raise a "ready to commit" badge. **No traffic
//!   change** beyond the normal reallocation.
//! * **AutoCommit** — write **100% to the winner** (a single-bucket
//!   [`RolloutDistribution`] of `{winner: 10000}`, all other arms omitted at 0bp)
//!   via the privileged [`AllocationApplier::apply`]; the flag stays locked (the
//!   experiment keeps running). Records `bandit_allocation_runs action='commit'`.
//!   Idempotent: once the experiment is committed, subsequent ticks no-op.
//! * **AutoRollout** — commit to the winner, then autonomously **stop** the
//!   experiment via [`LifecycleTransitioner::stop_experiment`]. Stopping clears
//!   the whole-flag lock (the flag-service derives lockedness from the active
//!   experiment, so a stopped experiment no longer holds it) and leaves the bound
//!   rule at 100%-winner — i.e. post-experiment traffic = winner. Records
//!   `action='rollout'`. Idempotent: a stopped/rolled-out experiment is never
//!   reprocessed (the scheduler only lists running experiments, and the commit
//!   sequence is ordered commit→stop so a crash mid-sequence re-runs cleanly).
//!
//! **Opt-in only:** Advisory never commits or rolls out; every autonomous traffic
//! change requires the operator's up-front `lifecycle_policy` choice.
//!
//! ## Idempotency
//!
//! The pure [`decide_lifecycle`] takes an `already_committed` flag (the bound rule
//! is already a single-bucket 100%-winner distribution). Once committed, AutoCommit
//! returns [`LifecycleAction::NoAction`] and AutoRollout returns
//! [`LifecycleAction::StopOnly`] (commit already done → just ensure it is stopped).
//! AutoRollout's stop is naturally idempotent (a no-longer-running experiment is
//! never listed again).
//!
//! ## Determinism
//!
//! Convergence detection seeds its Monte-Carlo from the same deterministic
//! [`crate::bandit::derive_seed`] the reallocation pass uses, so a given tick's
//! convergence verdict is reproducible.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde_json::Value;
use uuid::Uuid;

use stitchd_core::experimentation::bandit::{
    BanditConfig, ConvergedWinner, LifecyclePolicy, MetricRewards, detect_convergence, reward_arms,
};
use stitchd_core::metric::MetricDefinition;
use stitchd_core::rollout::{RolloutAllocation, RolloutDistribution};

use crate::bandit::{
    AllocationApplier, ApplyResult, RunRecorder, build_metric_rewards_pub, derive_seed,
    objective_metric_ids,
};
use crate::compute::CellReader;
use crate::scheduler::RunningExperiment;

// ── Pure decision ──────────────────────────────────────────────────────────────

/// The autonomous lifecycle action chosen for one tick of one experiment.
#[derive(Debug, Clone, PartialEq)]
pub enum LifecycleAction {
    /// Nothing to do: no convergence, or an already-committed AutoCommit, or an
    /// Advisory policy that has already recorded this state.
    NoAction,
    /// Record the converged winner for advisory surfacing (no traffic change).
    RecordAdvisory(ConvergedWinner),
    /// Commit 100% of traffic to the winner (flag stays locked / running).
    Commit(ConvergedWinner),
    /// Commit 100% to the winner AND stop the experiment (releasing the lock).
    Rollout(ConvergedWinner),
    /// The commit already happened; only the stop remains (idempotent rollout).
    StopOnly(ConvergedWinner),
}

/// Pure lifecycle decision: given the policy, the convergence verdict, and
/// whether the bound rule is already committed (single-bucket 100%-winner), pick
/// the action.
///
/// * No convergence → [`LifecycleAction::NoAction`] under every policy (no
///   autonomous action without a converged winner).
/// * `Advisory` → always [`LifecycleAction::RecordAdvisory`] on convergence (the
///   recorder is idempotent — re-stamping the same winner is harmless).
/// * `AutoCommit` → [`LifecycleAction::Commit`] unless already committed (then
///   [`LifecycleAction::NoAction`]).
/// * `AutoRollout` → [`LifecycleAction::Rollout`] unless already committed (then
///   [`LifecycleAction::StopOnly`] — the commit is done, ensure it is stopped).
#[must_use]
pub fn decide_lifecycle(
    policy: LifecyclePolicy,
    convergence: Option<ConvergedWinner>,
    already_committed: bool,
) -> LifecycleAction {
    let Some(winner) = convergence else {
        return LifecycleAction::NoAction;
    };
    match policy {
        LifecyclePolicy::Advisory => LifecycleAction::RecordAdvisory(winner),
        LifecyclePolicy::AutoCommit => {
            if already_committed {
                LifecycleAction::NoAction
            } else {
                LifecycleAction::Commit(winner)
            }
        }
        LifecyclePolicy::AutoRollout => {
            if already_committed {
                LifecycleAction::StopOnly(winner)
            } else {
                LifecycleAction::Rollout(winner)
            }
        }
    }
}

/// Build the single-bucket "100% to the winner" commit distribution.
///
/// All non-winner arms are dropped (0bp) — documented commit semantics: a commit
/// locks the full exploitable mass to the winner. A single allocation of 10000bp
/// is a valid [`RolloutDistribution`].
#[must_use]
pub fn winner_commit_distribution(winner: &str) -> RolloutDistribution {
    RolloutDistribution {
        allocations: vec![RolloutAllocation {
            variant_key: winner.to_string(),
            percentage_bp: 10_000,
        }],
    }
}

/// Is `dist` already a committed single-bucket 100%-winner distribution for
/// `winner`? Used to make AutoCommit/AutoRollout idempotent across ticks.
#[must_use]
pub fn is_committed_to(dist: &RolloutDistribution, winner: &str) -> bool {
    dist.allocations.len() == 1
        && dist.allocations[0].variant_key == winner
        && dist.allocations[0].percentage_bp == 10_000
}

// ── I/O seam: stop transition + advisory persistence ─────────────────────────

/// Thin async seam over the experimentation-service `TransitionExperiment` (stop)
/// RPC and the advisory-state PG write, so the lifecycle orchestrator is
/// unit-testable without a live experimentation-service or PG.
#[async_trait::async_trait]
pub trait LifecycleTransitioner: Send + Sync {
    /// Stop (conclude) `experiment_id` in `environment_id`. Idempotent at the
    /// caller's discretion: an already-stopped experiment may return Ok.
    async fn stop_experiment(
        &self,
        experiment_id: Uuid,
        environment_id: Uuid,
    ) -> Result<(), anyhow::Error>;

    /// Persist the detected convergence winner + probability onto the experiment
    /// row for advisory surfacing (`bandit_converged_variant` /
    /// `bandit_converged_prob`).
    async fn record_convergence(
        &self,
        experiment_id: Uuid,
        winner: &ConvergedWinner,
    ) -> Result<(), anyhow::Error>;
}

// ── Outcome ──────────────────────────────────────────────────────────────────

/// The result of one lifecycle pass over one experiment.
#[derive(Debug, Clone, PartialEq)]
pub enum LifecycleOutcome {
    /// No convergence / nothing to do / already committed.
    NoAction,
    /// Advisory winner recorded (no traffic change).
    Advisory(ConvergedWinner),
    /// Committed 100% to the winner (flag still held).
    Committed(ConvergedWinner),
    /// Committed + stopped: a full autonomous roll-out.
    RolledOut(ConvergedWinner),
    /// A recoverable failure (apply/stop RPC error); recorded, tick advances.
    Failed(String),
}

// ── Orchestration ──────────────────────────────────────────────────────────────

/// Detect convergence for `exp`'s objective and apply the lifecycle policy.
///
/// Runs AFTER [`crate::bandit::run_bandit_reallocation`] each tick. A non-bandit
/// or unconverged experiment is a no-op (no row written). Every autonomous action
/// records a `bandit_allocation_runs` row (`commit` / `rollout`). Skips never
/// abort the tick; only a real ClickHouse read error or PG insert error
/// propagates as `Err`.
///
/// `current_allocation` is the bound rule's allocation as last written by the
/// reallocation pass (used for the idempotency check). `None` → treated as not
/// yet committed.
#[allow(clippy::too_many_arguments)]
pub async fn run_bandit_lifecycle(
    reader: &dyn CellReader,
    applier: &dyn AllocationApplier,
    transitioner: &dyn LifecycleTransitioner,
    recorder: &dyn RunRecorder,
    exp: &RunningExperiment,
    metrics: &HashMap<Uuid, MetricDefinition>,
    current_allocation: Option<&RolloutDistribution>,
    iteration_end: DateTime<Utc>,
    tick: DateTime<Utc>,
) -> Result<LifecycleOutcome, anyhow::Error> {
    // Only bandit experiments with a config have a lifecycle policy.
    let Some(config) = exp.bandit_config.as_ref() else {
        return Ok(LifecycleOutcome::NoAction);
    };

    // Detect convergence over the objective posteriors.
    let Some(winner) = detect_winner(reader, exp, config, metrics, iteration_end, tick).await?
    else {
        return Ok(LifecycleOutcome::NoAction);
    };

    let already_committed = current_allocation
        .map(|d| is_committed_to(d, &winner.variant_key))
        .unwrap_or(false);

    match decide_lifecycle(
        config.lifecycle_policy,
        Some(winner.clone()),
        already_committed,
    ) {
        LifecycleAction::NoAction => Ok(LifecycleOutcome::NoAction),

        LifecycleAction::RecordAdvisory(w) => {
            transitioner
                .record_convergence(exp.experiment_id, &w)
                .await?;
            Ok(LifecycleOutcome::Advisory(w))
        }

        LifecycleAction::Commit(w) => {
            // Persist the convergence state for surfacing, then commit traffic.
            transitioner
                .record_convergence(exp.experiment_id, &w)
                .await?;
            match do_commit(applier, recorder, exp, &w, "commit").await? {
                Ok(()) => Ok(LifecycleOutcome::Committed(w)),
                Err(detail) => Ok(LifecycleOutcome::Failed(detail)),
            }
        }

        LifecycleAction::Rollout(w) => {
            transitioner
                .record_convergence(exp.experiment_id, &w)
                .await?;
            // Ordered: commit → stop. A crash after commit but before stop
            // re-runs next tick as StopOnly (commit is idempotent / already
            // applied), so the sequence is restart-safe.
            match do_commit(applier, recorder, exp, &w, "rollout").await? {
                Err(detail) => Ok(LifecycleOutcome::Failed(detail)),
                Ok(()) => match transitioner
                    .stop_experiment(exp.experiment_id, exp.env_id)
                    .await
                {
                    Ok(()) => Ok(LifecycleOutcome::RolledOut(w)),
                    Err(e) => {
                        let detail = format!("rollout stop failed: {e}");
                        recorder
                            .record(
                                exp.experiment_id,
                                exp.iteration_id,
                                "rollout",
                                None,
                                Some(commit_json(&w)),
                                "failed",
                                Some(detail.clone()),
                            )
                            .await?;
                        Ok(LifecycleOutcome::Failed(detail))
                    }
                },
            }
        }

        LifecycleAction::StopOnly(w) => {
            // Commit already applied on a prior tick; just ensure the experiment
            // is stopped (idempotent rollout completion).
            match transitioner
                .stop_experiment(exp.experiment_id, exp.env_id)
                .await
            {
                Ok(()) => {
                    recorder
                        .record(
                            exp.experiment_id,
                            exp.iteration_id,
                            "rollout",
                            None,
                            Some(commit_json(&w)),
                            "applied",
                            Some(format!(
                                "rollout completed (commit already applied): winner {} stopped",
                                w.variant_key
                            )),
                        )
                        .await?;
                    Ok(LifecycleOutcome::RolledOut(w))
                }
                Err(e) => {
                    let detail = format!("rollout stop failed: {e}");
                    recorder
                        .record(
                            exp.experiment_id,
                            exp.iteration_id,
                            "rollout",
                            None,
                            Some(commit_json(&w)),
                            "failed",
                            Some(detail.clone()),
                        )
                        .await?;
                    Ok(LifecycleOutcome::Failed(detail))
                }
            }
        }
    }
}

/// Apply the 100%-to-winner commit write via the privileged applier and record a
/// `commit`/`rollout` history row. Returns `Ok(Ok(()))` on a successful apply,
/// `Ok(Err(detail))` on a recoverable apply failure (row recorded as `failed`),
/// or `Err` only on a PG insert error.
async fn do_commit(
    applier: &dyn AllocationApplier,
    recorder: &dyn RunRecorder,
    exp: &RunningExperiment,
    winner: &ConvergedWinner,
    action: &str,
) -> Result<Result<(), String>, anyhow::Error> {
    let dist = winner_commit_distribution(&winner.variant_key);
    let new_json = commit_json(winner);

    match applier.apply(exp.experiment_id, &dist).await {
        Ok(ApplyResult::Applied {
            resolved_target,
            new_version,
        }) => {
            recorder
                .record(
                    exp.experiment_id,
                    exp.iteration_id,
                    action,
                    None,
                    Some(new_json),
                    "applied",
                    Some(format!(
                        "committed 100% to winner {} on {resolved_target} (version {new_version})",
                        winner.variant_key
                    )),
                )
                .await?;
            Ok(Ok(()))
        }
        Ok(ApplyResult::Skipped { detail }) => {
            recorder
                .record(
                    exp.experiment_id,
                    exp.iteration_id,
                    action,
                    None,
                    Some(new_json),
                    "skipped",
                    Some(detail.clone()),
                )
                .await?;
            Ok(Err(detail))
        }
        Ok(ApplyResult::Failed {
            detail,
            version_conflict: _,
        }) => {
            recorder
                .record(
                    exp.experiment_id,
                    exp.iteration_id,
                    action,
                    None,
                    Some(new_json),
                    "failed",
                    Some(detail.clone()),
                )
                .await?;
            Ok(Err(detail))
        }
        Err(e) => {
            let detail = format!("commit apply RPC error: {e}");
            recorder
                .record(
                    exp.experiment_id,
                    exp.iteration_id,
                    action,
                    None,
                    Some(new_json),
                    "failed",
                    Some(detail.clone()),
                )
                .await?;
            Ok(Err(detail))
        }
    }
}

/// JSON for the committed allocation history row.
fn commit_json(winner: &ConvergedWinner) -> Value {
    serde_json::json!({ winner.variant_key.clone(): 10_000 })
}

/// Detect the converged winner for the experiment's objective, or `None`.
///
/// Builds the same objective reward arms the reallocation pass uses (via
/// [`reward_arms`]) and runs [`detect_convergence`] at the config's threshold,
/// seeded deterministically per tick.
async fn detect_winner(
    reader: &dyn CellReader,
    exp: &RunningExperiment,
    config: &BanditConfig,
    metrics: &HashMap<Uuid, MetricDefinition>,
    iteration_end: DateTime<Utc>,
    tick: DateTime<Utc>,
) -> Result<Option<ConvergedWinner>, anyhow::Error> {
    let mut metric_rewards: Vec<MetricRewards> = Vec::new();
    for mid in objective_metric_ids(&config.objective) {
        let Some(def) = metrics.get(&mid) else {
            continue;
        };
        if let Some(mr) = build_metric_rewards_pub(reader, exp, def, metrics, iteration_end).await?
        {
            metric_rewards.push(mr);
        }
    }
    if metric_rewards.is_empty() {
        return Ok(None);
    }

    let (arms, goal, _exploitable) = reward_arms(&config.objective, &metric_rewards);
    if arms.len() < 2 {
        return Ok(None);
    }

    let seed = derive_seed(
        exp.experiment_id,
        exp.iteration_id,
        exp.variant_keys.len(),
        tick,
    );
    Ok(detect_convergence(
        &arms,
        goal,
        config.convergence_prob_threshold,
        seed,
    ))
}

// ── Production transitioner ──────────────────────────────────────────────────

/// Production [`LifecycleTransitioner`]: stops via the experimentation-service
/// `TransitionExperiment` RPC and persists advisory convergence state directly to
/// PG (runtime `sqlx::query`, no compile-time macro — mirrors [`crate::bandit`]).
pub struct GrpcLifecycleTransitioner {
    exp_client: std::sync::Arc<
        tokio::sync::Mutex<
            stitchd_proto::experiments::v1::experimentation_service_client::ExperimentationServiceClient<
                tonic::transport::Channel,
            >,
        >,
    >,
    pool: sqlx::PgPool,
}

impl GrpcLifecycleTransitioner {
    /// Assemble from the shared experimentation-service client + PG pool.
    #[must_use]
    pub fn new(
        exp_client: std::sync::Arc<
            tokio::sync::Mutex<
                stitchd_proto::experiments::v1::experimentation_service_client::ExperimentationServiceClient<
                    tonic::transport::Channel,
                >,
            >,
        >,
        pool: sqlx::PgPool,
    ) -> Self {
        Self { exp_client, pool }
    }
}

#[async_trait::async_trait]
impl LifecycleTransitioner for GrpcLifecycleTransitioner {
    async fn stop_experiment(
        &self,
        experiment_id: Uuid,
        environment_id: Uuid,
    ) -> Result<(), anyhow::Error> {
        use stitchd_proto::experiments::v1::{ExperimentStatus, TransitionExperimentRequest};
        let req = TransitionExperimentRequest {
            environment_id: environment_id.to_string(),
            experiment_id: experiment_id.to_string(),
            // Proto CONCLUDED maps to core Stopped (permanent stop) — releases
            // the whole-flag lock as the active experiment is gone.
            new_status: ExperimentStatus::Concluded as i32,
            reason: "autonomous bandit auto-rollout (winner committed)".to_string(),
        };
        let resp = {
            let mut client = self.exp_client.lock().await;
            client.transition_experiment(req).await
        };
        resp.map(|_| ())
            .map_err(|s| anyhow::anyhow!("TransitionExperiment(stop) failed: {s}"))
    }

    async fn record_convergence(
        &self,
        experiment_id: Uuid,
        winner: &ConvergedWinner,
    ) -> Result<(), anyhow::Error> {
        sqlx::query(
            "UPDATE experiments \
             SET bandit_converged_variant = $2, bandit_converged_prob = $3 \
             WHERE id = $1",
        )
        .bind(experiment_id)
        .bind(&winner.variant_key)
        .bind(winner.prob)
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use stitchd_core::experimentation::bandit::ConvergedWinner;

    fn winner(key: &str, p: f64) -> ConvergedWinner {
        ConvergedWinner {
            variant_key: key.into(),
            prob: p,
        }
    }

    // ── Pure decision ───────────────────────────────────────────────────────

    #[test]
    fn no_convergence_is_no_action_under_every_policy() {
        for policy in [
            LifecyclePolicy::Advisory,
            LifecyclePolicy::AutoCommit,
            LifecyclePolicy::AutoRollout,
        ] {
            assert_eq!(
                decide_lifecycle(policy, None, false),
                LifecycleAction::NoAction
            );
        }
    }

    #[test]
    fn advisory_records_on_convergence_never_commits() {
        let w = winner("b", 0.97);
        assert_eq!(
            decide_lifecycle(LifecyclePolicy::Advisory, Some(w.clone()), false),
            LifecycleAction::RecordAdvisory(w.clone())
        );
        // Even if (hypothetically) already committed, advisory never escalates.
        assert_eq!(
            decide_lifecycle(LifecyclePolicy::Advisory, Some(w.clone()), true),
            LifecycleAction::RecordAdvisory(w)
        );
    }

    #[test]
    fn auto_commit_commits_then_no_action() {
        let w = winner("b", 0.97);
        assert_eq!(
            decide_lifecycle(LifecyclePolicy::AutoCommit, Some(w.clone()), false),
            LifecycleAction::Commit(w.clone())
        );
        // Idempotent: already committed → no further action.
        assert_eq!(
            decide_lifecycle(LifecyclePolicy::AutoCommit, Some(w), true),
            LifecycleAction::NoAction
        );
    }

    #[test]
    fn auto_rollout_rolls_out_then_stop_only() {
        let w = winner("b", 0.99);
        assert_eq!(
            decide_lifecycle(LifecyclePolicy::AutoRollout, Some(w.clone()), false),
            LifecycleAction::Rollout(w.clone())
        );
        // Idempotent: commit already done → just stop.
        assert_eq!(
            decide_lifecycle(LifecyclePolicy::AutoRollout, Some(w.clone()), true),
            LifecycleAction::StopOnly(w)
        );
    }

    #[test]
    fn winner_commit_distribution_is_single_bucket_full() {
        let d = winner_commit_distribution("win");
        assert_eq!(d.allocations.len(), 1);
        assert_eq!(d.allocations[0].variant_key, "win");
        assert_eq!(d.allocations[0].percentage_bp, 10_000);
        assert!(d.validate().is_ok());
    }

    #[test]
    fn is_committed_to_detects_committed_distribution() {
        assert!(is_committed_to(&winner_commit_distribution("win"), "win"));
        assert!(!is_committed_to(
            &winner_commit_distribution("win"),
            "other"
        ));
        // A multi-bucket exploration split is not committed.
        let split = RolloutDistribution {
            allocations: vec![
                RolloutAllocation {
                    variant_key: "win".into(),
                    percentage_bp: 9500,
                },
                RolloutAllocation {
                    variant_key: "ctrl".into(),
                    percentage_bp: 500,
                },
            ],
        };
        assert!(!is_committed_to(&split, "win"));
    }

    // ── Orchestration with fakes ────────────────────────────────────────────

    use crate::bandit::tests_support::{FakeApplier, FakeReader, FakeRecorder, count_metric};
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[derive(Default)]
    struct FakeTransitioner {
        stops: AtomicUsize,
        records: AtomicUsize,
        last_winner: std::sync::Mutex<Option<ConvergedWinner>>,
        fail_stop: bool,
    }

    #[async_trait::async_trait]
    impl LifecycleTransitioner for FakeTransitioner {
        async fn stop_experiment(
            &self,
            _experiment_id: Uuid,
            _environment_id: Uuid,
        ) -> Result<(), anyhow::Error> {
            self.stops.fetch_add(1, Ordering::SeqCst);
            if self.fail_stop {
                anyhow::bail!("boom");
            }
            Ok(())
        }

        async fn record_convergence(
            &self,
            _experiment_id: Uuid,
            w: &ConvergedWinner,
        ) -> Result<(), anyhow::Error> {
            self.records.fetch_add(1, Ordering::SeqCst);
            *self.last_winner.lock().unwrap() = Some(w.clone());
            Ok(())
        }
    }

    fn running_with_policy(
        policy: LifecyclePolicy,
        env: Uuid,
        metric_id: Uuid,
    ) -> (RunningExperiment, HashMap<Uuid, MetricDefinition>) {
        use stitchd_core::experimentation::bandit::{
            BanditAlgorithm, BanditConfig, ExperimentMode, PropagationMode, RewardObjective,
        };
        let config = BanditConfig {
            algorithm: BanditAlgorithm::ThompsonSampling,
            propagation_mode: PropagationMode::Static,
            min_exploration_bp: 100,
            objective: RewardObjective::Scalar { metric_id },
            lifecycle_policy: policy,
            convergence_prob_threshold: 0.9,
        };
        let exp = crate::bandit::tests_support::running_bandit_with(
            Some(config),
            ExperimentMode::Bandit,
            env,
            vec!["control".into(), "win".into()],
            vec![metric_id],
        );
        let mut metrics = HashMap::new();
        metrics.insert(metric_id, count_metric(metric_id, env));
        (exp, metrics)
    }

    #[tokio::test]
    async fn advisory_records_no_traffic_change() {
        let env = Uuid::new_v4();
        let metric_id = Uuid::new_v4();
        let (exp, metrics) = running_with_policy(LifecyclePolicy::Advisory, env, metric_id);
        // Decisive winner: control 100/1000, win 400/1000.
        let reader = FakeReader::with_conversions(
            metric_id,
            vec![("control", 1000, 100), ("win", 1000, 400)],
        );
        let applier = FakeApplier::default();
        let transitioner = FakeTransitioner::default();
        let recorder = FakeRecorder::default();
        let now = Utc::now();

        let out = run_bandit_lifecycle(
            &reader,
            &applier,
            &transitioner,
            &recorder,
            &exp,
            &metrics,
            None,
            now,
            now,
        )
        .await
        .unwrap();

        match out {
            LifecycleOutcome::Advisory(w) => assert_eq!(w.variant_key, "win"),
            other => panic!("expected advisory, got {other:?}"),
        }
        // Advisory: convergence recorded, NO commit apply, NO stop, NO history row.
        assert_eq!(transitioner.records.load(Ordering::SeqCst), 1);
        assert_eq!(applier.apply_calls(), 0);
        assert_eq!(transitioner.stops.load(Ordering::SeqCst), 0);
        assert_eq!(recorder.count(), 0);
    }

    #[tokio::test]
    async fn auto_commit_commits_100_to_winner_no_stop() {
        let env = Uuid::new_v4();
        let metric_id = Uuid::new_v4();
        let (exp, metrics) = running_with_policy(LifecyclePolicy::AutoCommit, env, metric_id);
        let reader = FakeReader::with_conversions(
            metric_id,
            vec![("control", 1000, 100), ("win", 1000, 400)],
        );
        let applier = FakeApplier::default();
        let transitioner = FakeTransitioner::default();
        let recorder = FakeRecorder::default();
        let now = Utc::now();

        let out = run_bandit_lifecycle(
            &reader,
            &applier,
            &transitioner,
            &recorder,
            &exp,
            &metrics,
            None,
            now,
            now,
        )
        .await
        .unwrap();

        assert!(matches!(out, LifecycleOutcome::Committed(_)));
        assert_eq!(applier.apply_calls(), 1);
        // Committed 100% to the winner.
        let last = applier
            .last_allocation()
            .expect("an allocation was applied");
        assert!(is_committed_to(&last, "win"));
        // Flag stays locked: no stop.
        assert_eq!(transitioner.stops.load(Ordering::SeqCst), 0);
        // One `commit` history row.
        assert_eq!(recorder.count(), 1);
        assert_eq!(recorder.last_action().as_deref(), Some("commit"));
    }

    #[tokio::test]
    async fn auto_commit_idempotent_when_already_committed() {
        let env = Uuid::new_v4();
        let metric_id = Uuid::new_v4();
        let (exp, metrics) = running_with_policy(LifecyclePolicy::AutoCommit, env, metric_id);
        let reader = FakeReader::with_conversions(
            metric_id,
            vec![("control", 1000, 100), ("win", 1000, 400)],
        );
        let applier = FakeApplier::default();
        let transitioner = FakeTransitioner::default();
        let recorder = FakeRecorder::default();
        let now = Utc::now();

        // Already committed to "win".
        let committed = winner_commit_distribution("win");
        let out = run_bandit_lifecycle(
            &reader,
            &applier,
            &transitioner,
            &recorder,
            &exp,
            &metrics,
            Some(&committed),
            now,
            now,
        )
        .await
        .unwrap();

        assert_eq!(out, LifecycleOutcome::NoAction);
        assert_eq!(applier.apply_calls(), 0, "no re-commit");
        assert_eq!(recorder.count(), 0, "no row on idempotent no-op");
    }

    #[tokio::test]
    async fn auto_rollout_sequence_is_commit_then_stop() {
        let env = Uuid::new_v4();
        let metric_id = Uuid::new_v4();
        let (exp, metrics) = running_with_policy(LifecyclePolicy::AutoRollout, env, metric_id);
        let reader = FakeReader::with_conversions(
            metric_id,
            vec![("control", 1000, 100), ("win", 1000, 400)],
        );
        let applier = FakeApplier::default();
        let transitioner = FakeTransitioner::default();
        let recorder = FakeRecorder::default();
        let now = Utc::now();

        let out = run_bandit_lifecycle(
            &reader,
            &applier,
            &transitioner,
            &recorder,
            &exp,
            &metrics,
            None,
            now,
            now,
        )
        .await
        .unwrap();

        assert!(matches!(out, LifecycleOutcome::RolledOut(_)));
        // commit happened (apply) AND stop happened, in that order.
        assert_eq!(applier.apply_calls(), 1);
        assert!(is_committed_to(&applier.last_allocation().unwrap(), "win"));
        assert_eq!(transitioner.stops.load(Ordering::SeqCst), 1);
        // `rollout` history row recorded.
        assert_eq!(recorder.last_action().as_deref(), Some("rollout"));
    }

    #[tokio::test]
    async fn auto_rollout_idempotent_stop_only_when_already_committed() {
        let env = Uuid::new_v4();
        let metric_id = Uuid::new_v4();
        let (exp, metrics) = running_with_policy(LifecyclePolicy::AutoRollout, env, metric_id);
        let reader = FakeReader::with_conversions(
            metric_id,
            vec![("control", 1000, 100), ("win", 1000, 400)],
        );
        let applier = FakeApplier::default();
        let transitioner = FakeTransitioner::default();
        let recorder = FakeRecorder::default();
        let now = Utc::now();

        let committed = winner_commit_distribution("win");
        let out = run_bandit_lifecycle(
            &reader,
            &applier,
            &transitioner,
            &recorder,
            &exp,
            &metrics,
            Some(&committed),
            now,
            now,
        )
        .await
        .unwrap();

        assert!(matches!(out, LifecycleOutcome::RolledOut(_)));
        // No re-commit; only the stop runs.
        assert_eq!(applier.apply_calls(), 0, "commit already applied earlier");
        assert_eq!(transitioner.stops.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn no_convergence_yields_no_action() {
        let env = Uuid::new_v4();
        let metric_id = Uuid::new_v4();
        let (exp, metrics) = running_with_policy(LifecyclePolicy::AutoRollout, env, metric_id);
        // Two near-identical arms — no convergence at 0.9.
        let reader = FakeReader::with_conversions(
            metric_id,
            vec![("control", 1000, 100), ("win", 1000, 105)],
        );
        let applier = FakeApplier::default();
        let transitioner = FakeTransitioner::default();
        let recorder = FakeRecorder::default();
        let now = Utc::now();

        let out = run_bandit_lifecycle(
            &reader,
            &applier,
            &transitioner,
            &recorder,
            &exp,
            &metrics,
            None,
            now,
            now,
        )
        .await
        .unwrap();

        assert_eq!(out, LifecycleOutcome::NoAction);
        assert_eq!(applier.apply_calls(), 0);
        assert_eq!(transitioner.stops.load(Ordering::SeqCst), 0);
        assert_eq!(recorder.count(), 0);
    }

    #[tokio::test]
    async fn non_bandit_is_no_action() {
        use stitchd_core::experimentation::bandit::ExperimentMode;
        let env = Uuid::new_v4();
        let metric_id = Uuid::new_v4();
        let exp = crate::bandit::tests_support::running_bandit_with(
            None,
            ExperimentMode::Fixed,
            env,
            vec!["control".into(), "win".into()],
            vec![metric_id],
        );
        let mut metrics = HashMap::new();
        metrics.insert(metric_id, count_metric(metric_id, env));
        let reader = FakeReader::with_conversions(
            metric_id,
            vec![("control", 1000, 100), ("win", 1000, 400)],
        );
        let applier = FakeApplier::default();
        let transitioner = FakeTransitioner::default();
        let recorder = FakeRecorder::default();
        let now = Utc::now();

        let out = run_bandit_lifecycle(
            &reader,
            &applier,
            &transitioner,
            &recorder,
            &exp,
            &metrics,
            None,
            now,
            now,
        )
        .await
        .unwrap();
        assert_eq!(out, LifecycleOutcome::NoAction);
    }
}
