# Spec: Multi-Armed Bandit (Adaptive & Autonomous Experiment Allocation)

## Overview

Add a **bandit (adaptive allocation) mode** to the existing experiment entity. Instead of
a fixed split held constant for the experiment's life, a bandit experiment continuously
shifts traffic toward better-performing variants based on observed reward, balancing
exploration and exploitation — and can run its own lifecycle (commit/stop/roll out)
autonomously when the operator opts in.

The bandit reuses the existing experiment substrate — flag binding (custom rule OR
default-rule), `metric_ids`, `unit_context_types`, exclusion groups, whole-flag lock,
first-exposure ITT attribution, and the Bayesian stats core (Beta-Binomial / Normal-Normal
posteriors). It supports two propagation paths (see FR4): a **static-rewrite** path that
preserves the existing eval invariant, and a **real-time snapshot-resident** path required
for contextual bandits.

This is a large, multi-frontier track (accepted explicitly): four algorithms incl.
contextual, all four reward metric families, scalar + multi-objective reward, static +
real-time eval paths, autonomous lifecycle + campaigns, and 4+ way bandit-aware interaction
analysis.

## Functional Requirements

### FR1 — Bandit as an experiment mode
- Add `experiment_mode` (`fixed` | `bandit`; default `fixed`) to the `Experiment` /
  `ExperimentIteration` domain types, PG tables, proto, gateway REST bodies, Admin UI forms.
- Nullable `bandit_config` JSONB on both tables: `algorithm`, algorithm params,
  `min_exploration_bp`, `objective_metric_id`, `propagation_mode` (static | realtime),
  `lifecycle_policy` (advisory | auto_commit | auto_rollout), convergence thresholds.
  Snapshotted onto the iteration at start (mirrors sequential-testing settings).
- Mode + config immutable while running/paused (lock-consistent with existing rules).

### FR2 — Algorithms (all in scope)
- **Thompson Sampling** (default) — Monte-Carlo over existing reward posteriors
  (`bayesian::analyze_count` Beta-Binomial, `analyze_numeric` Normal-Normal, etc.);
  weight ∝ probability-best.
- **Epsilon-greedy** — ε spread across the floor, remainder to current best arm.
- **UCB** — optimism-under-uncertainty index from the same sufficient stats.
- **Contextual bandit** — reward conditioned on context features; a per-context linear /
  logistic reward model (e.g. LinUCB / Thompson-on-linear-model) whose parameters ride on
  the flag snapshot and are sampled per-context at eval time (FR4 real-time path).

### FR3 — Reward metric support (all four families)
- Single configured objective metric drives reward, honoring goal direction.
- Conversion (Beta-Binomial) + continuous (Normal-Normal) land first; ratio (delta-method)
  + funnel (final-step rate) folded in once the reward abstraction is proven. All reuse the
  per-family `analyze_*` functions and `variant_stats.rs` sufficient-stat queries.

### FR4 — Reallocation & propagation (two paths)
- **Static-rewrite path (non-contextual algorithms, default):** the new
  `stitchd-stats-service` bandit module computes weights each recompute tick + on
  `TriggerRecompute`, normalizes to basis points (sum 10,000; each arm ≥
  `min_exploration_bp`), and writes them to the bound rule's allocation via a **privileged
  system-actor allocation-update path** that bypasses the whole-flag human lock
  (version-bumped, audit-logged as the bandit/system actor — mirrors the scheduler).
  `evaluate_flag` stays experiment-unaware / static-snapshot / zero-DB; SDKs converge on
  next poll/stream. **Eval invariant fully preserved on this path.**
- **Real-time snapshot-resident path (contextual; optional for non-contextual):** reward
  model parameters (posteriors / linear-model coefficients) ride on the flag definition
  snapshot. `evaluate_flag` gains a bandit-aware sampling step that, for a real-time-mode
  rule, draws the variant per-context from the snapshot-resident model with **zero DB
  lookup** (consistent with the existing in-memory `ExclusionGate` pattern). This is an
  explicit, accepted departure from the "purely static / experiment-unaware" property:
  eval becomes bandit-*aware* but remains pure + zero-DB + deterministic under a
  context-seeded RNG. Resolves identically in preview and the Rust SDK (model rides the
  snapshot the SDK already fetches). The stats tick refreshes the model parameters in the
  snapshot rather than rewriting a static %.

### FR5 — Safety & autonomous lifecycle
- **Min-exploration floor:** each arm ≥ a configured % so no arm is starved.
- **Convergence detection:** posterior probability-to-be-best crossing a configured
  threshold.
- **Lifecycle policy (operator-selected per experiment):**
  - `advisory` (default) — raise a "ready to commit" badge, no automated action;
  - `auto_commit` — lock allocation to the winner (100% within the experiment, flag still
    held);
  - `auto_rollout` — commit to the winner **and** autonomously stop the experiment,
    promoting the winning variant into the flag's standing rule / default and releasing the
    flag lock — a fully autonomous stop-and-roll-out lifecycle.
  - Every autonomous action requires the operator's up-front `lifecycle_policy` opt-in; no
    action without it.
- **Allocation & lifecycle history / audit:** new `bandit_allocation_runs` table (mirrors
  `scheduled_change_runs`): `experiment_id`, `iteration_id`, `fired_at`, `old_allocation`,
  `new_allocation`, `action` (reallocate|commit|rollout|spawn_iteration|skip), `outcome`
  (applied|skipped|failed), `detail`. Every reallocation + autonomous action recorded.

### FR6 — Bandit-aware interaction analysis (incl. order 4+)
- Cross-experiment interaction analysis (currently 2-/3-way) must remain correct when a
  participating experiment has **time-varying** allocation: the self-join over
  `experiment_assignments` already keys on assignment time, but the interaction sweep's
  allocation/SRM assumptions are revisited so a shifting bandit split does not produce
  spurious interaction or SRM flags.
- **Generalize interaction order from 3 to 4+** (lifting the current order-3 cap) so
  overlapping bandit + fixed experiments can be analyzed at higher order, reusing the
  unified `experiment_ids Array(UUID)` + `interaction_order` + `term` schema and the
  hierarchical-decomposition + BH-FDR machinery. Order remains operator-bounded
  (configurable cap) to control combinatorial blow-up.

### FR7 — Surfacing (proto / REST / Admin UI)
- Per-variant current allocation + reward posterior (+ contextual model summary +
  per-objective posteriors + campaign status) surfaced via `WriteExperimentResultsRequest`
  → `experiment_results` → `VariantResult` → REST.
- Admin UI: bandit mode picker + algorithm/propagation/lifecycle/campaign/multi-objective
  config in create/edit; a **Bandit** Results view with live per-arm allocation-over-time
  chart (from `bandit_allocation_runs`), current weights, reward posteriors (per objective),
  convergence/commit badge, and a lifecycle-action timeline; interaction tab reflects 4+ way
  + bandit notes.

### FR8 — Autonomous optimization campaigns (auto-creation, opt-in)
- A **bandit campaign** is an operator-configured-once construct that auto-creates
  successive experiment iterations without per-experiment manual creation. Two triggers,
  both bounded by the one-time campaign opt-in (so "no operator trigger" means no *per-run*
  trigger — the campaign itself is the standing authorization):
  - **On convergence:** when an iteration commits/rolls out, the campaign opens a new
    iteration with the winner as the new control plus any newly-registered variants —
    perpetual optimization.
  - **On reward drift:** a configured drift detector (winner's posterior degrades past a
    threshold vs. a challenger) reopens exploration by spawning a fresh iteration.
- Campaign config (`max_iterations`, drift thresholds, variant-discovery policy, budget
  caps) persisted in `bandit_campaigns`; every auto-created iteration is audit-logged in
  `bandit_allocation_runs` with `action = spawn_iteration`. Hard `max_iterations` / budget
  ceiling so a campaign cannot run unbounded.

### FR9 — Multi-objective (vector-reward) bandits
- Reward may be a **vector across multiple objective metrics** rather than a single scalar.
  Two combination modes, operator-selected:
  - **Scalarization:** weighted sum of per-metric standardized rewards (each goal-direction
    normalized), weights in `bandit_config`. The bandit optimizes the scalarized reward via
    the same algorithms (Thompson/epsilon/UCB/contextual).
  - **Constrained:** optimize a primary objective metric subject to guardrail-metric
    constraints (an arm whose guardrail posterior violates its bound is down-weighted /
    excluded from exploitation, kept only at the exploration floor). Reuses the existing
    guardrail direction machinery.
- Per-objective posteriors all surface in the Bandit Results view so operators see the
  tradeoff, not just the combined score.

## Non-Functional Requirements
- **Eval paths:** static-rewrite path preserves the full existing invariant (static,
  experiment-unaware, zero-DB). Real-time path is bandit-*aware* but stays pure, zero-DB,
  snapshot-resident, and deterministic under a context-seeded RNG — an explicit accepted
  tradeoff, gated to real-time-mode rules only so `fixed` and static-bandit flags are
  unaffected.
- **Determinism / testability:** all allocation + sampling are pure functions of sufficient
  stats / model params + config + seeded RNG (reuse the LCG in `bayesian.rs`); Monte-Carlo +
  golden-vector tests validate weights, contextual sampling, and convergence/lifecycle
  transitions.
- **Privacy:** allocation traces, history, and contextual-model summaries name variant keys
  + feature names + weights only — never `privateParameters` values, in any layer.
- **Restart-safe & idempotent:** a tick crashing mid-write rolls back; next tick recomputes
  cleanly; no duplicate allocation/lifecycle rows per tick; autonomous stop/rollout and
  campaign-iteration spawn are idempotent.
- **Concurrency:** allocation/lifecycle writes use optimistic version bumps; concurrent
  human flag mutation still returns the lock 409.
- **Coverage:** ≥90% per crate; new `stitchd-stats-service` self-seeding live-ClickHouse
  integration tests added to the CI explicit `--test` list (workflow.md CI gotcha).

## Acceptance Criteria
1. Experiment creatable in `bandit` mode with algorithm, objective metric, floor,
   propagation mode, and lifecycle policy; config snapshotted at start; immutable while
   running/paused.
2. Static path: each tick rewrites the bound rule allocation toward higher reward, arms ≥
   floor, `bandit_allocation_runs` row recorded; succeeds despite the flag lock (system
   actor) while concurrent human mutation still gets 409.
3. `evaluate_flag` / preview / SDK assign per updated static allocation with NO change to
   the static eval path; SDK reflects new weights after a poll.
4. Real-time path: a contextual bandit assigns per-context from the snapshot-resident model
   with zero DB lookup, identical in preview + SDK, deterministic under seed; non-bandit and
   static-bandit flags' eval path is provably unchanged.
5. Thompson, epsilon-greedy, UCB, and contextual each produce valid normalized outputs
   validated by Monte-Carlo / golden-vector tests; conversion + continuous + ratio + funnel
   rewards all work.
6. Lifecycle: `advisory` badge fires on convergence; `auto_commit` locks to winner;
   `auto_rollout` commits + autonomously stops + promotes winner + releases lock — each only
   under its opt-in.
7. Campaigns auto-create successive iterations on convergence/drift, bounded by
   `max_iterations`/budget, idempotent, fully audited.
8. Multi-objective: scalarization + constrained guardrail modes both work; per-objective
   posteriors surfaced.
9. Interaction analysis stays correct under time-varying bandit allocation (no spurious
   SRM/interaction flags) and supports order 4+ behind an operator-bounded cap.
10. Admin UI: full bandit config + live allocation chart + posteriors + convergence/commit
    badge + lifecycle/campaign timeline + interaction surfacing.
11. CI green: workspace tests (incl. new bandit unit + live-ClickHouse integration), clippy
    `-Dwarnings`, fmt, sqlx-check, OpenAPI contract, docs idempotent, admin vitest.

## Out of Scope
- Streaming flag push (SDK still polls; tracked as its own track) — real-time-path weight
  changes still reach SDKs faster via the snapshot they already fetch.
- Multi-objective beyond scalarization + constrained (e.g. full Pareto-front exploration) —
  the two combination modes in FR9 are the scope; explicit Pareto-front bandits are a later
  track.
