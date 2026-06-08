//! Autonomous bandit optimization campaigns (FR8).
//!
//! A campaign owns a sequence of bandit experiment *iterations* on one flag. It
//! sits on top of the Phase-7 lifecycle: when a campaign-owned iteration **rolls
//! out** (converges + commits + stops under `auto_rollout`), the campaign spawns
//! the **next** iteration — the winner becomes the new control, plus any
//! newly-registered variants (`variant_discovery` policy) — *unless* the
//! `max_iterations` / budget ceiling is reached, in which case the campaign is
//! finalized. A configured **drift** detector reopens exploration after a commit
//! by spawning a fresh iteration when the committed winner's posterior degrades.
//!
//! ## Bounded + idempotent
//!
//! Spawning is gated by the campaign's `iterations_spawned` counter + `version`
//! via the repository's atomic
//! [`try_claim_spawn`](stitchd_db::repository::pg::BanditCampaignRepository::try_claim_spawn):
//! a given convergence event claims AT MOST one spawn slot (a stale-version retry
//! or a capped campaign claims nothing). When a claim is refused because the cap
//! is reached, the campaign is finalized (`completed`). Every auto-created
//! iteration is audited as `bandit_allocation_runs action='spawn_iteration'`.
//!
//! This module's decision core ([`decide_campaign_action`]) is pure; the spawn /
//! finalize I/O is behind the [`CampaignSpawner`] trait so it is unit-testable
//! with fakes.

use chrono::{DateTime, Utc};
use uuid::Uuid;

use stitchd_core::experimentation::bandit::{
    BanditCampaign, ConvergedWinner, DriftVerdict, GoalDirection as BanditGoal, MetricRewards,
    detect_drift, reward_arms,
};
use stitchd_core::metric::MetricDefinition;

use crate::bandit::{build_metric_rewards_pub, derive_seed, objective_metric_ids};
use crate::compute::CellReader;
use crate::lifecycle::LifecycleOutcome;
use crate::scheduler::RunningExperiment;
use std::collections::HashMap;

/// The campaign action chosen for one tick of one campaign-owned experiment.
#[derive(Debug, Clone, PartialEq)]
pub enum CampaignAction {
    /// Nothing to do (no rollout / no drift, or campaign not spawnable).
    NoAction,
    /// Spawn the next iteration with `winner` as the new control.
    SpawnNext(ConvergedWinner),
    /// The campaign hit its cap on this convergence — finalize (no spawn).
    Finalize,
}

/// The reason a spawn was triggered (for the audit detail).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpawnTrigger {
    /// The current iteration rolled out (converged winner committed + stopped).
    Convergence,
    /// The committed winner drifted; exploration is reopened.
    Drift,
}

/// Pure campaign decision.
///
/// * A `RolledOut` lifecycle outcome on a spawnable campaign → spawn the next
///   iteration; on a non-spawnable (capped / inactive) campaign → finalize.
/// * Otherwise, a detected drift on a spawnable campaign → spawn (reopen); on a
///   non-spawnable campaign → no action (a capped campaign does not reopen).
/// * Everything else → no action.
#[must_use]
pub fn decide_campaign_action(
    lifecycle: &LifecycleOutcome,
    drift: Option<&DriftVerdict>,
    can_spawn: bool,
) -> (CampaignAction, SpawnTrigger) {
    if let LifecycleOutcome::RolledOut(w) = lifecycle {
        return if can_spawn {
            (
                CampaignAction::SpawnNext(w.clone()),
                SpawnTrigger::Convergence,
            )
        } else {
            // Rolled out but the campaign can't spawn another → it's done.
            (CampaignAction::Finalize, SpawnTrigger::Convergence)
        };
    }
    if let Some(v) = drift
        && v.drifted
        && can_spawn
        && let Some((challenger_key, prob)) = v.challenger.clone()
    {
        // Reopen exploration: the challenger becomes the new control candidate.
        return (
            CampaignAction::SpawnNext(ConvergedWinner {
                variant_key: challenger_key,
                prob,
            }),
            SpawnTrigger::Drift,
        );
    }
    (CampaignAction::NoAction, SpawnTrigger::Convergence)
}

/// The result of a spawn attempt.
#[derive(Debug, Clone, PartialEq)]
pub enum SpawnOutcome {
    /// No campaign linkage / nothing to do.
    NoAction,
    /// A next iteration was spawned (campaign claimed a slot).
    Spawned {
        /// The new control variant for the spawned iteration.
        new_control: String,
        /// Why the spawn fired.
        trigger: SpawnTrigger,
    },
    /// The spawn was a no-op: the slot was already claimed (idempotent) or the
    /// cap was reached (campaign finalized).
    NotSpawned {
        /// Human-readable reason.
        reason: String,
    },
    /// A recoverable failure during spawn; recorded, tick advances.
    Failed(String),
}

/// I/O seam for the campaign spawn / finalize path, so the orchestration is
/// unit-testable without a live experimentation-service or PG.
#[async_trait::async_trait]
pub trait CampaignSpawner: Send + Sync {
    /// Load the campaign by id.
    async fn load(&self, campaign_id: Uuid) -> Result<BanditCampaign, anyhow::Error>;

    /// Atomically claim a spawn slot (idempotent + cap-bounded). Returns the
    /// updated campaign when the slot was claimed, or `None` when refused
    /// (already spawned / capped).
    async fn try_claim_spawn(
        &self,
        campaign_id: Uuid,
        expected_version: i64,
    ) -> Result<Option<BanditCampaign>, anyhow::Error>;

    /// Open the next iteration: rewrite the bound rule so `new_control` is the
    /// new control, then restart the experiment (a stopped→running transition
    /// creates a fresh `experiment_iterations` row). `exp` is the just-rolled-out
    /// experiment.
    async fn open_next_iteration(
        &self,
        exp: &RunningExperiment,
        campaign: &BanditCampaign,
        new_control: &str,
    ) -> Result<(), anyhow::Error>;

    /// Finalize the campaign (set status `completed`) when its cap is reached.
    async fn finalize(&self, campaign: &BanditCampaign) -> Result<(), anyhow::Error>;
}

/// Run the campaign spawn pass after the Phase-7 lifecycle pass.
///
/// A no-op unless `exp` is campaign-owned (`bandit_campaign_id`). On a rolled-out
/// iteration (or a detected drift), claims a spawn slot and opens the next
/// iteration, recording `bandit_allocation_runs action='spawn_iteration'`.
/// Bounded + idempotent via the repository's atomic claim. Only a PG insert error
/// propagates as `Err`.
#[allow(clippy::too_many_arguments)]
pub async fn run_campaign_spawn(
    reader: &dyn CellReader,
    spawner: &dyn CampaignSpawner,
    recorder: &dyn crate::bandit::RunRecorder,
    exp: &RunningExperiment,
    metrics: &HashMap<Uuid, MetricDefinition>,
    lifecycle: &LifecycleOutcome,
    iteration_end: DateTime<Utc>,
    tick: DateTime<Utc>,
) -> Result<SpawnOutcome, anyhow::Error> {
    let Some(campaign_id) = exp.bandit_campaign_id else {
        return Ok(SpawnOutcome::NoAction);
    };

    let campaign = spawner.load(campaign_id).await?;

    // Drift is only meaningful AFTER a commit, and only when not already rolling
    // out. Compute it lazily for committed iterations.
    let drift = if matches!(lifecycle, LifecycleOutcome::Committed(_)) {
        compute_drift(reader, exp, metrics, &campaign, iteration_end, tick).await?
    } else {
        None
    };

    let (action, trigger) = decide_campaign_action(lifecycle, drift.as_ref(), campaign.can_spawn());

    match action {
        CampaignAction::NoAction => Ok(SpawnOutcome::NoAction),

        CampaignAction::Finalize => {
            spawner.finalize(&campaign).await?;
            recorder
                .record(
                    exp.experiment_id,
                    exp.iteration_id,
                    "spawn_iteration",
                    None,
                    None,
                    "skipped",
                    Some(format!(
                        "campaign {campaign_id} reached max_iterations \
                         ({}); finalized",
                        campaign.config.max_iterations
                    )),
                )
                .await?;
            Ok(SpawnOutcome::NotSpawned {
                reason: "cap reached; campaign finalized".to_string(),
            })
        }

        CampaignAction::SpawnNext(winner) => {
            // Atomically claim the spawn slot (idempotent + cap-bounded).
            let Some(claimed) = spawner
                .try_claim_spawn(campaign_id, campaign.version)
                .await?
            else {
                // Refused: already spawned by a concurrent/previous tick, or cap
                // reached between load and claim. Idempotent no-op.
                return Ok(SpawnOutcome::NotSpawned {
                    reason: "spawn slot not claimed (idempotent no-op or cap reached)".to_string(),
                });
            };

            match spawner
                .open_next_iteration(exp, &claimed, &winner.variant_key)
                .await
            {
                Ok(()) => {
                    recorder
                        .record(
                            exp.experiment_id,
                            exp.iteration_id,
                            "spawn_iteration",
                            None,
                            Some(serde_json::json!({
                                "new_control": winner.variant_key.clone(),
                                "trigger": match trigger {
                                    SpawnTrigger::Convergence => "convergence",
                                    SpawnTrigger::Drift => "drift",
                                },
                                "iteration": claimed.iterations_spawned,
                            })),
                            "applied",
                            Some(format!(
                                "campaign {campaign_id} spawned iteration {} (control={})",
                                claimed.iterations_spawned, winner.variant_key
                            )),
                        )
                        .await?;
                    Ok(SpawnOutcome::Spawned {
                        new_control: winner.variant_key,
                        trigger,
                    })
                }
                Err(e) => {
                    let detail = format!("open_next_iteration failed: {e}");
                    recorder
                        .record(
                            exp.experiment_id,
                            exp.iteration_id,
                            "spawn_iteration",
                            None,
                            None,
                            "failed",
                            Some(detail.clone()),
                        )
                        .await?;
                    Ok(SpawnOutcome::Failed(detail))
                }
            }
        }
    }
}

/// Compute the drift verdict for a committed campaign iteration against its
/// objective posteriors.
async fn compute_drift(
    reader: &dyn CellReader,
    exp: &RunningExperiment,
    metrics: &HashMap<Uuid, MetricDefinition>,
    campaign: &BanditCampaign,
    iteration_end: DateTime<Utc>,
    tick: DateTime<Utc>,
) -> Result<Option<DriftVerdict>, anyhow::Error> {
    let Some(config) = exp.bandit_config.as_ref() else {
        return Ok(None);
    };
    // The committed winner is the first variant of a single-bucket allocation;
    // when unknown, fall back to the goal-directed best arm.
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
    let (arms, goal, _excl): (_, BanditGoal, _) = reward_arms(&config.objective, &metric_rewards);
    if arms.len() < 2 {
        return Ok(None);
    }
    // Heuristic committed winner: the current goal-directed best arm (the commit
    // would have locked to it). Drift then asks whether it still dominates.
    let committed = pick_committed_winner(&arms, goal);
    let seed = derive_seed(
        exp.experiment_id,
        exp.iteration_id,
        exp.variant_keys.len(),
        tick,
    );
    Ok(Some(detect_drift(
        &arms,
        goal,
        &committed,
        campaign.config.drift_threshold,
        seed,
    )))
}

/// Pick the committed winner as the arm with the best goal-directed posterior
/// mean (a deterministic proxy when the persisted commit target is not threaded
/// through to this pass).
fn pick_committed_winner(
    arms: &[stitchd_core::experimentation::bandit::BanditArm],
    goal: BanditGoal,
) -> String {
    let mut best = &arms[0];
    for a in &arms[1..] {
        let better = match goal {
            BanditGoal::Increase => a.posterior.mean() > best.posterior.mean(),
            BanditGoal::Decrease => a.posterior.mean() < best.posterior.mean(),
        };
        if better {
            best = a;
        }
    }
    best.variant_key.clone()
}

// ── Production spawner ───────────────────────────────────────────────────────

/// Production [`CampaignSpawner`]: claims spawn slots via the PG campaign repo,
/// restarts the experiment (stopped→running, creating a fresh iteration) via the
/// experimentation-service `TransitionExperiment`, and rewrites the bound rule so
/// the winner is the new control via the privileged `ApplyBanditAllocation`.
pub struct GrpcCampaignSpawner {
    exp_client: std::sync::Arc<
        tokio::sync::Mutex<
            stitchd_proto::experiments::v1::experimentation_service_client::ExperimentationServiceClient<
                tonic::transport::Channel,
            >,
        >,
    >,
    pool: sqlx::PgPool,
}

impl GrpcCampaignSpawner {
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
impl CampaignSpawner for GrpcCampaignSpawner {
    async fn load(&self, campaign_id: Uuid) -> Result<BanditCampaign, anyhow::Error> {
        use stitchd_db::repository::pg::{BanditCampaignRepository, PgBanditCampaignRepository};
        let repo = PgBanditCampaignRepository::new(self.pool.clone());
        repo.find_by_id(campaign_id)
            .await
            .map_err(|e| anyhow::anyhow!("load campaign: {e}"))
    }

    async fn try_claim_spawn(
        &self,
        campaign_id: Uuid,
        expected_version: i64,
    ) -> Result<Option<BanditCampaign>, anyhow::Error> {
        use stitchd_db::repository::pg::{BanditCampaignRepository, PgBanditCampaignRepository};
        let repo = PgBanditCampaignRepository::new(self.pool.clone());
        repo.try_claim_spawn(campaign_id, expected_version)
            .await
            .map_err(|e| anyhow::anyhow!("claim spawn: {e}"))
    }

    async fn open_next_iteration(
        &self,
        exp: &RunningExperiment,
        _campaign: &BanditCampaign,
        new_control: &str,
    ) -> Result<(), anyhow::Error> {
        use stitchd_proto::experiments::v1::{
            ApplyBanditAllocationRequest, ExperimentStatus, TransitionExperimentRequest,
        };
        // 1. Restart the experiment (stopped→running): the repo creates a fresh
        //    experiment_iterations row and re-acquires the whole-flag lock.
        {
            let mut client = self.exp_client.lock().await;
            client
                .transition_experiment(TransitionExperimentRequest {
                    environment_id: exp.env_id.to_string(),
                    experiment_id: exp.experiment_id.to_string(),
                    new_status: ExperimentStatus::Active as i32,
                    reason: format!(
                        "campaign auto-spawn: new iteration with control={new_control}"
                    ),
                })
                .await
                .map_err(|s| anyhow::anyhow!("restart experiment: {s}"))?;
        }
        // 2. Rewrite the bound rule so the new control gets the dominant share and
        //    the other arms keep the exploration floor — the winner-as-new-control
        //    wiring. We push 90% to the new control, splitting the remaining 10%
        //    evenly across the other arms (each >0 so the distribution validates).
        let others: Vec<&String> = exp
            .variant_keys
            .iter()
            .filter(|k| k.as_str() != new_control)
            .collect();
        let allocations = control_dominant_allocation(new_control, &others);
        {
            let mut client = self.exp_client.lock().await;
            client
                .apply_bandit_allocation(ApplyBanditAllocationRequest {
                    experiment_id: exp.experiment_id.to_string(),
                    allocations,
                    expected_version: 0,
                    realtime_model: None,
                })
                .await
                .map_err(|s| anyhow::anyhow!("set new control allocation: {s}"))?;
        }
        Ok(())
    }

    async fn finalize(&self, campaign: &BanditCampaign) -> Result<(), anyhow::Error> {
        use stitchd_core::experimentation::bandit::BanditCampaignStatus;
        use stitchd_db::repository::pg::{BanditCampaignRepository, PgBanditCampaignRepository};
        let repo = PgBanditCampaignRepository::new(self.pool.clone());
        repo.set_status(
            campaign.id,
            BanditCampaignStatus::Completed,
            campaign.version,
        )
        .await
        .map(|_| ())
        .map_err(|e| anyhow::anyhow!("finalize campaign: {e}"))
    }
}

/// Build a control-dominant allocation (new control 90%, remaining 10% spread
/// across the other arms, every arm >0 so it validates). With no other arms the
/// control takes the full 10000bp.
fn control_dominant_allocation(
    new_control: &str,
    others: &[&String],
) -> Vec<stitchd_proto::experiments::v1::BanditAllocationBucket> {
    use stitchd_proto::experiments::v1::BanditAllocationBucket;
    if others.is_empty() {
        return vec![BanditAllocationBucket {
            variant_key: new_control.to_string(),
            weight_bp: 10_000,
        }];
    }
    let n = others.len() as u32;
    let each = (1_000 / n).max(1);
    let spread = each * n;
    let control_bp = 10_000 - spread;
    let mut out = vec![BanditAllocationBucket {
        variant_key: new_control.to_string(),
        weight_bp: control_bp,
    }];
    for (i, k) in others.iter().enumerate() {
        // Push any rounding remainder onto the first "other" arm.
        let extra = if i == 0 {
            10_000 - control_bp - spread
        } else {
            0
        };
        out.push(BanditAllocationBucket {
            variant_key: (*k).clone(),
            weight_bp: each + extra,
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use stitchd_core::experimentation::bandit::{
        BanditCampaignConfig, BanditCampaignStatus, VariantDiscoveryPolicy,
    };

    fn winner(k: &str, p: f64) -> ConvergedWinner {
        ConvergedWinner {
            variant_key: k.into(),
            prob: p,
        }
    }

    fn campaign(spawned: i32, max: u32, status: BanditCampaignStatus) -> BanditCampaign {
        BanditCampaign {
            id: Uuid::new_v4(),
            environment_id: Uuid::new_v4(),
            flag_id: Uuid::new_v4(),
            name: "c".into(),
            config: BanditCampaignConfig {
                max_iterations: max,
                drift_threshold: 0.5,
                variant_discovery: VariantDiscoveryPolicy::WinnerPlusNew,
                budget_cap: None,
            },
            status,
            iterations_spawned: spawned,
            version: spawned as i64,
        }
    }

    // ── pure decision ───────────────────────────────────────────────────────

    #[test]
    fn rollout_spawns_when_under_cap() {
        let lc = LifecycleOutcome::RolledOut(winner("win", 0.99));
        let (a, t) = decide_campaign_action(&lc, None, true);
        assert_eq!(a, CampaignAction::SpawnNext(winner("win", 0.99)));
        assert_eq!(t, SpawnTrigger::Convergence);
    }

    #[test]
    fn rollout_finalizes_at_cap() {
        let lc = LifecycleOutcome::RolledOut(winner("win", 0.99));
        let (a, _) = decide_campaign_action(&lc, None, false);
        assert_eq!(a, CampaignAction::Finalize);
    }

    #[test]
    fn drift_spawns_when_committed_and_under_cap() {
        let lc = LifecycleOutcome::Committed(winner("win", 0.6));
        let v = DriftVerdict {
            drifted: true,
            winner_prob: 0.3,
            challenger: Some(("rival".into(), 0.7)),
        };
        let (a, t) = decide_campaign_action(&lc, Some(&v), true);
        assert_eq!(a, CampaignAction::SpawnNext(winner("rival", 0.7)));
        assert_eq!(t, SpawnTrigger::Drift);
    }

    #[test]
    fn no_drift_no_spawn() {
        let lc = LifecycleOutcome::Committed(winner("win", 0.9));
        let v = DriftVerdict {
            drifted: false,
            winner_prob: 0.9,
            challenger: Some(("rival".into(), 0.1)),
        };
        let (a, _) = decide_campaign_action(&lc, Some(&v), true);
        assert_eq!(a, CampaignAction::NoAction);
    }

    #[test]
    fn drift_at_cap_does_not_reopen() {
        let lc = LifecycleOutcome::Committed(winner("win", 0.3));
        let v = DriftVerdict {
            drifted: true,
            winner_prob: 0.3,
            challenger: Some(("rival".into(), 0.7)),
        };
        let (a, _) = decide_campaign_action(&lc, Some(&v), false);
        assert_eq!(a, CampaignAction::NoAction);
    }

    #[test]
    fn advisory_or_noaction_never_spawns() {
        for lc in [
            LifecycleOutcome::NoAction,
            LifecycleOutcome::Advisory(winner("w", 0.99)),
            LifecycleOutcome::Committed(winner("w", 0.99)),
        ] {
            let (a, _) = decide_campaign_action(&lc, None, true);
            assert_eq!(a, CampaignAction::NoAction);
        }
    }

    // ── orchestration with fakes ──────────────────────────────────────────────

    use crate::bandit::tests_support::{FakeReader, count_metric};
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct FakeSpawner {
        campaign: BanditCampaign,
        claims: AtomicUsize,
        opens: AtomicUsize,
        finalizes: AtomicUsize,
        last_control: Mutex<Option<String>>,
        // When false, try_claim_spawn returns None (slot refused).
        claim_succeeds: bool,
    }

    impl FakeSpawner {
        fn new(c: BanditCampaign, claim_succeeds: bool) -> Self {
            Self {
                campaign: c,
                claims: AtomicUsize::new(0),
                opens: AtomicUsize::new(0),
                finalizes: AtomicUsize::new(0),
                last_control: Mutex::new(None),
                claim_succeeds,
            }
        }
    }

    #[async_trait::async_trait]
    impl CampaignSpawner for FakeSpawner {
        async fn load(&self, _id: Uuid) -> Result<BanditCampaign, anyhow::Error> {
            Ok(self.campaign.clone())
        }
        async fn try_claim_spawn(
            &self,
            _id: Uuid,
            _v: i64,
        ) -> Result<Option<BanditCampaign>, anyhow::Error> {
            self.claims.fetch_add(1, Ordering::SeqCst);
            if self.claim_succeeds {
                let mut c = self.campaign.clone();
                c.iterations_spawned += 1;
                c.version += 1;
                Ok(Some(c))
            } else {
                Ok(None)
            }
        }
        async fn open_next_iteration(
            &self,
            _exp: &RunningExperiment,
            _c: &BanditCampaign,
            new_control: &str,
        ) -> Result<(), anyhow::Error> {
            self.opens.fetch_add(1, Ordering::SeqCst);
            *self.last_control.lock().unwrap() = Some(new_control.to_string());
            Ok(())
        }
        async fn finalize(&self, _c: &BanditCampaign) -> Result<(), anyhow::Error> {
            self.finalizes.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    fn campaign_exp(env: Uuid, metric_id: Uuid, campaign_id: Uuid) -> RunningExperiment {
        use stitchd_core::experimentation::bandit::{
            BanditAlgorithm, BanditConfig, ExperimentMode, LifecyclePolicy, PropagationMode,
            RewardObjective,
        };
        let cfg = BanditConfig {
            algorithm: BanditAlgorithm::ThompsonSampling,
            propagation_mode: PropagationMode::Static,
            min_exploration_bp: 100,
            objective: RewardObjective::Scalar { metric_id },
            lifecycle_policy: LifecyclePolicy::AutoRollout,
            convergence_prob_threshold: 0.9,
        };
        let mut exp = crate::bandit::tests_support::running_bandit_with(
            Some(cfg),
            ExperimentMode::Bandit,
            env,
            vec!["control".into(), "win".into()],
            vec![metric_id],
        );
        exp.bandit_campaign_id = Some(campaign_id);
        exp
    }

    #[tokio::test]
    async fn non_campaign_experiment_is_no_action() {
        let env = Uuid::new_v4();
        let metric_id = Uuid::new_v4();
        let mut exp = campaign_exp(env, metric_id, Uuid::new_v4());
        exp.bandit_campaign_id = None; // not campaign-owned
        let reader = FakeReader::with_conversions(
            metric_id,
            vec![("control", 1000, 100), ("win", 1000, 400)],
        );
        let spawner = FakeSpawner::new(campaign(0, 3, BanditCampaignStatus::Active), true);
        let recorder = crate::bandit::tests_support::FakeRecorder::default();
        let mut metrics = HashMap::new();
        metrics.insert(metric_id, count_metric(metric_id, env));
        let now = Utc::now();

        let out = run_campaign_spawn(
            &reader,
            &spawner,
            &recorder,
            &exp,
            &metrics,
            &LifecycleOutcome::RolledOut(winner("win", 0.99)),
            now,
            now,
        )
        .await
        .unwrap();
        assert_eq!(out, SpawnOutcome::NoAction);
        assert_eq!(spawner.claims.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn rollout_spawns_next_iteration_winner_as_control() {
        let env = Uuid::new_v4();
        let metric_id = Uuid::new_v4();
        let campaign_id = Uuid::new_v4();
        let exp = campaign_exp(env, metric_id, campaign_id);
        let reader = FakeReader::with_conversions(
            metric_id,
            vec![("control", 1000, 100), ("win", 1000, 400)],
        );
        let spawner = FakeSpawner::new(campaign(0, 3, BanditCampaignStatus::Active), true);
        let recorder = crate::bandit::tests_support::FakeRecorder::default();
        let mut metrics = HashMap::new();
        metrics.insert(metric_id, count_metric(metric_id, env));
        let now = Utc::now();

        let out = run_campaign_spawn(
            &reader,
            &spawner,
            &recorder,
            &exp,
            &metrics,
            &LifecycleOutcome::RolledOut(winner("win", 0.99)),
            now,
            now,
        )
        .await
        .unwrap();

        assert!(matches!(out, SpawnOutcome::Spawned { .. }));
        assert_eq!(spawner.claims.load(Ordering::SeqCst), 1);
        assert_eq!(spawner.opens.load(Ordering::SeqCst), 1);
        assert_eq!(spawner.last_control.lock().unwrap().as_deref(), Some("win"));
        assert_eq!(recorder.last_action().as_deref(), Some("spawn_iteration"));
    }

    #[tokio::test]
    async fn idempotent_when_claim_refused() {
        let env = Uuid::new_v4();
        let metric_id = Uuid::new_v4();
        let campaign_id = Uuid::new_v4();
        let exp = campaign_exp(env, metric_id, campaign_id);
        let reader = FakeReader::with_conversions(
            metric_id,
            vec![("control", 1000, 100), ("win", 1000, 400)],
        );
        // claim_succeeds=false → the same convergence does not double-spawn.
        let spawner = FakeSpawner::new(campaign(1, 3, BanditCampaignStatus::Active), false);
        let recorder = crate::bandit::tests_support::FakeRecorder::default();
        let mut metrics = HashMap::new();
        metrics.insert(metric_id, count_metric(metric_id, env));
        let now = Utc::now();

        let out = run_campaign_spawn(
            &reader,
            &spawner,
            &recorder,
            &exp,
            &metrics,
            &LifecycleOutcome::RolledOut(winner("win", 0.99)),
            now,
            now,
        )
        .await
        .unwrap();

        assert!(matches!(out, SpawnOutcome::NotSpawned { .. }));
        assert_eq!(spawner.claims.load(Ordering::SeqCst), 1, "claim attempted");
        assert_eq!(
            spawner.opens.load(Ordering::SeqCst),
            0,
            "no iteration opened"
        );
    }

    #[tokio::test]
    async fn cap_reached_finalizes_campaign() {
        let env = Uuid::new_v4();
        let metric_id = Uuid::new_v4();
        let campaign_id = Uuid::new_v4();
        let exp = campaign_exp(env, metric_id, campaign_id);
        let reader = FakeReader::with_conversions(
            metric_id,
            vec![("control", 1000, 100), ("win", 1000, 400)],
        );
        // spawned == max → can_spawn() == false → Finalize.
        let spawner = FakeSpawner::new(campaign(3, 3, BanditCampaignStatus::Active), true);
        let recorder = crate::bandit::tests_support::FakeRecorder::default();
        let mut metrics = HashMap::new();
        metrics.insert(metric_id, count_metric(metric_id, env));
        let now = Utc::now();

        let out = run_campaign_spawn(
            &reader,
            &spawner,
            &recorder,
            &exp,
            &metrics,
            &LifecycleOutcome::RolledOut(winner("win", 0.99)),
            now,
            now,
        )
        .await
        .unwrap();

        assert!(matches!(out, SpawnOutcome::NotSpawned { .. }));
        assert_eq!(spawner.finalizes.load(Ordering::SeqCst), 1);
        assert_eq!(
            spawner.claims.load(Ordering::SeqCst),
            0,
            "no claim past cap"
        );
    }
}
