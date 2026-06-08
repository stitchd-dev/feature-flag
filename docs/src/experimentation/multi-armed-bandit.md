# Multi-Armed Bandit

Bandit mode (post-`bandit_20260608`) turns an experiment from a **fixed-split A/B test** into an
adaptive allocator that shifts traffic toward better-performing arms while it runs. It is an
opt-in **experiment mode** — `experiment_mode = 'ab_test' | 'bandit'` — layered on top of the
existing first-exposure attribution, whole-flag lock, and per-context-type stats pipeline.

## Algorithms

Selected per experiment via `bandit_config` (in `stitchd-core::experimentation::bandit`):

- **Thompson sampling** — posterior sampling (Beta for conversion/count, Normal for continuous).
- **ε-greedy** — exploit the current best arm with probability `1-ε`, explore uniformly otherwise.
- **UCB1** — upper-confidence-bound optimism.
- **Contextual** — per-context-feature linear / Thompson models, so allocation can vary by context.

A `min_exploration_bp` floor guarantees every arm keeps a minimum share so no arm is starved
during exploration.

## Propagation Modes

**Static** — the scheduled `stitchd-stats-service` tick reads live rewards from ClickHouse,
recomputes arm weights, and writes the new distribution onto the bound rule via a **privileged
lock-bypass** flag-service write. (The whole-flag lock blocks *human* edits while an experiment
runs; it does not block the autonomous reallocation.)

**Realtime** — a compact per-arm posterior **snapshot model** is published onto the rule and
evaluated **DB-free, in-memory** inside `evaluate_flag`. Non-bandit evaluation is **byte-for-byte
identical** to the static path — guaranteed by a golden-bits test and enforced structurally by
`stitchd-core::evaluation::purity`, which keeps `evaluate_flag` free of any I/O.

## Autonomous Lifecycle

Opt-in per experiment (`stitchd-stats-service::lifecycle`):

- **Advisory** — record the converged winner (`bandit_converged_variant` / `bandit_converged_prob`)
  for a "ready to commit" badge. No traffic change.
- **AutoCommit** — additionally commit 100% to the winner. The flag stays running.
- **AutoRollout** — commit, then **stop** the experiment, which releases the whole-flag lock.

Convergence is detected over the objective posteriors at a configurable probability threshold. The
commit → stop sequence is idempotent and restart-safe. Every autonomous action records a
`bandit_allocation_runs` history row (`reallocate` / `commit` / `rollout`).

## Campaigns & Multi-Objective Rewards

- **Campaigns** (`stitchd-stats-service::campaign`) chain successive bandit iterations
  autonomously, spawning the next iteration when the current one converges.
- **Reward objectives** (`RewardObjective`): **scalar** (single metric), **scalarized** (weighted
  blend of metrics), or **constrained** (optimize one metric subject to guardrail constraints).

## Surfacing

- REST: `GET …/experiments/{id}/bandit` (state — current allocation, posteriors, convergence,
  committed flag), `…/bandit/history` (allocation timeline), `…/bandit-campaigns`.
- Admin UI: bandit config in the experiment create/edit form, and a **Bandit** Results view
  (allocation-over-time chart, posteriors, convergence badge).

## Configuration

- `STITCHD_STATS_MAX_INTERACTION_ORDER` (default `3`) caps cross-experiment interaction order; the
  interaction analysis is generalized to **order 4+** alongside bandit mode (see the env-vars
  reference).

## Implementation Status

| Component                                                          | Status      |
|-------------------------------------------------------------------|-------------|
| Bandit algorithms (Thompson / ε-greedy / UCB / contextual)        | ✅ Complete |
| Static reallocation (privileged lock-bypass write)                | ✅ Complete |
| Realtime snapshot model (DB-free in-memory eval)                   | ✅ Complete |
| Autonomous lifecycle (Advisory / AutoCommit / AutoRollout)        | ✅ Complete |
| Campaigns (chained iterations)                                    | ✅ Complete |
| Multi-objective rewards (scalar / scalarized / constrained)        | ✅ Complete |
| Cross-experiment interaction order 4+                             | ✅ Complete |
| REST bandit state / history / campaigns                          | ✅ Complete |
| Admin UI bandit config + Results view                            | ✅ Complete |
