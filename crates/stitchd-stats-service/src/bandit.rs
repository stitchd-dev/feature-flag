//! Bandit static-reallocation pass (FR4 static-rewrite path, FR5 history).
//!
//! For every RUNNING **bandit-mode** experiment whose `propagation_mode` is
//! [`PropagationMode::Static`], the 60-min stats-service compute tick:
//!
//! 1. reads per-variant **sufficient statistics** for the experiment's objective
//!    metric(s) from ClickHouse (reusing the [`crate::compute::CellReader`] the
//!    main stats pass already uses), summed across the experiment's context
//!    types so the single global rule allocation is driven by the whole signal;
//! 2. builds one [`MetricRewards`] per objective metric (Beta-Binomial for
//!    conversion / funnel, Normal-Normal for numeric, delta-method for ratio);
//! 3. computes raw per-variant weights via the configured
//!    [`BanditAlgorithm`] (Thompson / epsilon-greedy / UCB) over those reward
//!    posteriors — multi-objective objectives go through
//!    [`reward_arms`]/[`allocate_exploitable`] first — and normalises them to a
//!    basis-point [`RolloutDistribution`] with the configured exploration floor;
//! 4. writes the new allocation back to the bound flag rule via the privileged
//!    `ApplyBanditAllocation` RPC (which bypasses the whole-flag human lock for
//!    the owning experiment); and
//! 5. records ONE [`bandit_allocation_runs`](crate) row per experiment per tick.
//!
//! The **eval path is untouched**: weights land in PG and propagate via the
//! existing flag snapshot + SDK polling (the static-rewrite invariant).
//!
//! ## Determinism
//!
//! Thompson Sampling's RNG seed is derived deterministically from the
//! experiment id, iteration id, arm count and the tick timestamp (truncated to
//! the tick bucket) — never from wall-clock entropy — so a given tick is
//! reproducible. See [`derive_seed`].
//!
//! ## Skip, never error
//!
//! Every recoverable condition (not a bandit, not static, insufficient data,
//! normalize error, ineligible) is reported as a **skip** and recorded with
//! `action = "skip"` / `outcome = SKIPPED` — it never aborts the tick. Only an
//! actual ClickHouse read error or PG insert error propagates.
//!
//! ## Idempotency / restart-safety
//!
//! Exactly one `bandit_allocation_runs` row is written per experiment per tick.
//! A tick that crashes mid-way leaves no half-state: the next tick simply
//! recomputes from the live sufficient stats and the live flag version. No long
//! locks are held.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde_json::Value;
use uuid::Uuid;

use stitchd_core::experimentation::bandit::{
    BanditAlgorithm, BanditConfig, ExperimentMode, GoalDirection as BanditGoal, MetricRewards,
    PropagationMode, RewardObjective, RewardPosterior, allocate_exploitable,
    apply_exploitable_mask, epsilon_greedy_weights, normalize_to_distribution, reward_arms,
    thompson_weights, ucb_weights,
};
use stitchd_core::experimentation::stats::MetricType;
use stitchd_core::experimentation::stats::sequential::RatioGroupStats;
use stitchd_core::metric::{GoalDirection as MetricGoal, MetricDefinition, MetricKind};
use stitchd_core::rollout::{RolloutAllocation, RolloutDistribution};

use crate::compute::{
    AggCell, CellReader, count_variant_stats, funnel_variant_stats, metric_type_for,
    numeric_variant_stats,
};
use crate::scheduler::RunningExperiment;

/// Number of Monte-Carlo draws Thompson Sampling takes per arm.
const THOMPSON_SAMPLES: usize = 10_000;

/// Minimum per-arm sample size below which reallocation is skipped (insufficient
/// data). A bandit must not shift traffic off near-zero evidence.
const MIN_ARM_SAMPLE: u64 = 30;

/// The decision recorded for a single reallocation pass over one experiment.
///
/// Maps onto a `bandit_allocation_runs` row (`action`, `outcome`, `detail`).
#[derive(Debug, Clone, PartialEq)]
pub enum BanditRunOutcome {
    /// The experiment is not an eligible static-propagation bandit — no row is
    /// written (the pass is a pure no-op for non-bandit experiments).
    NotApplicable,
    /// A `reallocate` was computed and the `ApplyBanditAllocation` RPC returned
    /// APPLIED. Carries the written distribution + resolved target + new version.
    Applied {
        /// The newly-written allocation.
        allocation: RolloutDistribution,
        /// The bound target the RPC rewrote.
        resolved_target: String,
        /// The post-write flag version.
        new_version: u64,
    },
    /// The pass produced no write — either it decided to skip pre-RPC
    /// (insufficient data / normalize error / ineligible) or the RPC itself
    /// returned SKIPPED. A `skip` row is recorded.
    Skipped {
        /// Human-readable skip reason (recorded in `detail`).
        reason: String,
    },
    /// The `ApplyBanditAllocation` RPC returned FAILED (incl. version conflict).
    /// A `reallocate`/`failed` row is recorded; the tick keeps advancing.
    Failed {
        /// The computed allocation that failed to apply.
        allocation: RolloutDistribution,
        /// Failure detail (recorded in `detail`).
        detail: String,
        /// True when the failure was an optimistic-concurrency conflict.
        version_conflict: bool,
    },
}

/// The pure decision: given the experiment config + per-objective metric reward
/// posteriors + a deterministic seed, either compute the new distribution or
/// decide to skip.
#[derive(Debug, Clone, PartialEq)]
pub enum AllocationDecision {
    /// Reallocate to this distribution.
    Reallocate(RolloutDistribution),
    /// Skip with this reason.
    Skip(String),
}

// ── Eligibility ──────────────────────────────────────────────────────────────

/// Is this experiment eligible for the static reallocation pass?
///
/// Requires bandit mode, a bandit config whose `propagation_mode` is
/// [`PropagationMode::Static`]. (The experiment is already known to be running —
/// the scheduler only lists running experiments.)
#[must_use]
pub fn is_eligible(exp: &RunningExperiment) -> bool {
    if exp.experiment_mode != ExperimentMode::Bandit {
        return false;
    }
    match &exp.bandit_config {
        Some(cfg) => cfg.propagation_mode == PropagationMode::Static,
        None => false,
    }
}

/// Is this experiment eligible for the **real-time** snapshot-resident pass?
///
/// Requires bandit mode + a bandit config whose `propagation_mode` is
/// [`PropagationMode::Realtime`]. The realtime path refreshes the bound rule's
/// snapshot-resident model parameters instead of rewriting a static %.
#[must_use]
pub fn is_eligible_realtime(exp: &RunningExperiment) -> bool {
    if exp.experiment_mode != ExperimentMode::Bandit {
        return false;
    }
    match &exp.bandit_config {
        Some(cfg) => cfg.propagation_mode == PropagationMode::Realtime,
        None => false,
    }
}

// ── Deterministic seed ───────────────────────────────────────────────────────

/// Derive a stable `u64` RNG seed for a reallocation tick.
///
/// Combines the experiment id, iteration id, arm count and the tick's timestamp
/// (truncated to the whole second) via a simple FNV-1a-style mix over the bytes,
/// so the seed is reproducible for a given (experiment, iteration, tick) and
/// changes tick-to-tick — never drawn from wall-clock entropy or a real RNG.
#[must_use]
pub fn derive_seed(
    experiment_id: Uuid,
    iteration_id: Uuid,
    n_arms: usize,
    tick: DateTime<Utc>,
) -> u64 {
    // FNV-1a 64-bit.
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    let mut mix = |bytes: &[u8]| {
        for &b in bytes {
            hash ^= u64::from(b);
            hash = hash.wrapping_mul(0x1000_0000_01b3);
        }
    };
    mix(experiment_id.as_bytes());
    mix(iteration_id.as_bytes());
    mix(&(n_arms as u64).to_le_bytes());
    mix(&tick.timestamp().to_le_bytes());
    hash
}

// ── Goal-direction mapping ───────────────────────────────────────────────────

/// Map a metric's [`MetricGoal`] onto the bandit's [`BanditGoal`].
///
/// `Neutral` has no optimisation direction, so a neutral objective metric makes
/// the bandit ineligible (returns `None`).
#[must_use]
pub fn map_goal(goal: MetricGoal) -> Option<BanditGoal> {
    match goal {
        MetricGoal::Increase => Some(BanditGoal::Increase),
        MetricGoal::Decrease => Some(BanditGoal::Decrease),
        MetricGoal::Neutral => None,
    }
}

// ── Objective metric resolution ──────────────────────────────────────────────

/// The objective metric ids referenced by a [`RewardObjective`] (in a stable
/// order). Used to know which metrics to read sufficient stats for.
#[must_use]
pub fn objective_metric_ids(objective: &RewardObjective) -> Vec<Uuid> {
    match objective {
        RewardObjective::Scalar { metric_id } => vec![*metric_id],
        RewardObjective::Scalarized { weights } => weights.iter().map(|w| w.metric_id).collect(),
        RewardObjective::Constrained {
            primary_metric_id,
            constraints,
        } => {
            let mut ids = vec![*primary_metric_id];
            ids.extend(constraints.iter().map(|c| c.metric_id));
            ids
        }
    }
}

// ── Reward-posterior construction (pure) ─────────────────────────────────────

/// Build a [`RewardPosterior`] for one variant from its summed sufficient stats,
/// keyed by the metric family.
#[must_use]
fn posterior_for(
    mt: MetricType,
    sample_size: u64,
    agg: Option<AggCell>,
    ratio: Option<RatioGroupStats>,
) -> RewardPosterior {
    match mt {
        MetricType::Count => {
            let successes = agg.map_or(0, |c| c.successes);
            RewardPosterior::Conversion(count_variant_stats(sample_size, successes))
        }
        MetricType::Funnel => {
            let successes = agg.map_or(0, |c| c.successes);
            RewardPosterior::Funnel(funnel_variant_stats(sample_size, successes))
        }
        MetricType::Numeric => {
            if let Some(g) = ratio {
                RewardPosterior::Ratio(g)
            } else {
                let cell = agg.unwrap_or(AggCell {
                    event_n: 0,
                    successes: 0,
                    value_sum: 0.0,
                    value_sq_sum: 0.0,
                });
                RewardPosterior::Numeric(numeric_variant_stats(
                    sample_size,
                    cell.value_sum,
                    cell.value_sq_sum,
                ))
            }
        }
        // Percentile metrics have no posterior-sampling family; treat the point
        // (carried as a Numeric mean) as a zero-variance numeric reward.
        MetricType::Percentile => {
            let cell = agg.unwrap_or(AggCell {
                event_n: 0,
                successes: 0,
                value_sum: 0.0,
                value_sq_sum: 0.0,
            });
            RewardPosterior::Numeric(numeric_variant_stats(
                sample_size,
                cell.value_sum,
                cell.value_sq_sum,
            ))
        }
    }
}

/// The minimum per-arm sample size across all arms in a [`MetricRewards`] set.
/// Returns 0 for an empty set (which the caller treats as insufficient).
#[must_use]
fn min_arm_n(metrics: &[MetricRewards]) -> u64 {
    metrics
        .iter()
        .flat_map(|m| m.arms.iter())
        .map(|(_, p)| p.arm_n())
        .min()
        .unwrap_or(0)
}

// ── Algorithm dispatch (pure) ────────────────────────────────────────────────

/// Compute raw per-arm weights for a single-objective allocation by dispatching
/// on the configured algorithm. `arms`/`goal` come from [`reward_arms`].
#[must_use]
fn raw_weights_for(
    algorithm: &BanditAlgorithm,
    arms: &[stitchd_core::experimentation::bandit::BanditArm],
    goal: BanditGoal,
    exploitable: &[bool],
    seed: u64,
) -> Vec<(String, f64)> {
    // Use the robust exploitable-merge path for every allocator so a constrained
    // winner-take-all allocator never starves (matches the Phase-2 guidance).
    match algorithm {
        BanditAlgorithm::ThompsonSampling => {
            // Thompson is proportional; the post-hoc mask is sufficient and keeps
            // full uncertainty sampling across all arms.
            let raw = thompson_weights(arms, goal, THOMPSON_SAMPLES, seed);
            apply_exploitable_mask(&raw, exploitable)
        }
        BanditAlgorithm::EpsilonGreedy { epsilon } => {
            let eps = *epsilon;
            allocate_exploitable(arms, exploitable, |eligible| {
                epsilon_greedy_weights(eligible, goal, eps)
            })
        }
        BanditAlgorithm::Ucb { c } => {
            let cc = *c;
            allocate_exploitable(arms, exploitable, |eligible| {
                ucb_weights(eligible, goal, cc)
            })
        }
        // Contextual bandits use the real-time snapshot path, not static
        // reallocation; treated as no raw weights (caller skips).
        BanditAlgorithm::Contextual(_) => Vec::new(),
    }
}

/// The pure allocation decision: from the experiment's [`BanditConfig`], the
/// per-objective metric reward posteriors, and a deterministic seed, either
/// produce the new [`RolloutDistribution`] or decide to skip.
///
/// Skips when:
/// * `metrics` is empty (no objective metric resolved),
/// * any arm's sample size is below [`MIN_ARM_SAMPLE`] (insufficient data),
/// * the algorithm is contextual (real-time path, not static), or
/// * the basis-point normaliser rejects the floor / arm count.
#[must_use]
pub fn decide_allocation(
    config: &BanditConfig,
    metrics: &[MetricRewards],
    seed: u64,
) -> AllocationDecision {
    if metrics.is_empty() {
        return AllocationDecision::Skip("no objective metric reward data".to_string());
    }

    let min_n = min_arm_n(metrics);
    if min_n < MIN_ARM_SAMPLE {
        return AllocationDecision::Skip(format!(
            "insufficient data: smallest arm n={min_n} < {MIN_ARM_SAMPLE}"
        ));
    }

    if matches!(config.algorithm, BanditAlgorithm::Contextual(_)) {
        return AllocationDecision::Skip(
            "contextual algorithm uses the real-time path, not static reallocation".to_string(),
        );
    }

    let (arms, goal, exploitable) = reward_arms(&config.objective, metrics);
    if arms.is_empty() {
        return AllocationDecision::Skip("reward_arms produced no arms".to_string());
    }

    let raw = raw_weights_for(&config.algorithm, &arms, goal, &exploitable, seed);
    if raw.is_empty() {
        return AllocationDecision::Skip("algorithm produced no weights".to_string());
    }

    match normalize_to_distribution(&raw, config.min_exploration_bp) {
        Ok(dist) => AllocationDecision::Reallocate(dist),
        Err(e) => AllocationDecision::Skip(format!("normalize failed: {e}")),
    }
}

// ── Real-time model decision (pure) ──────────────────────────────────────────

/// The pure decision for the real-time propagation path: build the
/// snapshot-resident [`RealtimeBanditModel`](stitchd_proto::flags::v1::RealtimeBanditModel)
/// parameters or decide to skip.
#[derive(Debug, Clone, PartialEq)]
pub enum RealtimeModelDecision {
    /// Refresh the bound rule's snapshot-resident model to these params.
    Refresh(stitchd_proto::flags::v1::RealtimeBanditModel),
    /// Skip with this reason.
    Skip(String),
}

/// The minimum per-arm sample size below which the realtime model is not
/// refreshed (insufficient evidence — same floor as the static path).
fn realtime_min_arm_n(rewards: &MetricRewards) -> u64 {
    rewards
        .arms
        .iter()
        .map(|(_, p)| p.arm_n())
        .min()
        .unwrap_or(0)
}

/// Convert one [`RewardPosterior`] into proto [`VariantPosterior`] params for a
/// given family. Beta family carries `(alpha, beta)`; Normal family carries
/// `(mu, sigma2)`. Pure.
fn variant_posterior_proto(
    variant_key: &str,
    family: stitchd_proto::flags::v1::RewardFamily,
    post: &RewardPosterior,
) -> stitchd_proto::flags::v1::VariantPosterior {
    use stitchd_proto::flags::v1::{RewardFamily as PF, VariantPosterior as PV};
    match family {
        PF::Beta => {
            // Beta(alpha, beta) from the count/funnel posterior mean + n.
            let n = post.arm_n() as f64;
            let mean = post.mean().clamp(0.0, 1.0);
            let successes = (mean * n).round();
            PV {
                variant_key: variant_key.to_string(),
                alpha: 1.0 + successes,
                beta: 1.0 + (n - successes).max(0.0),
                mu: 0.0,
                sigma2: 0.0,
            }
        }
        // Normal(mu, sigma2): mean + a variance proxy. We approximate the
        // posterior variance as (point variance / n) using the mean only when no
        // richer stat is exposed; the engine collapses sigma2<=0 to a point draw.
        PF::Normal | PF::Unspecified => {
            let mean = post.mean();
            // A small positive variance keeps Thompson exploration alive; scaled
            // down by n so a well-sampled arm sharpens.
            let n = (post.arm_n() as f64).max(1.0);
            PV {
                variant_key: variant_key.to_string(),
                alpha: 0.0,
                beta: 0.0,
                mu: mean,
                sigma2: (mean.abs().max(1.0)) / n,
            }
        }
    }
}

/// The pure real-time model decision: from the experiment's [`BanditConfig`] and
/// a single objective metric's reward posteriors, build the snapshot-resident
/// model params or decide to skip.
///
/// Skips when there is no reward data, any arm has insufficient evidence, the
/// objective is multi-objective (deferred to a later phase for the realtime
/// path), or the metric family has no posterior-sampling form.
#[must_use]
pub fn decide_realtime_model(
    config: &BanditConfig,
    metrics: &[MetricRewards],
    salt: &str,
    unit_context_type: &str,
) -> RealtimeModelDecision {
    use stitchd_proto::flags::v1::{
        BanditGoalDirection as PG, RealtimeBanditModel as PModel, RewardFamily as PF,
    };

    // Single-objective scalar only for the non-contextual realtime model; multi
    // objective is folded in once the contextual phase lands.
    if !matches!(config.objective, RewardObjective::Scalar { .. }) {
        return RealtimeModelDecision::Skip(
            "realtime model currently supports a single scalar objective".to_string(),
        );
    }
    let Some(rewards) = metrics.first() else {
        return RealtimeModelDecision::Skip("no objective metric reward data".to_string());
    };
    if rewards.arms.is_empty() {
        return RealtimeModelDecision::Skip("no arms in reward posteriors".to_string());
    }
    let min_n = realtime_min_arm_n(rewards);
    if min_n < MIN_ARM_SAMPLE {
        return RealtimeModelDecision::Skip(format!(
            "insufficient data: smallest arm n={min_n} < {MIN_ARM_SAMPLE}"
        ));
    }

    // Map the posterior family to the wire family.
    let family = match &rewards.arms[0].1 {
        RewardPosterior::Conversion(_) | RewardPosterior::Funnel(_) => PF::Beta,
        RewardPosterior::Numeric(_) | RewardPosterior::Ratio(_) => PF::Normal,
    };
    let goal = match rewards.goal {
        BanditGoal::Increase => PG::Increase,
        BanditGoal::Decrease => PG::Decrease,
    };

    let variants = rewards
        .arms
        .iter()
        .map(|(vk, post)| variant_posterior_proto(vk, family, post))
        .collect();

    RealtimeModelDecision::Refresh(PModel {
        salt: salt.to_string(),
        unit_context_type: unit_context_type.to_string(),
        family: family as i32,
        goal: goal as i32,
        variants,
        contextual: None,
    })
}

// ── Contextual model decision (pure) ─────────────────────────────────────────

/// One assigned unit's contextual training row: its arm, its feature value
/// (raw string from event properties, `""` when absent), and its reward.
pub type ContextualRow = (String, String, f64);

/// The pure decision for the CONTEXTUAL realtime path: from the experiment's
/// contextual feature set + the per-unit `(variant, feature_value, reward)`
/// training rows, fit a per-variant ridge linear model and assemble the
/// snapshot-resident contextual [`RealtimeBanditModel`]. Or decide to skip.
///
/// All declared features are encoded as a SINGLE numeric passthrough each (the
/// `ContextualConfig.features` carries only names; richer encodings are a future
/// extension). The design vector is intercept-first, shared across variants. A
/// variant with no rows fits to a zero vector (it simply predicts the intercept
/// baseline = 0). Pure: the only work is `encode_features` + `fit_ridge`.
///
/// Skips when there are no features, no rows, or fewer than [`MIN_ARM_SAMPLE`]
/// rows on the smallest-sampled arm (insufficient evidence — same floor as the
/// non-contextual path).
#[must_use]
pub fn decide_contextual_model(
    feature_names: &[String],
    rows: &[ContextualRow],
    goal: BanditGoal,
    salt: &str,
    unit_context_type: &str,
) -> RealtimeModelDecision {
    use stitchd_core::experimentation::bandit::contextual::{
        ContextualModel, FeatureEncoding, FeatureSpec, VariantCoefficients, encode_features,
        fit_design_inverse, fit_ridge,
    };
    use stitchd_proto::flags::v1::{
        BanditGoalDirection as PG, RealtimeBanditModel as PModel, RewardFamily as PF,
        feature_encoding::Kind as PKind,
    };

    if feature_names.is_empty() {
        return RealtimeModelDecision::Skip("contextual model has no features".to_string());
    }
    if rows.is_empty() {
        return RealtimeModelDecision::Skip("no contextual training rows".to_string());
    }

    // Build the feature specs: each declared name is a numeric passthrough on a
    // parameter of the unit context type.
    let specs: Vec<FeatureSpec> = feature_names
        .iter()
        .map(|name| FeatureSpec {
            context_type: unit_context_type.to_string(),
            parameter: name.clone(),
            encoding: FeatureEncoding::Numeric,
        })
        .collect();

    // Group rows by variant; build per-variant design rows. A resolver over a
    // single (feature_value) row: for a single numeric feature this is the
    // parameter value. With multiple features all sharing one row value we map
    // the row's `feature_value` onto the FIRST feature and 0 for the rest — the
    // CH query returns one feature column, so multi-feature contextual fits are
    // a future extension; today the first declared feature carries the signal.
    let mut by_variant: HashMap<String, Vec<(Vec<f64>, f64)>> = HashMap::new();
    for (variant, feature_value, reward) in rows {
        let resolver = SingleFeatureResolver {
            context_type: unit_context_type,
            parameter: &specs[0].parameter,
            value: feature_value,
        };
        let fv = encode_features(&specs, &resolver);
        by_variant
            .entry(variant.clone())
            .or_default()
            .push((fv, *reward));
    }

    // Insufficient-evidence floor: the smallest-sampled arm must clear it.
    let min_n = by_variant.values().map(Vec::len).min().unwrap_or(0) as u64;
    if min_n < MIN_ARM_SAMPLE {
        return RealtimeModelDecision::Skip(format!(
            "insufficient contextual data: smallest arm n={min_n} < {MIN_ARM_SAMPLE}"
        ));
    }

    // Ridge λ: a light penalty keeps the normal-equations solve well-conditioned
    // without materially shrinking a real signal.
    const RIDGE_LAMBDA: f64 = 1.0;
    let mut variants: Vec<VariantCoefficients> = by_variant
        .into_iter()
        .map(|(variant_key, design)| {
            let coeffs = fit_ridge(&design, RIDGE_LAMBDA);
            let a_inv = fit_design_inverse(&design, RIDGE_LAMBDA);
            VariantCoefficients {
                variant_key,
                coeffs,
                a_inv,
            }
        })
        .collect();
    // Stable order so the model is deterministic across ticks.
    variants.sort_by(|a, b| a.variant_key.cmp(&b.variant_key));

    let cmodel = ContextualModel {
        features: specs,
        variants,
    };

    // Map the contextual model onto the proto via the same shape the flag-service
    // mapping uses (here we build the proto directly to avoid a cross-crate dep).
    let proto_features = cmodel
        .features
        .iter()
        .map(|f| stitchd_proto::flags::v1::FeatureSpec {
            context_type: f.context_type.clone(),
            parameter: f.parameter.clone(),
            encoding: Some(stitchd_proto::flags::v1::FeatureEncoding {
                kind: Some(match &f.encoding {
                    FeatureEncoding::Numeric => {
                        PKind::Numeric(stitchd_proto::flags::v1::NumericEncoding {})
                    }
                    FeatureEncoding::OneHot { categories } => {
                        PKind::OneHot(stitchd_proto::flags::v1::OneHotEncoding {
                            categories: categories.clone(),
                        })
                    }
                }),
            }),
        })
        .collect();
    let proto_variants = cmodel
        .variants
        .iter()
        .map(|v| stitchd_proto::flags::v1::VariantCoefficients {
            variant_key: v.variant_key.clone(),
            coeffs: v.coeffs.clone(),
            a_inv: v.a_inv.clone().unwrap_or_default(),
        })
        .collect();

    let goal_pg = match goal {
        BanditGoal::Increase => PG::Increase,
        BanditGoal::Decrease => PG::Decrease,
    };

    RealtimeModelDecision::Refresh(PModel {
        salt: salt.to_string(),
        unit_context_type: unit_context_type.to_string(),
        // Contextual models predict a continuous reward → Normal family wire tag
        // (the engine reads `contextual` first, so the family is informational).
        family: PF::Normal as i32,
        goal: goal_pg as i32,
        variants: vec![],
        contextual: Some(stitchd_proto::flags::v1::ContextualModel {
            features: proto_features,
            variants: proto_variants,
        }),
    })
}

/// A [`FeatureResolver`](stitchd_core::experimentation::bandit::contextual::FeatureResolver)
/// over a single `(context_type, parameter) → value` pair (one training row's
/// feature value). Any other lookup misses (→ baseline / `0.0`).
struct SingleFeatureResolver<'a> {
    context_type: &'a str,
    parameter: &'a str,
    value: &'a str,
}

impl stitchd_core::experimentation::bandit::contextual::FeatureResolver
    for SingleFeatureResolver<'_>
{
    fn resolve(&self, context_type: &str, parameter: &str) -> Option<&str> {
        if context_type == self.context_type && parameter == self.parameter {
            Some(self.value)
        } else {
            None
        }
    }
}

// ── I/O seams (thin traits) ──────────────────────────────────────────────────

/// Result of applying an allocation via the privileged RPC.
#[derive(Debug, Clone, PartialEq)]
pub enum ApplyResult {
    /// APPLIED: the bound rule was rewritten.
    Applied {
        /// The bound target that was rewritten.
        resolved_target: String,
        /// The post-write flag version.
        new_version: u64,
    },
    /// SKIPPED: the server found the experiment ineligible (recoverable no-op).
    Skipped {
        /// Server-supplied skip detail.
        detail: String,
    },
    /// FAILED: the write failed (incl. optimistic-concurrency conflict).
    Failed {
        /// Failure detail.
        detail: String,
        /// True for a version conflict.
        version_conflict: bool,
    },
}

/// Thin async seam over the `ApplyBanditAllocation` RPC so the orchestrator is
/// unit-testable without a live experimentation-service.
#[async_trait::async_trait]
pub trait AllocationApplier: Send + Sync {
    /// Apply `allocation` to the bound rule of `experiment_id`.
    async fn apply(
        &self,
        experiment_id: Uuid,
        allocation: &RolloutDistribution,
    ) -> Result<ApplyResult, anyhow::Error>;

    /// Refresh the bound rule's snapshot-resident real-time bandit `model`
    /// (realtime propagation path). `allocation` still rides along as the static
    /// fallback for contexts that lack the diversion unit.
    async fn apply_realtime(
        &self,
        experiment_id: Uuid,
        allocation: &RolloutDistribution,
        model: stitchd_proto::flags::v1::RealtimeBanditModel,
    ) -> Result<ApplyResult, anyhow::Error>;
}

/// Thin async seam over the `bandit_allocation_runs` PG insert.
#[async_trait::async_trait]
pub trait RunRecorder: Send + Sync {
    /// Insert exactly one history row for this tick.
    #[allow(clippy::too_many_arguments)]
    async fn record(
        &self,
        experiment_id: Uuid,
        iteration_id: Uuid,
        action: &str,
        old_allocation: Option<Value>,
        new_allocation: Option<Value>,
        outcome: &str,
        detail: Option<String>,
    ) -> Result<(), anyhow::Error>;
}

// ── Production I/O impls ─────────────────────────────────────────────────────

/// Production [`AllocationApplier`] over the experimentation-service gRPC client.
///
/// Passes `expected_version = 0` ("look it up server-side") since the
/// stats-service does not pre-fetch the flag version; the server resolves the
/// bound target and applies under its own optimistic-version read.
pub struct GrpcAllocationApplier {
    client: std::sync::Arc<
        tokio::sync::Mutex<
            stitchd_proto::experiments::v1::experimentation_service_client::ExperimentationServiceClient<
                tonic::transport::Channel,
            >,
        >,
    >,
}

impl GrpcAllocationApplier {
    /// Wrap a shared experimentation-service client.
    #[must_use]
    pub fn new(
        client: std::sync::Arc<
            tokio::sync::Mutex<
                stitchd_proto::experiments::v1::experimentation_service_client::ExperimentationServiceClient<
                    tonic::transport::Channel,
                >,
            >,
        >,
    ) -> Self {
        Self { client }
    }
}

impl GrpcAllocationApplier {
    /// Shared dispatch for both the static and realtime paths. `model = None` is
    /// the static-rewrite path; `Some(model)` refreshes the snapshot-resident
    /// model on the bound rule.
    async fn dispatch(
        &self,
        experiment_id: Uuid,
        allocation: &RolloutDistribution,
        model: Option<stitchd_proto::flags::v1::RealtimeBanditModel>,
    ) -> Result<ApplyResult, anyhow::Error> {
        use stitchd_proto::experiments::v1::{
            ApplyBanditAllocationRequest, BanditAllocationBucket, BanditAllocationOutcome,
        };

        let allocations: Vec<BanditAllocationBucket> = allocation
            .allocations
            .iter()
            .map(|a| BanditAllocationBucket {
                variant_key: a.variant_key.clone(),
                weight_bp: a.percentage_bp,
            })
            .collect();

        let req = ApplyBanditAllocationRequest {
            experiment_id: experiment_id.to_string(),
            allocations,
            expected_version: 0,
            realtime_model: model,
        };

        let resp = {
            let mut client = self.client.lock().await;
            client.apply_bandit_allocation(req).await
        };

        match resp {
            Ok(resp) => {
                let r = resp.into_inner();
                match BanditAllocationOutcome::try_from(r.outcome) {
                    Ok(BanditAllocationOutcome::Applied) => Ok(ApplyResult::Applied {
                        resolved_target: r.resolved_target,
                        new_version: r.new_version,
                    }),
                    Ok(BanditAllocationOutcome::Skipped) => Ok(ApplyResult::Skipped {
                        detail: if r.detail.is_empty() {
                            "server skipped".to_string()
                        } else {
                            r.detail
                        },
                    }),
                    // Failed / Unspecified / unknown → treat as failed.
                    _ => Ok(ApplyResult::Failed {
                        detail: if r.detail.is_empty() {
                            "server reported failure".to_string()
                        } else {
                            r.detail
                        },
                        version_conflict: r.version_conflict,
                    }),
                }
            }
            // gRPC transport error: surface as a recoverable failure so the
            // orchestrator records a `failed` row and the tick keeps advancing.
            Err(status) => Ok(ApplyResult::Failed {
                detail: format!("ApplyBanditAllocation transport error: {status}"),
                version_conflict: status.code() == tonic::Code::Aborted,
            }),
        }
    }
}

#[async_trait::async_trait]
impl AllocationApplier for GrpcAllocationApplier {
    async fn apply(
        &self,
        experiment_id: Uuid,
        allocation: &RolloutDistribution,
    ) -> Result<ApplyResult, anyhow::Error> {
        self.dispatch(experiment_id, allocation, None).await
    }

    async fn apply_realtime(
        &self,
        experiment_id: Uuid,
        allocation: &RolloutDistribution,
        model: stitchd_proto::flags::v1::RealtimeBanditModel,
    ) -> Result<ApplyResult, anyhow::Error> {
        self.dispatch(experiment_id, allocation, Some(model)).await
    }
}

/// Production [`RunRecorder`] inserting into the `bandit_allocation_runs` PG
/// table via a runtime `sqlx::query` (no compile-time macro — mirrors the
/// stats-service's other runtime queries).
pub struct PgRunRecorder {
    pool: sqlx::PgPool,
}

impl PgRunRecorder {
    /// Wrap a PG pool.
    #[must_use]
    pub fn new(pool: sqlx::PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait::async_trait]
impl RunRecorder for PgRunRecorder {
    #[allow(clippy::too_many_arguments)]
    async fn record(
        &self,
        experiment_id: Uuid,
        iteration_id: Uuid,
        action: &str,
        old_allocation: Option<Value>,
        new_allocation: Option<Value>,
        outcome: &str,
        detail: Option<String>,
    ) -> Result<(), anyhow::Error> {
        sqlx::query(
            "INSERT INTO bandit_allocation_runs \
             (experiment_id, iteration_id, fired_at, action, old_allocation, new_allocation, outcome, detail) \
             VALUES ($1, $2, now(), $3, $4, $5, $6, $7)",
        )
        .bind(experiment_id)
        .bind(iteration_id)
        .bind(action)
        .bind(old_allocation)
        .bind(new_allocation)
        .bind(outcome)
        .bind(detail)
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}

/// Production [`crate::grpc::service::ExperimentRecomputer`] driving the
/// per-experiment bandit reallocation pass on a `TriggerRecompute` RPC.
///
/// Fetches the live running experiments, finds the targeted one, resolves its
/// metric definitions, and runs [`run_bandit_reallocation`] against the live
/// ClickHouse sufficient stats — applying fresh weights immediately rather than
/// waiting for the next 60-min tick. A non-bandit (or not-currently-running)
/// experiment is a no-op.
pub struct PerExperimentRecomputer {
    exp_client: std::sync::Arc<
        tokio::sync::Mutex<
            stitchd_proto::experiments::v1::experimentation_service_client::ExperimentationServiceClient<
                tonic::transport::Channel,
            >,
        >,
    >,
    ch: std::sync::Arc<clickhouse::Client>,
    metric_repo: std::sync::Arc<dyn stitchd_db::MetricRepository>,
    pool: sqlx::PgPool,
}

impl PerExperimentRecomputer {
    /// Assemble the recomputer from the shared service dependencies.
    #[must_use]
    pub fn new(
        exp_client: std::sync::Arc<
            tokio::sync::Mutex<
                stitchd_proto::experiments::v1::experimentation_service_client::ExperimentationServiceClient<
                    tonic::transport::Channel,
                >,
            >,
        >,
        ch: std::sync::Arc<clickhouse::Client>,
        metric_repo: std::sync::Arc<dyn stitchd_db::MetricRepository>,
        pool: sqlx::PgPool,
    ) -> Self {
        Self {
            exp_client,
            ch,
            metric_repo,
            pool,
        }
    }
}

#[async_trait::async_trait]
impl crate::grpc::service::ExperimentRecomputer for PerExperimentRecomputer {
    async fn recompute(&self, experiment_id: Uuid) -> Result<(), anyhow::Error> {
        // Fetch the live running experiments and find the target.
        let experiments = {
            let mut client = self.exp_client.lock().await;
            crate::scheduler::fetch_running_experiments(&mut client).await?
        };
        let Some(exp) = experiments
            .into_iter()
            .find(|e| e.experiment_id == experiment_id)
        else {
            // Not currently running → nothing to reallocate (no-op).
            return Ok(());
        };

        // Resolve the experiment's metric definitions.
        let metric_ids: Vec<stitchd_core::id::MetricId> = exp
            .metric_ids
            .iter()
            .copied()
            .map(stitchd_core::id::MetricId::from_uuid)
            .collect();
        let metrics: HashMap<Uuid, MetricDefinition> = self
            .metric_repo
            .find_batch_by_ids(&metric_ids)
            .await
            .map(|defs| defs.into_iter().map(|m| (m.id.as_uuid(), m)).collect())
            .unwrap_or_default();

        let now = Utc::now();
        let reader = crate::compute::ClickHouseCellReader::new(self.ch.clone());
        let applier = GrpcAllocationApplier::new(self.exp_client.clone());
        let recorder = PgRunRecorder::new(self.pool.clone());

        run_bandit_reallocation(&reader, &applier, &recorder, &exp, &metrics, now, now)
            .await
            .map(|_| ())
    }
}

// ── Orchestration ────────────────────────────────────────────────────────────

/// Serialise a [`RolloutDistribution`] into the `new_allocation` JSON shape:
/// `{"variant_key": weight_bp, ...}`.
#[must_use]
fn distribution_to_json(dist: &RolloutDistribution) -> Value {
    let map: serde_json::Map<String, Value> = dist
        .allocations
        .iter()
        .map(|a| (a.variant_key.clone(), Value::from(a.percentage_bp)))
        .collect();
    Value::Object(map)
}

/// Crate-internal re-export of [`build_metric_rewards`] for the lifecycle module
/// (which builds the same objective posteriors for convergence detection).
pub(crate) async fn build_metric_rewards_pub(
    reader: &dyn CellReader,
    exp: &RunningExperiment,
    metric: &MetricDefinition,
    metrics: &HashMap<Uuid, MetricDefinition>,
    iteration_end: DateTime<Utc>,
) -> Result<Option<MetricRewards>, anyhow::Error> {
    build_metric_rewards(reader, exp, metric, metrics, iteration_end).await
}

/// Read summed per-variant sufficient stats for one objective metric across all
/// of the experiment's context types, and build a [`MetricRewards`] for it.
///
/// Returns `Ok(None)` when the metric has no usable family (e.g. a ratio whose
/// legs are unresolvable, or a neutral goal direction) — the caller skips that
/// metric. Propagates only real ClickHouse read errors.
async fn build_metric_rewards(
    reader: &dyn CellReader,
    exp: &RunningExperiment,
    metric: &MetricDefinition,
    metrics: &HashMap<Uuid, MetricDefinition>,
    iteration_end: DateTime<Utc>,
) -> Result<Option<MetricRewards>, anyhow::Error> {
    let Some(goal) = map_goal(metric.goal_direction) else {
        return Ok(None);
    };
    let mt = metric_type_for(metric);
    let context_types: Vec<String> = if exp.unit_context_types.is_empty() {
        vec!["user".to_string()]
    } else {
        exp.unit_context_types.clone()
    };

    // Accumulate per-variant sufficient stats summed across context types.
    let mut sample: HashMap<String, u64> = HashMap::new();
    let mut agg: HashMap<String, AggCell> = HashMap::new();
    let mut ratio: HashMap<String, RatioGroupStats> = HashMap::new();

    for ctx in &context_types {
        let counts = reader
            .assignment_counts(
                exp.experiment_id,
                exp.iteration_id,
                exp.env_id,
                ctx,
                iteration_end,
            )
            .await?;
        for (vk, n) in counts {
            *sample.entry(vk).or_insert(0) += n;
        }

        match &metric.kind {
            MetricKind::Aggregation(cfg) => {
                let cells = reader
                    .aggregation_cells(
                        cfg,
                        exp.experiment_id,
                        exp.iteration_id,
                        exp.env_id,
                        ctx,
                        iteration_end,
                    )
                    .await?;
                for (vk, c) in cells {
                    let e = agg.entry(vk).or_insert(AggCell {
                        event_n: 0,
                        successes: 0,
                        value_sum: 0.0,
                        value_sq_sum: 0.0,
                    });
                    e.event_n += c.event_n;
                    e.successes += c.successes;
                    e.value_sum += c.value_sum;
                    e.value_sq_sum += c.value_sq_sum;
                }
            }
            MetricKind::Funnel(cfg) => {
                let cells = reader
                    .funnel_cells(
                        cfg,
                        exp.experiment_id,
                        exp.iteration_id,
                        exp.env_id,
                        ctx,
                        iteration_end,
                    )
                    .await?;
                for (vk, (_n, successes)) in cells {
                    let e = agg.entry(vk).or_insert(AggCell {
                        event_n: 0,
                        successes: 0,
                        value_sum: 0.0,
                        value_sq_sum: 0.0,
                    });
                    e.successes += successes;
                }
            }
            MetricKind::Ratio(ratio_cfg) => {
                let (Some(num), Some(den)) = (
                    metrics.get(&ratio_cfg.numerator_metric_id.as_uuid()),
                    metrics.get(&ratio_cfg.denominator_metric_id.as_uuid()),
                ) else {
                    return Ok(None);
                };
                let (MetricKind::Aggregation(num_cfg), MetricKind::Aggregation(den_cfg)) =
                    (&num.kind, &den.kind)
                else {
                    return Ok(None);
                };
                let cells = reader
                    .ratio_cells(
                        num_cfg,
                        den_cfg,
                        exp.experiment_id,
                        exp.iteration_id,
                        exp.env_id,
                        ctx,
                        iteration_end,
                    )
                    .await?;
                for (vk, g) in cells {
                    let e = ratio.entry(vk).or_insert(RatioGroupStats {
                        n: 0,
                        num_sum: 0.0,
                        den_sum: 0.0,
                        num_sq_sum: 0.0,
                        den_sq_sum: 0.0,
                        num_den_sum: 0.0,
                    });
                    e.n += g.n;
                    e.num_sum += g.num_sum;
                    e.den_sum += g.den_sum;
                    e.num_sq_sum += g.num_sq_sum;
                    e.den_sq_sum += g.den_sq_sum;
                    e.num_den_sum += g.num_den_sum;
                }
            }
        }
    }

    // Build one arm per configured variant key, in the experiment's variant
    // order, zero-filling missing variants so every arm is represented.
    let is_ratio = matches!(mt, MetricType::Numeric) && matches!(metric.kind, MetricKind::Ratio(_));
    let arms: Vec<(String, RewardPosterior)> = exp
        .variant_keys
        .iter()
        .map(|vk| {
            let n = sample.get(vk).copied().unwrap_or(0);
            let post = if is_ratio {
                let g = ratio.get(vk).copied().unwrap_or(RatioGroupStats {
                    n: n as i64,
                    num_sum: 0.0,
                    den_sum: 0.0,
                    num_sq_sum: 0.0,
                    den_sq_sum: 0.0,
                    num_den_sum: 0.0,
                });
                RewardPosterior::Ratio(g)
            } else {
                posterior_for(mt, n, agg.get(vk).copied(), None)
            };
            (vk.clone(), post)
        })
        .collect();

    Ok(Some(MetricRewards {
        metric_id: metric.id.as_uuid(),
        goal,
        arms,
    }))
}

/// An even basis-point split across `variant_keys`, used as the static fallback
/// distribution that rides alongside a refreshed realtime model (for contexts
/// without the diversion unit). Largest-remainder so the sum is exactly 10000.
#[must_use]
fn even_split(variant_keys: &[String]) -> RolloutDistribution {
    let n = variant_keys.len().max(1) as u32;
    let base = 10_000 / n;
    let mut rem = 10_000 - base * n;
    let allocations = variant_keys
        .iter()
        .map(|vk| {
            let extra = if rem > 0 {
                rem -= 1;
                1
            } else {
                0
            };
            RolloutAllocation {
                variant_key: vk.clone(),
                percentage_bp: base + extra,
            }
        })
        .collect();
    RolloutDistribution { allocations }
}

/// Read per-unit contextual reward rows for the single scalar objective metric
/// across the experiment's context types and decide the contextual model.
///
/// Skips (recoverable) when the objective is not a single `Aggregation`-backed
/// scalar metric (the only family the per-unit reward query supports today).
/// Propagates only a real ClickHouse read error.
#[allow(clippy::too_many_arguments)]
async fn decide_contextual_refresh(
    reader: &dyn CellReader,
    exp: &RunningExperiment,
    config: &BanditConfig,
    ctx_cfg: &stitchd_core::experimentation::bandit::ContextualConfig,
    metrics: &HashMap<Uuid, MetricDefinition>,
    iteration_end: DateTime<Utc>,
    salt: &str,
    unit_context_type: &str,
) -> Result<RealtimeModelDecision, anyhow::Error> {
    // Single scalar objective only; the per-unit reward query reads one metric.
    let RewardObjective::Scalar { metric_id } = &config.objective else {
        return Ok(RealtimeModelDecision::Skip(
            "contextual realtime model currently supports a single scalar objective".to_string(),
        ));
    };
    let Some(def) = metrics.get(metric_id) else {
        return Ok(RealtimeModelDecision::Skip(
            "objective metric definition not found".to_string(),
        ));
    };
    let MetricKind::Aggregation(agg_cfg) = &def.kind else {
        return Ok(RealtimeModelDecision::Skip(
            "contextual realtime model currently supports an aggregation reward metric".to_string(),
        ));
    };
    let Some(goal) = map_goal(def.goal_direction) else {
        return Ok(RealtimeModelDecision::Skip(
            "neutral objective metric has no optimisation direction".to_string(),
        ));
    };

    // The first declared feature drives the per-unit reward query (the query
    // returns one feature column); additional features ride the model spec but
    // are encoded from the same single column today.
    let Some(feature_param) = ctx_cfg.features.first() else {
        return Ok(RealtimeModelDecision::Skip(
            "contextual config declares no features".to_string(),
        ));
    };

    let context_types: Vec<String> = if exp.unit_context_types.is_empty() {
        vec![unit_context_type.to_string()]
    } else {
        exp.unit_context_types.clone()
    };

    let mut rows: Vec<ContextualRow> = Vec::new();
    for ct in &context_types {
        let part = reader
            .contextual_reward_rows(
                agg_cfg,
                feature_param,
                exp.experiment_id,
                exp.iteration_id,
                exp.env_id,
                ct,
                iteration_end,
            )
            .await?;
        rows.extend(part);
    }

    Ok(decide_contextual_model(
        &ctx_cfg.features,
        &rows,
        goal,
        salt,
        unit_context_type,
    ))
}

/// Run the **realtime** snapshot-resident refresh for one bandit experiment.
///
/// Reads the objective metric posteriors, builds the [`RealtimeBanditModel`] wire
/// params, and dispatches them to the bound rule via `apply_realtime` so the next
/// snapshot build + SDK poll delivers the new posteriors. Records exactly one
/// `bandit_allocation_runs` row (action `reallocate`, or `skip` on a recoverable
/// condition). Only a real ClickHouse / PG error propagates.
async fn run_realtime_refresh(
    reader: &dyn CellReader,
    applier: &dyn AllocationApplier,
    recorder: &dyn RunRecorder,
    exp: &RunningExperiment,
    metrics: &HashMap<Uuid, MetricDefinition>,
    iteration_end: DateTime<Utc>,
) -> Result<BanditRunOutcome, anyhow::Error> {
    let config = exp
        .bandit_config
        .as_ref()
        .expect("eligible realtime bandit has config");

    // The diversion unit + salt: the first configured unit context type, and the
    // experiment id as a stable per-experiment salt.
    let unit_context_type = exp
        .unit_context_types
        .first()
        .cloned()
        .unwrap_or_else(|| "user".to_string());
    let salt = exp.experiment_id.to_string();

    // CONTEXTUAL algorithm → fit a per-context linear model from per-unit reward
    // rows; otherwise refresh the non-contextual posteriors. Both produce a
    // RealtimeModelDecision that the shared apply/record path below handles.
    let decision = if let BanditAlgorithm::Contextual(ctx_cfg) = &config.algorithm {
        decide_contextual_refresh(
            reader,
            exp,
            config,
            ctx_cfg,
            metrics,
            iteration_end,
            &salt,
            &unit_context_type,
        )
        .await?
    } else {
        // Resolve + read the objective metric posteriors (single scalar objective).
        let mut metric_rewards: Vec<MetricRewards> = Vec::new();
        for mid in objective_metric_ids(&config.objective) {
            let Some(def) = metrics.get(&mid) else {
                continue;
            };
            if let Some(mr) = build_metric_rewards(reader, exp, def, metrics, iteration_end).await?
            {
                metric_rewards.push(mr);
            }
        }
        decide_realtime_model(config, &metric_rewards, &salt, &unit_context_type)
    };

    match decision {
        RealtimeModelDecision::Skip(reason) => {
            recorder
                .record(
                    exp.experiment_id,
                    exp.iteration_id,
                    "skip",
                    None,
                    None,
                    "skipped",
                    Some(reason.clone()),
                )
                .await?;
            Ok(BanditRunOutcome::Skipped { reason })
        }
        RealtimeModelDecision::Refresh(model) => {
            // Static fallback distribution rides along for contexts missing the
            // diversion unit (an even split keeps every arm represented).
            let dist = even_split(&exp.variant_keys);
            let detail_json = serde_json::json!({
                "realtime_model": {
                    "family": model.family,
                    "goal": model.goal,
                    "unit_context_type": model.unit_context_type,
                    "variants": model.variants.iter().map(|v| v.variant_key.clone()).collect::<Vec<_>>(),
                },
            });
            match applier
                .apply_realtime(exp.experiment_id, &dist, model)
                .await
            {
                Ok(ApplyResult::Applied {
                    resolved_target,
                    new_version,
                }) => {
                    recorder
                        .record(
                            exp.experiment_id,
                            exp.iteration_id,
                            "reallocate",
                            None,
                            Some(detail_json),
                            "applied",
                            Some(format!(
                                "realtime model refreshed on {resolved_target} (version {new_version})"
                            )),
                        )
                        .await?;
                    Ok(BanditRunOutcome::Applied {
                        allocation: dist,
                        resolved_target,
                        new_version,
                    })
                }
                Ok(ApplyResult::Skipped { detail }) => {
                    recorder
                        .record(
                            exp.experiment_id,
                            exp.iteration_id,
                            "skip",
                            None,
                            Some(detail_json),
                            "skipped",
                            Some(detail.clone()),
                        )
                        .await?;
                    Ok(BanditRunOutcome::Skipped { reason: detail })
                }
                Ok(ApplyResult::Failed {
                    detail,
                    version_conflict,
                }) => {
                    recorder
                        .record(
                            exp.experiment_id,
                            exp.iteration_id,
                            "reallocate",
                            None,
                            Some(detail_json),
                            "failed",
                            Some(detail.clone()),
                        )
                        .await?;
                    Ok(BanditRunOutcome::Failed {
                        allocation: dist,
                        detail,
                        version_conflict,
                    })
                }
                Err(e) => {
                    let detail = format!("apply_realtime RPC error: {e}");
                    recorder
                        .record(
                            exp.experiment_id,
                            exp.iteration_id,
                            "reallocate",
                            None,
                            Some(detail_json),
                            "failed",
                            Some(detail.clone()),
                        )
                        .await?;
                    Ok(BanditRunOutcome::Failed {
                        allocation: dist,
                        detail,
                        version_conflict: false,
                    })
                }
            }
        }
    }
}

/// Run the bandit static-reallocation pass for one experiment.
///
/// No-ops (returns [`BanditRunOutcome::NotApplicable`], no row written) for any
/// experiment that is not an eligible bandit. A static-propagation bandit reads
/// sufficient stats, computes + writes weights; a realtime-propagation bandit
/// refreshes the snapshot-resident model params instead. Records exactly one
/// history row. Skips (records a `skip` row) on any recoverable condition; only
/// a real ClickHouse read error or PG insert error propagates as `Err`.
#[allow(clippy::too_many_arguments)]
pub async fn run_bandit_reallocation(
    reader: &dyn CellReader,
    applier: &dyn AllocationApplier,
    recorder: &dyn RunRecorder,
    exp: &RunningExperiment,
    metrics: &HashMap<Uuid, MetricDefinition>,
    iteration_end: DateTime<Utc>,
    tick: DateTime<Utc>,
) -> Result<BanditRunOutcome, anyhow::Error> {
    // Realtime propagation path: refresh the snapshot-resident model params
    // rather than rewriting a static %. Mutually exclusive with the static path.
    if is_eligible_realtime(exp) {
        return run_realtime_refresh(reader, applier, recorder, exp, metrics, iteration_end).await;
    }

    if !is_eligible(exp) {
        return Ok(BanditRunOutcome::NotApplicable);
    }
    // Safe: is_eligible guarantees Some(config).
    let config = exp
        .bandit_config
        .as_ref()
        .expect("eligible bandit has config");

    // Resolve + read the objective metric posteriors.
    let mut metric_rewards: Vec<MetricRewards> = Vec::new();
    for mid in objective_metric_ids(&config.objective) {
        let Some(def) = metrics.get(&mid) else {
            continue;
        };
        if let Some(mr) = build_metric_rewards(reader, exp, def, metrics, iteration_end).await? {
            metric_rewards.push(mr);
        }
    }

    let seed = derive_seed(
        exp.experiment_id,
        exp.iteration_id,
        exp.variant_keys.len(),
        tick,
    );

    match decide_allocation(config, &metric_rewards, seed) {
        AllocationDecision::Skip(reason) => {
            recorder
                .record(
                    exp.experiment_id,
                    exp.iteration_id,
                    "skip",
                    None,
                    None,
                    "skipped",
                    Some(reason.clone()),
                )
                .await?;
            Ok(BanditRunOutcome::Skipped { reason })
        }
        AllocationDecision::Reallocate(dist) => {
            let new_json = distribution_to_json(&dist);
            match applier.apply(exp.experiment_id, &dist).await {
                Ok(ApplyResult::Applied {
                    resolved_target,
                    new_version,
                }) => {
                    recorder
                        .record(
                            exp.experiment_id,
                            exp.iteration_id,
                            "reallocate",
                            None,
                            Some(new_json),
                            "applied",
                            Some(format!(
                                "applied to {resolved_target} (version {new_version})"
                            )),
                        )
                        .await?;
                    Ok(BanditRunOutcome::Applied {
                        allocation: dist,
                        resolved_target,
                        new_version,
                    })
                }
                Ok(ApplyResult::Skipped { detail }) => {
                    recorder
                        .record(
                            exp.experiment_id,
                            exp.iteration_id,
                            "skip",
                            None,
                            Some(new_json),
                            "skipped",
                            Some(detail.clone()),
                        )
                        .await?;
                    Ok(BanditRunOutcome::Skipped { reason: detail })
                }
                Ok(ApplyResult::Failed {
                    detail,
                    version_conflict,
                }) => {
                    recorder
                        .record(
                            exp.experiment_id,
                            exp.iteration_id,
                            "reallocate",
                            None,
                            Some(new_json),
                            "failed",
                            Some(detail.clone()),
                        )
                        .await?;
                    Ok(BanditRunOutcome::Failed {
                        allocation: dist,
                        detail,
                        version_conflict,
                    })
                }
                // A transport-level RPC error is recoverable (next tick retries):
                // record a failed row and keep advancing.
                Err(e) => {
                    let detail = format!("ApplyBanditAllocation RPC error: {e}");
                    recorder
                        .record(
                            exp.experiment_id,
                            exp.iteration_id,
                            "reallocate",
                            None,
                            Some(new_json),
                            "failed",
                            Some(detail.clone()),
                        )
                        .await?;
                    Ok(BanditRunOutcome::Failed {
                        allocation: dist,
                        detail,
                        version_conflict: false,
                    })
                }
            }
        }
    }
}

/// Shared test fakes + builders reused by both the [`tests`] module here and the
/// [`crate::lifecycle`] tests. Richer API (call counts, captured allocations,
/// recorded-row inspection) than the inline fakes in [`tests`].
#[cfg(test)]
pub(crate) mod tests_support {
    use super::*;
    use std::sync::Mutex;

    use stitchd_core::experimentation::bandit::BanditConfig;
    use stitchd_core::id::{EnvironmentId, MetricId};
    use stitchd_core::metric::{
        AggregationConfig, AggregationOperator, MetricDefinition, MetricKind,
    };

    use crate::scheduler::SequentialSettings;

    /// A [`RunRecorder`] that captures every recorded row for inspection.
    #[derive(Default)]
    pub(crate) struct FakeRecorder {
        actions: Mutex<Vec<String>>,
    }

    impl FakeRecorder {
        /// Number of rows recorded.
        pub(crate) fn count(&self) -> usize {
            self.actions.lock().unwrap().len()
        }
        /// The most recently recorded action, if any.
        pub(crate) fn last_action(&self) -> Option<String> {
            self.actions.lock().unwrap().last().cloned()
        }
    }

    #[async_trait::async_trait]
    impl RunRecorder for FakeRecorder {
        #[allow(clippy::too_many_arguments)]
        async fn record(
            &self,
            _experiment_id: Uuid,
            _iteration_id: Uuid,
            action: &str,
            _old: Option<Value>,
            _new: Option<Value>,
            _outcome: &str,
            _detail: Option<String>,
        ) -> Result<(), anyhow::Error> {
            self.actions.lock().unwrap().push(action.to_string());
            Ok(())
        }
    }

    /// An [`AllocationApplier`] that counts calls and captures the last applied
    /// allocation. Always returns APPLIED.
    #[derive(Default)]
    pub(crate) struct FakeApplier {
        apply_calls: Mutex<u32>,
        last_allocation: Mutex<Option<RolloutDistribution>>,
    }

    impl FakeApplier {
        /// Number of static `apply` calls.
        pub(crate) fn apply_calls(&self) -> u32 {
            *self.apply_calls.lock().unwrap()
        }
        /// The last applied allocation, if any.
        pub(crate) fn last_allocation(&self) -> Option<RolloutDistribution> {
            self.last_allocation.lock().unwrap().clone()
        }
    }

    #[async_trait::async_trait]
    impl AllocationApplier for FakeApplier {
        async fn apply(
            &self,
            _experiment_id: Uuid,
            allocation: &RolloutDistribution,
        ) -> Result<ApplyResult, anyhow::Error> {
            *self.apply_calls.lock().unwrap() += 1;
            *self.last_allocation.lock().unwrap() = Some(allocation.clone());
            Ok(ApplyResult::Applied {
                resolved_target: "default_rule".into(),
                new_version: 1,
            })
        }

        async fn apply_realtime(
            &self,
            _experiment_id: Uuid,
            _allocation: &RolloutDistribution,
            _model: stitchd_proto::flags::v1::RealtimeBanditModel,
        ) -> Result<ApplyResult, anyhow::Error> {
            Ok(ApplyResult::Applied {
                resolved_target: "rule".into(),
                new_version: 1,
            })
        }
    }

    /// A [`CellReader`] returning fixed assignment counts + conversion successes
    /// for one metric (count family). All other families return empty.
    pub(crate) struct FakeReader {
        counts: Vec<(String, u64)>,
        successes: HashMap<String, u64>,
    }

    impl FakeReader {
        /// Build from `(variant, n, conversions)` tuples for a count metric.
        pub(crate) fn with_conversions(_metric_id: Uuid, arms: Vec<(&str, u64, u64)>) -> Self {
            Self {
                counts: arms.iter().map(|(k, n, _)| (k.to_string(), *n)).collect(),
                successes: arms.iter().map(|(k, _, s)| (k.to_string(), *s)).collect(),
            }
        }
    }

    #[async_trait::async_trait]
    impl CellReader for FakeReader {
        async fn assignment_counts(
            &self,
            _e: Uuid,
            _i: Uuid,
            _env: Uuid,
            _ctx: &str,
            _end: DateTime<Utc>,
        ) -> Result<Vec<(String, u64)>, anyhow::Error> {
            Ok(self.counts.clone())
        }
        async fn aggregation_cells(
            &self,
            _cfg: &AggregationConfig,
            _e: Uuid,
            _i: Uuid,
            _env: Uuid,
            _ctx: &str,
            _end: DateTime<Utc>,
        ) -> Result<HashMap<String, AggCell>, anyhow::Error> {
            Ok(self
                .successes
                .iter()
                .map(|(k, s)| {
                    (
                        k.clone(),
                        AggCell {
                            event_n: 0,
                            successes: *s,
                            value_sum: 0.0,
                            value_sq_sum: 0.0,
                        },
                    )
                })
                .collect())
        }
        async fn ratio_cells(
            &self,
            _n: &AggregationConfig,
            _d: &AggregationConfig,
            _e: Uuid,
            _i: Uuid,
            _env: Uuid,
            _ctx: &str,
            _end: DateTime<Utc>,
        ) -> Result<HashMap<String, RatioGroupStats>, anyhow::Error> {
            Ok(HashMap::new())
        }
        async fn funnel_cells(
            &self,
            _cfg: &stitchd_core::metric::FunnelConfig,
            _e: Uuid,
            _i: Uuid,
            _env: Uuid,
            _ctx: &str,
            _end: DateTime<Utc>,
        ) -> Result<HashMap<String, (u64, u64)>, anyhow::Error> {
            Ok(HashMap::new())
        }
        async fn percentile_samples(
            &self,
            _cfg: &AggregationConfig,
            _e: Uuid,
            _i: Uuid,
            _env: Uuid,
            _ctx: &str,
            _end: DateTime<Utc>,
        ) -> Result<HashMap<String, Vec<f64>>, anyhow::Error> {
            Ok(HashMap::new())
        }
        #[allow(clippy::too_many_arguments)]
        async fn unit_values_with_pre(
            &self,
            _cfg: &AggregationConfig,
            _e: Uuid,
            _i: Uuid,
            _env: Uuid,
            _ctx: &str,
            _end: DateTime<Utc>,
            _ps: DateTime<Utc>,
            _pe: DateTime<Utc>,
        ) -> Result<Vec<(String, String, f64, f64)>, anyhow::Error> {
            Ok(Vec::new())
        }
        #[allow(clippy::too_many_arguments)]
        async fn contextual_reward_rows(
            &self,
            _cfg: &AggregationConfig,
            _feature_param: &str,
            _e: Uuid,
            _i: Uuid,
            _env: Uuid,
            _ctx: &str,
            _end: DateTime<Utc>,
        ) -> Result<Vec<(String, String, f64)>, anyhow::Error> {
            Ok(Vec::new())
        }
    }

    /// A count-family [`MetricDefinition`] for tests.
    pub(crate) fn count_metric(id: Uuid, env: Uuid) -> MetricDefinition {
        let now = Utc::now();
        MetricDefinition {
            id: MetricId::from_uuid(id),
            environment_id: EnvironmentId::from_uuid(env),
            key: "conv".into(),
            name: "conv".into(),
            description: None,
            kind: MetricKind::Aggregation(AggregationConfig {
                event_key: "conv".into(),
                aggregator: AggregationOperator::Count,
                on_field: None,
                where_clause: None,
            }),
            goal_direction: MetricGoal::Increase,
            version: 1,
            created_at: now,
            updated_at: now,
            deleted_at: None,
        }
    }

    /// A running experiment with explicit env / variant_keys / metric_ids.
    pub(crate) fn running_bandit_with(
        config: Option<BanditConfig>,
        mode: ExperimentMode,
        env: Uuid,
        variant_keys: Vec<String>,
        metric_ids: Vec<Uuid>,
    ) -> RunningExperiment {
        RunningExperiment {
            experiment_id: Uuid::new_v4(),
            env_id: env,
            iteration_id: Uuid::new_v4(),
            metric_ids,
            variant_keys,
            started_at: Utc::now(),
            unit_context_types: vec!["user".into()],
            pre_period_days: 0,
            sequential: SequentialSettings::default(),
            variant_expected_bp: HashMap::new(),
            experiment_mode: mode,
            bandit_config: config,
            bandit_campaign_id: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use stitchd_core::experimentation::bandit::{
        BanditConfig, ContextualConfig, ExperimentMode, LifecyclePolicy, PropagationMode,
        RewardObjective,
    };
    use stitchd_core::id::{EnvironmentId, MetricId};
    use stitchd_core::metric::{
        AggregationConfig, AggregationOperator, MetricDefinition, MetricKind,
    };

    use crate::scheduler::SequentialSettings;

    fn conv_posterior(n: u64, succ: u64) -> RewardPosterior {
        RewardPosterior::Conversion(count_variant_stats(n, succ))
    }

    fn metric_rewards(goal: BanditGoal, arms: Vec<(&str, u64, u64)>) -> MetricRewards {
        MetricRewards {
            metric_id: Uuid::new_v4(),
            goal,
            arms: arms
                .into_iter()
                .map(|(k, n, s)| (k.to_string(), conv_posterior(n, s)))
                .collect(),
        }
    }

    fn thompson_config(min_floor: u32) -> BanditConfig {
        BanditConfig {
            algorithm: BanditAlgorithm::ThompsonSampling,
            propagation_mode: PropagationMode::Static,
            min_exploration_bp: min_floor,
            objective: RewardObjective::Scalar {
                metric_id: Uuid::new_v4(),
            },
            lifecycle_policy: LifecyclePolicy::Advisory,
            convergence_prob_threshold: 0.95,
        }
    }

    fn running_bandit(config: Option<BanditConfig>, mode: ExperimentMode) -> RunningExperiment {
        RunningExperiment {
            experiment_id: Uuid::new_v4(),
            env_id: Uuid::new_v4(),
            iteration_id: Uuid::new_v4(),
            metric_ids: vec![],
            variant_keys: vec!["control".into(), "treatment".into()],
            started_at: Utc::now(),
            unit_context_types: vec!["user".into()],
            pre_period_days: 0,
            sequential: SequentialSettings::default(),
            variant_expected_bp: HashMap::new(),
            experiment_mode: mode,
            bandit_config: config,
            bandit_campaign_id: None,
        }
    }

    // ── Eligibility guard ────────────────────────────────────────────────────

    #[test]
    fn eligible_only_for_static_bandit() {
        let static_cfg = thompson_config(500);
        assert!(is_eligible(&running_bandit(
            Some(static_cfg.clone()),
            ExperimentMode::Bandit
        )));

        // Fixed mode → ineligible even with a config.
        assert!(!is_eligible(&running_bandit(
            Some(static_cfg.clone()),
            ExperimentMode::Fixed
        )));

        // Bandit mode but no config → ineligible.
        assert!(!is_eligible(&running_bandit(None, ExperimentMode::Bandit)));

        // Realtime propagation → not the static path.
        let mut realtime = static_cfg;
        realtime.propagation_mode = PropagationMode::Realtime;
        assert!(!is_eligible(&running_bandit(
            Some(realtime),
            ExperimentMode::Bandit
        )));
    }

    // ── Seed determinism ─────────────────────────────────────────────────────

    #[test]
    fn seed_is_deterministic_and_varies_with_inputs() {
        let exp = Uuid::new_v4();
        let iter = Uuid::new_v4();
        let t = Utc::now();
        let s1 = derive_seed(exp, iter, 3, t);
        let s2 = derive_seed(exp, iter, 3, t);
        assert_eq!(s1, s2, "same inputs → same seed");

        // Different tick (1s later) → different seed.
        assert_ne!(
            s1,
            derive_seed(exp, iter, 3, t + chrono::Duration::seconds(1))
        );
        // Different arm count → different seed.
        assert_ne!(s1, derive_seed(exp, iter, 4, t));
        // Different experiment → different seed.
        assert_ne!(s1, derive_seed(Uuid::new_v4(), iter, 3, t));
    }

    #[test]
    fn decide_is_reproducible_under_same_seed() {
        let cfg = thompson_config(500);
        let mr = vec![metric_rewards(
            BanditGoal::Increase,
            vec![("control", 1000, 100), ("treatment", 1000, 250)],
        )];
        let d1 = decide_allocation(&cfg, &mr, 42);
        let d2 = decide_allocation(&cfg, &mr, 42);
        assert_eq!(d1, d2, "Thompson decision must be reproducible for a seed");
    }

    // ── Skip on insufficient data ────────────────────────────────────────────

    #[test]
    fn skips_when_any_arm_below_min_sample() {
        let cfg = thompson_config(500);
        let mr = vec![metric_rewards(
            BanditGoal::Increase,
            vec![("control", 1000, 100), ("treatment", 5, 1)], // treatment n=5 < 30
        )];
        match decide_allocation(&cfg, &mr, 1) {
            AllocationDecision::Skip(r) => assert!(r.contains("insufficient data"), "{r}"),
            other => panic!("expected skip, got {other:?}"),
        }
    }

    #[test]
    fn skips_when_no_metric_rewards() {
        let cfg = thompson_config(500);
        match decide_allocation(&cfg, &[], 1) {
            AllocationDecision::Skip(r) => assert!(r.contains("no objective metric")),
            other => panic!("expected skip, got {other:?}"),
        }
    }

    #[test]
    fn skips_contextual_algorithm() {
        let mut cfg = thompson_config(500);
        cfg.algorithm = BanditAlgorithm::Contextual(ContextualConfig::default());
        let mr = vec![metric_rewards(
            BanditGoal::Increase,
            vec![("control", 1000, 100), ("treatment", 1000, 250)],
        )];
        match decide_allocation(&cfg, &mr, 1) {
            AllocationDecision::Skip(r) => assert!(r.contains("contextual"), "{r}"),
            other => panic!("expected skip, got {other:?}"),
        }
    }

    // ── Contextual model fit (pure) ──────────────────────────────────────────

    #[test]
    fn decide_contextual_model_skips_without_features() {
        let rows: Vec<ContextualRow> = vec![("a".into(), "1".into(), 1.0)];
        match decide_contextual_model(&[], &rows, BanditGoal::Increase, "s", "user") {
            RealtimeModelDecision::Skip(r) => assert!(r.contains("no features"), "{r}"),
            other => panic!("expected skip, got {other:?}"),
        }
    }

    #[test]
    fn decide_contextual_model_skips_on_insufficient_rows() {
        // 2 rows per arm, below MIN_ARM_SAMPLE (30).
        let mut rows: Vec<ContextualRow> = Vec::new();
        for _ in 0..2 {
            rows.push(("a".into(), "1".into(), 1.0));
            rows.push(("b".into(), "1".into(), 1.0));
        }
        match decide_contextual_model(&["score".into()], &rows, BanditGoal::Increase, "s", "user") {
            RealtimeModelDecision::Skip(r) => assert!(r.contains("insufficient"), "{r}"),
            other => panic!("expected skip, got {other:?}"),
        }
    }

    #[test]
    fn decide_contextual_model_fits_better_variant_per_feature() {
        use stitchd_core::experimentation::bandit::contextual::sample_contextual_variant;
        use stitchd_core::experimentation::bandit::{
            ContextualModel, FeatureEncoding, FeatureSpec,
        };

        // Reward depends on the feature: variant "a" rewards HIGH score
        // (reward = score), variant "b" rewards LOW score (reward = 10 - score).
        // 60 rows per arm over scores 0..10 so the fit clears MIN_ARM_SAMPLE.
        let mut rows: Vec<ContextualRow> = Vec::new();
        for i in 0..60u32 {
            let score = (i % 11) as f64; // 0..10
            rows.push(("a".into(), score.to_string(), score));
            rows.push(("b".into(), score.to_string(), 10.0 - score));
        }

        let decision = decide_contextual_model(
            &["score".into()],
            &rows,
            BanditGoal::Increase,
            "salt",
            "user",
        );
        let RealtimeModelDecision::Refresh(model) = decision else {
            panic!("expected a refreshed contextual model");
        };
        let proto_ctx = model.contextual.expect("contextual model on the wire");
        // Map the proto contextual model back to the domain to sample from it.
        let domain = ContextualModel {
            features: proto_ctx
                .features
                .iter()
                .map(|f| FeatureSpec {
                    context_type: f.context_type.clone(),
                    parameter: f.parameter.clone(),
                    encoding: FeatureEncoding::Numeric,
                })
                .collect(),
            variants: proto_ctx
                .variants
                .iter()
                .map(|v| {
                    stitchd_core::experimentation::bandit::VariantCoefficients {
                        variant_key: v.variant_key.clone(),
                        coeffs: v.coeffs.clone(),
                        a_inv: None, // exploit the fitted means for the assertion
                    }
                })
                .collect(),
        };

        // For HIGH score (10), "a" should win the majority; for LOW score (0),
        // "b" should win the majority.
        let mut a_high = 0;
        let mut b_low = 0;
        let n = 500u64;
        for seed in 0..n {
            if sample_contextual_variant(
                &domain,
                &[1.0, 10.0],
                stitchd_core::rule_engine::types::BanditGoal::Increase,
                seed,
            )
            .as_deref()
                == Some("a")
            {
                a_high += 1;
            }
            if sample_contextual_variant(
                &domain,
                &[1.0, 0.0],
                stitchd_core::rule_engine::types::BanditGoal::Increase,
                seed,
            )
            .as_deref()
                == Some("b")
            {
                b_low += 1;
            }
        }
        assert!(
            a_high as f64 / n as f64 > 0.9,
            "a wins high score: {a_high}/{n}"
        );
        assert!(
            b_low as f64 / n as f64 > 0.9,
            "b wins low score: {b_low}/{n}"
        );
    }

    #[test]
    fn skips_when_floor_too_high() {
        // 2 arms × 6000 bp floor = 12000 > 10000 → normalize rejects.
        let cfg = thompson_config(6000);
        let mr = vec![metric_rewards(
            BanditGoal::Increase,
            vec![("control", 1000, 100), ("treatment", 1000, 250)],
        )];
        match decide_allocation(&cfg, &mr, 1) {
            AllocationDecision::Skip(r) => assert!(r.contains("normalize"), "{r}"),
            other => panic!("expected skip, got {other:?}"),
        }
    }

    // ── Algorithm dispatch produces valid, better-arm-favouring weights ───────

    fn assert_valid_distribution(dist: &RolloutDistribution, floor: u32) {
        let sum: u32 = dist.allocations.iter().map(|a| a.percentage_bp).sum();
        assert_eq!(sum, 10_000, "must sum to 10000");
        for a in &dist.allocations {
            assert!(
                a.percentage_bp >= floor,
                "{} = {} below floor {floor}",
                a.variant_key,
                a.percentage_bp
            );
        }
    }

    fn weight_of(dist: &RolloutDistribution, key: &str) -> u32 {
        dist.allocations
            .iter()
            .find(|a| a.variant_key == key)
            .map(|a| a.percentage_bp)
            .unwrap_or(0)
    }

    #[test]
    fn thompson_favours_better_arm() {
        let cfg = thompson_config(500);
        // treatment clearly better (25% vs 10%).
        let mr = vec![metric_rewards(
            BanditGoal::Increase,
            vec![("control", 1000, 100), ("treatment", 1000, 250)],
        )];
        let AllocationDecision::Reallocate(dist) = decide_allocation(&cfg, &mr, 7) else {
            panic!("expected reallocate");
        };
        assert_valid_distribution(&dist, 500);
        assert!(
            weight_of(&dist, "treatment") > weight_of(&dist, "control"),
            "treatment should get more traffic: {dist:?}"
        );
    }

    #[test]
    fn epsilon_greedy_dispatch_favours_best_arm() {
        let mut cfg = thompson_config(0);
        cfg.algorithm = BanditAlgorithm::EpsilonGreedy { epsilon: 0.1 };
        let mr = vec![metric_rewards(
            BanditGoal::Increase,
            vec![("control", 1000, 100), ("treatment", 1000, 300)],
        )];
        let AllocationDecision::Reallocate(dist) = decide_allocation(&cfg, &mr, 7) else {
            panic!("expected reallocate");
        };
        assert_valid_distribution(&dist, 0);
        assert!(
            weight_of(&dist, "treatment") > weight_of(&dist, "control"),
            "epsilon-greedy should put most traffic on best arm: {dist:?}"
        );
    }

    #[test]
    fn ucb_dispatch_favours_best_arm() {
        let mut cfg = thompson_config(500);
        cfg.algorithm = BanditAlgorithm::Ucb { c: 1.0 };
        let mr = vec![metric_rewards(
            BanditGoal::Increase,
            vec![("control", 1000, 100), ("treatment", 1000, 400)],
        )];
        let AllocationDecision::Reallocate(dist) = decide_allocation(&cfg, &mr, 7) else {
            panic!("expected reallocate");
        };
        assert_valid_distribution(&dist, 500);
        assert!(
            weight_of(&dist, "treatment") >= weight_of(&dist, "control"),
            "ucb should favour the higher-mean arm: {dist:?}"
        );
    }

    #[test]
    fn decrease_goal_favours_lower_arm() {
        let cfg = thompson_config(500);
        // For a Decrease goal, the LOWER rate is better. control 10%, treatment 30%.
        let mr = vec![metric_rewards(
            BanditGoal::Decrease,
            vec![("control", 1000, 100), ("treatment", 1000, 300)],
        )];
        let AllocationDecision::Reallocate(dist) = decide_allocation(&cfg, &mr, 7) else {
            panic!("expected reallocate");
        };
        assert!(
            weight_of(&dist, "control") > weight_of(&dist, "treatment"),
            "lower-rate arm should win for Decrease: {dist:?}"
        );
    }

    // ── Goal mapping ─────────────────────────────────────────────────────────

    #[test]
    fn neutral_goal_maps_to_none() {
        assert_eq!(map_goal(MetricGoal::Neutral), None);
        assert_eq!(map_goal(MetricGoal::Increase), Some(BanditGoal::Increase));
        assert_eq!(map_goal(MetricGoal::Decrease), Some(BanditGoal::Decrease));
    }

    // ── Objective metric id extraction ───────────────────────────────────────

    #[test]
    fn objective_ids_cover_all_objective_shapes() {
        let m = Uuid::new_v4();
        assert_eq!(
            objective_metric_ids(&RewardObjective::Scalar { metric_id: m }),
            vec![m]
        );
        let g = Uuid::new_v4();
        let ids = objective_metric_ids(&RewardObjective::Constrained {
            primary_metric_id: m,
            constraints: vec![stitchd_core::experimentation::bandit::GuardrailConstraint {
                metric_id: g,
                bound: 1.0,
                direction: stitchd_core::experimentation::bandit::ConstraintDirection::AtLeast,
            }],
        });
        assert_eq!(ids, vec![m, g]);
    }

    // ── distribution_to_json shape ───────────────────────────────────────────

    #[test]
    fn distribution_json_is_keyed_by_variant() {
        let dist = RolloutDistribution {
            allocations: vec![
                stitchd_core::rollout::RolloutAllocation {
                    variant_key: "control".into(),
                    percentage_bp: 3000,
                },
                stitchd_core::rollout::RolloutAllocation {
                    variant_key: "treatment".into(),
                    percentage_bp: 7000,
                },
            ],
        };
        let json = distribution_to_json(&dist);
        assert_eq!(json["control"], serde_json::json!(3000));
        assert_eq!(json["treatment"], serde_json::json!(7000));
    }

    // ── posterior_for numeric path ───────────────────────────────────────────

    #[test]
    fn posterior_for_numeric_builds_numeric_stats() {
        let cell = AggCell {
            event_n: 100,
            successes: 0,
            value_sum: 500.0,
            value_sq_sum: 3000.0,
        };
        let p = posterior_for(MetricType::Numeric, 100, Some(cell), None);
        match p {
            RewardPosterior::Numeric(vs) => {
                assert_eq!(vs.sample_size, 100);
                assert!((vs.mean.unwrap() - 5.0).abs() < 1e-9);
            }
            other => panic!("expected Numeric, got {other:?}"),
        }
    }

    // ── min_arm_n ────────────────────────────────────────────────────────────

    #[test]
    fn min_arm_n_takes_smallest_across_metrics() {
        let m1 = metric_rewards(BanditGoal::Increase, vec![("a", 100, 10), ("b", 50, 5)]);
        let m2 = metric_rewards(BanditGoal::Increase, vec![("a", 100, 10), ("b", 20, 2)]);
        assert_eq!(min_arm_n(&[m1, m2]), 20);
    }

    // ── Orchestration with fakes ─────────────────────────────────────────────

    use std::sync::Mutex;

    #[derive(Default)]
    struct RecordedRow {
        action: String,
        outcome: String,
        new_allocation: Option<Value>,
        detail: Option<String>,
    }

    #[derive(Default)]
    struct FakeRecorder {
        rows: Mutex<Vec<RecordedRow>>,
    }

    #[async_trait::async_trait]
    impl RunRecorder for FakeRecorder {
        #[allow(clippy::too_many_arguments)]
        async fn record(
            &self,
            _experiment_id: Uuid,
            _iteration_id: Uuid,
            action: &str,
            _old: Option<Value>,
            new_allocation: Option<Value>,
            outcome: &str,
            detail: Option<String>,
        ) -> Result<(), anyhow::Error> {
            self.rows.lock().unwrap().push(RecordedRow {
                action: action.to_string(),
                outcome: outcome.to_string(),
                new_allocation,
                detail,
            });
            Ok(())
        }
    }

    struct FakeApplier {
        result: ApplyResult,
        calls: Mutex<u32>,
        realtime_calls: Mutex<u32>,
        last_model: Mutex<Option<stitchd_proto::flags::v1::RealtimeBanditModel>>,
    }

    impl FakeApplier {
        fn new(result: ApplyResult) -> Self {
            Self {
                result,
                calls: Mutex::new(0),
                realtime_calls: Mutex::new(0),
                last_model: Mutex::new(None),
            }
        }
    }

    #[async_trait::async_trait]
    impl AllocationApplier for FakeApplier {
        async fn apply(
            &self,
            _experiment_id: Uuid,
            _allocation: &RolloutDistribution,
        ) -> Result<ApplyResult, anyhow::Error> {
            *self.calls.lock().unwrap() += 1;
            Ok(self.result.clone())
        }

        async fn apply_realtime(
            &self,
            _experiment_id: Uuid,
            _allocation: &RolloutDistribution,
            model: stitchd_proto::flags::v1::RealtimeBanditModel,
        ) -> Result<ApplyResult, anyhow::Error> {
            *self.realtime_calls.lock().unwrap() += 1;
            *self.last_model.lock().unwrap() = Some(model);
            Ok(self.result.clone())
        }
    }

    // A CellReader that returns fixed assignment counts + conversion successes.
    struct FakeReader {
        counts: Vec<(String, u64)>,
        successes: HashMap<String, u64>,
        // Per-unit contextual reward rows the contextual fit reads.
        contextual_rows: Vec<ContextualRow>,
    }

    #[async_trait::async_trait]
    impl CellReader for FakeReader {
        async fn assignment_counts(
            &self,
            _e: Uuid,
            _i: Uuid,
            _env: Uuid,
            _ctx: &str,
            _end: DateTime<Utc>,
        ) -> Result<Vec<(String, u64)>, anyhow::Error> {
            Ok(self.counts.clone())
        }
        async fn aggregation_cells(
            &self,
            _cfg: &AggregationConfig,
            _e: Uuid,
            _i: Uuid,
            _env: Uuid,
            _ctx: &str,
            _end: DateTime<Utc>,
        ) -> Result<HashMap<String, AggCell>, anyhow::Error> {
            Ok(self
                .successes
                .iter()
                .map(|(k, s)| {
                    (
                        k.clone(),
                        AggCell {
                            event_n: 0,
                            successes: *s,
                            value_sum: 0.0,
                            value_sq_sum: 0.0,
                        },
                    )
                })
                .collect())
        }
        async fn ratio_cells(
            &self,
            _n: &AggregationConfig,
            _d: &AggregationConfig,
            _e: Uuid,
            _i: Uuid,
            _env: Uuid,
            _ctx: &str,
            _end: DateTime<Utc>,
        ) -> Result<HashMap<String, RatioGroupStats>, anyhow::Error> {
            Ok(HashMap::new())
        }
        async fn funnel_cells(
            &self,
            _cfg: &stitchd_core::metric::FunnelConfig,
            _e: Uuid,
            _i: Uuid,
            _env: Uuid,
            _ctx: &str,
            _end: DateTime<Utc>,
        ) -> Result<HashMap<String, (u64, u64)>, anyhow::Error> {
            Ok(HashMap::new())
        }
        async fn percentile_samples(
            &self,
            _cfg: &AggregationConfig,
            _e: Uuid,
            _i: Uuid,
            _env: Uuid,
            _ctx: &str,
            _end: DateTime<Utc>,
        ) -> Result<HashMap<String, Vec<f64>>, anyhow::Error> {
            Ok(HashMap::new())
        }
        #[allow(clippy::too_many_arguments)]
        async fn unit_values_with_pre(
            &self,
            _cfg: &AggregationConfig,
            _e: Uuid,
            _i: Uuid,
            _env: Uuid,
            _ctx: &str,
            _end: DateTime<Utc>,
            _ps: DateTime<Utc>,
            _pe: DateTime<Utc>,
        ) -> Result<Vec<(String, String, f64, f64)>, anyhow::Error> {
            Ok(Vec::new())
        }
        #[allow(clippy::too_many_arguments)]
        async fn contextual_reward_rows(
            &self,
            _cfg: &AggregationConfig,
            _feature_param: &str,
            _e: Uuid,
            _i: Uuid,
            _env: Uuid,
            _ctx: &str,
            _end: DateTime<Utc>,
        ) -> Result<Vec<(String, String, f64)>, anyhow::Error> {
            Ok(self.contextual_rows.clone())
        }
    }

    fn count_metric(id: Uuid, env: Uuid) -> MetricDefinition {
        let now = Utc::now();
        MetricDefinition {
            id: MetricId::from_uuid(id),
            environment_id: EnvironmentId::from_uuid(env),
            key: "conv".into(),
            name: "conv".into(),
            description: None,
            kind: MetricKind::Aggregation(AggregationConfig {
                event_key: "conv".into(),
                aggregator: AggregationOperator::Count,
                on_field: None,
                where_clause: None,
            }),
            goal_direction: MetricGoal::Increase,
            version: 1,
            created_at: now,
            updated_at: now,
            deleted_at: None,
        }
    }

    #[tokio::test]
    async fn non_bandit_is_no_op_no_row() {
        let reader = FakeReader {
            counts: vec![],
            successes: HashMap::new(),
            contextual_rows: vec![],
        };
        let applier = FakeApplier::new(ApplyResult::Applied {
            resolved_target: "x".into(),
            new_version: 1,
        });
        let recorder = FakeRecorder::default();
        let exp = running_bandit(None, ExperimentMode::Fixed);
        let out = run_bandit_reallocation(
            &reader,
            &applier,
            &recorder,
            &exp,
            &HashMap::new(),
            Utc::now(),
            Utc::now(),
        )
        .await
        .unwrap();
        assert_eq!(out, BanditRunOutcome::NotApplicable);
        assert_eq!(*applier.calls.lock().unwrap(), 0, "no apply for non-bandit");
        assert!(
            recorder.rows.lock().unwrap().is_empty(),
            "no row for non-bandit"
        );
    }

    #[tokio::test]
    async fn applied_writes_one_reallocate_row() {
        let metric_id = Uuid::new_v4();
        let env = Uuid::new_v4();
        let mut cfg = thompson_config(500);
        cfg.objective = RewardObjective::Scalar { metric_id };

        let mut exp = running_bandit(Some(cfg), ExperimentMode::Bandit);
        exp.env_id = env;
        exp.metric_ids = vec![metric_id];

        // treatment clearly better.
        let reader = FakeReader {
            counts: vec![("control".into(), 1000), ("treatment".into(), 1000)],
            successes: HashMap::from([("control".into(), 100), ("treatment".into(), 300)]),
            contextual_rows: vec![],
        };
        let applier = FakeApplier::new(ApplyResult::Applied {
            resolved_target: "default_rule".into(),
            new_version: 9,
        });
        let recorder = FakeRecorder::default();
        let metrics = HashMap::from([(metric_id, count_metric(metric_id, env))]);

        let out = run_bandit_reallocation(
            &reader,
            &applier,
            &recorder,
            &exp,
            &metrics,
            Utc::now(),
            Utc::now(),
        )
        .await
        .unwrap();

        match out {
            BanditRunOutcome::Applied {
                allocation,
                resolved_target,
                new_version,
            } => {
                assert_eq!(resolved_target, "default_rule");
                assert_eq!(new_version, 9);
                assert!(weight_of(&allocation, "treatment") > weight_of(&allocation, "control"));
            }
            other => panic!("expected Applied, got {other:?}"),
        }
        assert_eq!(*applier.calls.lock().unwrap(), 1);
        let rows = recorder.rows.lock().unwrap();
        assert_eq!(rows.len(), 1, "exactly one row per tick");
        assert_eq!(rows[0].action, "reallocate");
        assert_eq!(rows[0].outcome, "applied");
        assert!(rows[0].new_allocation.is_some());
    }

    #[tokio::test]
    async fn insufficient_data_records_skip_no_apply() {
        let metric_id = Uuid::new_v4();
        let env = Uuid::new_v4();
        let mut cfg = thompson_config(500);
        cfg.objective = RewardObjective::Scalar { metric_id };
        let mut exp = running_bandit(Some(cfg), ExperimentMode::Bandit);
        exp.env_id = env;
        exp.metric_ids = vec![metric_id];

        // treatment n=5 < MIN_ARM_SAMPLE.
        let reader = FakeReader {
            counts: vec![("control".into(), 1000), ("treatment".into(), 5)],
            successes: HashMap::from([("control".into(), 100), ("treatment".into(), 1)]),
            contextual_rows: vec![],
        };
        let applier = FakeApplier::new(ApplyResult::Applied {
            resolved_target: "x".into(),
            new_version: 1,
        });
        let recorder = FakeRecorder::default();
        let metrics = HashMap::from([(metric_id, count_metric(metric_id, env))]);

        let out = run_bandit_reallocation(
            &reader,
            &applier,
            &recorder,
            &exp,
            &metrics,
            Utc::now(),
            Utc::now(),
        )
        .await
        .unwrap();
        assert!(matches!(out, BanditRunOutcome::Skipped { .. }));
        assert_eq!(*applier.calls.lock().unwrap(), 0, "no apply when skipping");
        let rows = recorder.rows.lock().unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].action, "skip");
        assert_eq!(rows[0].outcome, "skipped");
    }

    #[tokio::test]
    async fn rpc_failed_records_failed_row() {
        let metric_id = Uuid::new_v4();
        let env = Uuid::new_v4();
        let mut cfg = thompson_config(500);
        cfg.objective = RewardObjective::Scalar { metric_id };
        let mut exp = running_bandit(Some(cfg), ExperimentMode::Bandit);
        exp.env_id = env;
        exp.metric_ids = vec![metric_id];

        let reader = FakeReader {
            counts: vec![("control".into(), 1000), ("treatment".into(), 1000)],
            successes: HashMap::from([("control".into(), 100), ("treatment".into(), 300)]),
            contextual_rows: vec![],
        };
        let applier = FakeApplier::new(ApplyResult::Failed {
            detail: "version conflict".into(),
            version_conflict: true,
        });
        let recorder = FakeRecorder::default();
        let metrics = HashMap::from([(metric_id, count_metric(metric_id, env))]);

        let out = run_bandit_reallocation(
            &reader,
            &applier,
            &recorder,
            &exp,
            &metrics,
            Utc::now(),
            Utc::now(),
        )
        .await
        .unwrap();
        match out {
            BanditRunOutcome::Failed {
                version_conflict, ..
            } => assert!(version_conflict),
            other => panic!("expected Failed, got {other:?}"),
        }
        let rows = recorder.rows.lock().unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].action, "reallocate");
        assert_eq!(rows[0].outcome, "failed");
        assert_eq!(rows[0].detail.as_deref(), Some("version conflict"));
    }

    // ── bandit_20260608 Phase 5: realtime propagation path ──────────────────

    fn realtime_config(metric_id: Uuid) -> BanditConfig {
        let mut cfg = thompson_config(500);
        cfg.propagation_mode = PropagationMode::Realtime;
        cfg.objective = RewardObjective::Scalar { metric_id };
        cfg
    }

    #[test]
    fn eligible_realtime_only_for_realtime_bandit() {
        let rt = realtime_config(Uuid::new_v4());
        assert!(is_eligible_realtime(&running_bandit(
            Some(rt.clone()),
            ExperimentMode::Bandit
        )));
        // Static propagation → not the realtime path.
        let mut static_cfg = rt.clone();
        static_cfg.propagation_mode = PropagationMode::Static;
        assert!(!is_eligible_realtime(&running_bandit(
            Some(static_cfg),
            ExperimentMode::Bandit
        )));
        // Fixed mode → ineligible.
        assert!(!is_eligible_realtime(&running_bandit(
            Some(rt),
            ExperimentMode::Fixed
        )));
        // No config → ineligible.
        assert!(!is_eligible_realtime(&running_bandit(
            None,
            ExperimentMode::Bandit
        )));
    }

    #[test]
    fn decide_realtime_model_builds_beta_posteriors() {
        let cfg = realtime_config(Uuid::new_v4());
        let mr = metric_rewards(
            BanditGoal::Increase,
            vec![("control", 1000, 100), ("treatment", 1000, 300)],
        );
        match decide_realtime_model(&cfg, &[mr], "salt", "user") {
            RealtimeModelDecision::Refresh(model) => {
                assert_eq!(model.salt, "salt");
                assert_eq!(model.unit_context_type, "user");
                assert_eq!(
                    model.family,
                    stitchd_proto::flags::v1::RewardFamily::Beta as i32
                );
                assert_eq!(model.variants.len(), 2);
                // treatment has the higher Beta mean → larger alpha/(alpha+beta).
                let by = |k: &str| {
                    model
                        .variants
                        .iter()
                        .find(|v| v.variant_key == k)
                        .unwrap()
                        .clone()
                };
                let c = by("control");
                let t = by("treatment");
                assert!(
                    t.alpha / (t.alpha + t.beta) > c.alpha / (c.alpha + c.beta),
                    "treatment posterior mean should exceed control"
                );
            }
            other => panic!("expected Refresh, got {other:?}"),
        }
    }

    #[test]
    fn decide_realtime_model_skips_on_insufficient_data() {
        let cfg = realtime_config(Uuid::new_v4());
        let mr = metric_rewards(
            BanditGoal::Increase,
            vec![("control", 1000, 100), ("treatment", 5, 1)],
        );
        assert!(matches!(
            decide_realtime_model(&cfg, &[mr], "s", "user"),
            RealtimeModelDecision::Skip(_)
        ));
    }

    #[test]
    fn decide_realtime_model_skips_multi_objective() {
        let mut cfg = realtime_config(Uuid::new_v4());
        cfg.objective = RewardObjective::Scalarized { weights: vec![] };
        let mr = metric_rewards(
            BanditGoal::Increase,
            vec![("control", 1000, 100), ("treatment", 1000, 300)],
        );
        assert!(matches!(
            decide_realtime_model(&cfg, &[mr], "s", "user"),
            RealtimeModelDecision::Skip(_)
        ));
    }

    /// The realtime path dispatches a MODEL (not a static %), and records a
    /// `reallocate` row.
    #[tokio::test]
    async fn realtime_mode_refreshes_model_not_static_dist() {
        let metric_id = Uuid::new_v4();
        let env = Uuid::new_v4();
        let cfg = realtime_config(metric_id);
        let mut exp = running_bandit(Some(cfg), ExperimentMode::Bandit);
        exp.env_id = env;
        exp.metric_ids = vec![metric_id];

        let reader = FakeReader {
            counts: vec![("control".into(), 1000), ("treatment".into(), 1000)],
            successes: HashMap::from([("control".into(), 100), ("treatment".into(), 300)]),
            contextual_rows: vec![],
        };
        let applier = FakeApplier::new(ApplyResult::Applied {
            resolved_target: "rule-1".into(),
            new_version: 4,
        });
        let recorder = FakeRecorder::default();
        let metrics = HashMap::from([(metric_id, count_metric(metric_id, env))]);

        let out = run_bandit_reallocation(
            &reader,
            &applier,
            &recorder,
            &exp,
            &metrics,
            Utc::now(),
            Utc::now(),
        )
        .await
        .unwrap();

        assert!(matches!(out, BanditRunOutcome::Applied { .. }));
        // The realtime path was used, NOT the static apply.
        assert_eq!(*applier.realtime_calls.lock().unwrap(), 1);
        assert_eq!(
            *applier.calls.lock().unwrap(),
            0,
            "static apply must not run"
        );
        let model = applier
            .last_model
            .lock()
            .unwrap()
            .clone()
            .expect("model dispatched");
        assert_eq!(model.variants.len(), 2);
        assert_eq!(model.unit_context_type, "user");
        let rows = recorder.rows.lock().unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].action, "reallocate");
        assert_eq!(rows[0].outcome, "applied");
    }

    /// REGRESSION: a static-mode bandit still uses the static apply path (NOT
    /// the realtime model dispatch), unchanged from Phase 4.
    #[tokio::test]
    async fn static_mode_still_uses_static_apply() {
        let metric_id = Uuid::new_v4();
        let env = Uuid::new_v4();
        let mut cfg = thompson_config(500); // Static propagation
        cfg.objective = RewardObjective::Scalar { metric_id };
        let mut exp = running_bandit(Some(cfg), ExperimentMode::Bandit);
        exp.env_id = env;
        exp.metric_ids = vec![metric_id];

        let reader = FakeReader {
            counts: vec![("control".into(), 1000), ("treatment".into(), 1000)],
            successes: HashMap::from([("control".into(), 100), ("treatment".into(), 300)]),
            contextual_rows: vec![],
        };
        let applier = FakeApplier::new(ApplyResult::Applied {
            resolved_target: "default_rule".into(),
            new_version: 7,
        });
        let recorder = FakeRecorder::default();
        let metrics = HashMap::from([(metric_id, count_metric(metric_id, env))]);

        let out = run_bandit_reallocation(
            &reader,
            &applier,
            &recorder,
            &exp,
            &metrics,
            Utc::now(),
            Utc::now(),
        )
        .await
        .unwrap();

        assert!(matches!(out, BanditRunOutcome::Applied { .. }));
        assert_eq!(*applier.calls.lock().unwrap(), 1, "static apply path used");
        assert_eq!(
            *applier.realtime_calls.lock().unwrap(),
            0,
            "realtime path must not run for a static bandit"
        );
    }

    #[test]
    fn even_split_sums_to_10000() {
        for n in 1..=7usize {
            let keys: Vec<String> = (0..n).map(|i| format!("v{i}")).collect();
            let dist = even_split(&keys);
            let sum: u32 = dist.allocations.iter().map(|a| a.percentage_bp).sum();
            assert_eq!(sum, 10_000, "n={n}");
            assert_eq!(dist.allocations.len(), n);
        }
    }
}
