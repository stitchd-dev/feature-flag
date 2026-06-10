# Spec: Experiment Lifecycle UI — real transitions + honest config

**Track ID:** `experiment_lifecycle_ui_20260610`
**Type:** Feature (UI wiring; backend ahead of UI)
**Beads:** `feature-flag-dx6` (wisp `feature-flag-wisp-99m`)

## Overview

The backend exposes experiment status transitions
(`POST /v1/environments/{env}/experiments/{id}/transitions`, body
`{ new_status, reason? }` → `TransitionExperiment` RPC; statuses
`draft`/`running`/`paused`/`concluded`), but the admin Experiment detail page
(`admin/src/pages/experiments/ExperimentDetail.tsx`) **never calls it**:

- The header shows a dead **"Stop"** button (no `onClick`) and a dead
  **"Ship winner"** button. There is no Start/Pause/Resume/Conclude control. The
  only way to change status today is the Schedule tab (a *scheduled* change).
- The **Configuration tab** is largely fabricated: hardcoded `Allocation 50/50`,
  `Targeting segment beta-customers AND country in [US, CA]`, `Min sample size
  380,000`, `MDE 2.0%`, `α/β 0.05/0.20`, `CUPED enabled · variance reduction
  18%`, `Duration 14 days`.
- The **Lifecycle card** is entirely mock (`Marco G.`, `Priya R.`, fixed dates).
- The **Events tab** is a placeholder empty-state ("Live tail of qualifying
  events…") with no backing API.

This track wires real status transitions and replaces fabricated config/lifecycle
content with real data (or honest removal). No backend change.

## Functional Requirements

### FR1 — Transition API client
Add `transitionExperiment(envId, experimentId, newStatus, reason?)` to
`admin/src/lib/api.ts`, POSTing `{ new_status, reason }` to the transitions
route and returning the updated `ExperimentSummary`. `newStatus` typed to
`'draft' | 'active' | 'paused' | 'concluded'`.

### FR2 — Real transition controls
Replace the dead header buttons with contextual, valid-only controls derived
from the real `status`:
- `draft` → **Start** (→ active)
- `running` → **Pause** (→ paused), **Conclude** (→ concluded)
- `paused` → **Resume** (→ active), **Conclude** (→ concluded)
- `concluded` → terminal: no transition actions
Each action: confirm dialog (Conclude is destructive-styled), optional reason,
call `transitionExperiment`, refresh the page state on success, surface gateway
errors. Gate the controls on `org_admin` (consistent with the Members track;
server remains source of truth). Keep the existing **Recompute** button.

### FR3 — Real Configuration tab
Replace the fabricated rows with values sourced from `ExperimentSummary` /
results: bound flag (`flag_key`), statistical model, status, variant count +
`variant_keys`, resolved primary/secondary metric names, `unit_context_types`,
exclusion-group membership (`exclusion_group_id`), and real timestamps
(`created_at`, `started_at`, `ended_at`). Pre-period/CUPED days only if present
in the results envelope (`pre_period_days`); otherwise omit. **Remove** the
invented Allocation/Targeting/Min-sample/MDE/α-β rows that have no backend
source — do not display fabricated numbers.

### FR4 — Honest Lifecycle timeline
Replace the fully-mocked Lifecycle card with a timeline derived from real fields
(Created → Started → Ended/Concluded, plus current status), using
`created_at`/`started_at`/`ended_at`/`updated_at`. If a stage's timestamp is
absent, omit that stage. No fabricated actors/dates.

### FR5 — Events tab
There is no per-experiment event-stream API. Either remove the Events tab or
replace its body with an honest note pointing to the existing Events page /
metric definitions — no aspirational "live tail" placeholder implying a feature
that doesn't exist.

### FR6 — Metrics tab integrity
The Metrics tab already lists real metric names but shows `—` for Type/
Aggregation/Threshold. Either enrich those columns from the already-fetched
metric definitions (kind/aggregation/goal) or drop the empty columns — no
all-`—` columns that imply missing data.

## Non-Functional Requirements

- **TDD:** Vitest tests for `transitionExperiment` (client) and the pure
  transition-action helper (status → allowed actions), plus presentational
  render tests for the new config/lifecycle/header-actions where feasible
  (node-env: `renderToString` + `?raw` source assertions).
- **Type-safe**, matches existing experiment-page conventions (PageHeader,
  badges, ConfirmDialog, toasts/error surfacing).
- **No backend change.** If a transition the UI offers is rejected by the
  service (invalid transition), surface the error; don't pre-guess beyond the
  documented state machine.

## Acceptance Criteria

1. Header offers only valid transitions for the current status and performs them
   via the transitions endpoint; the dead Stop/Ship-winner buttons are gone.
2. After a transition, the page reflects the new status without a manual reload.
3. Config tab shows only real, sourced values — no fabricated allocation/MDE/
   targeting/CUPED numbers.
4. Lifecycle timeline is derived from real timestamps; no mock actors/dates.
5. Events tab no longer shows an aspirational placeholder; Metrics tab has no
   all-`—` columns.
6. `tsc`, `lint` (0 errors), vitest (green), `build` all pass.

## Out of Scope

- New backend endpoints (allocation/targeting/MDE/α-β config, lifecycle audit
  history, per-experiment event stream) — flag as follow-ups.
- Bandit campaign management UI (separate track, `feature-flag-j38`).
- Creating experiments (already covered by the create modal).
