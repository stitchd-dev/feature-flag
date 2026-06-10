# Implementation Plan: Experiment Lifecycle UI

Track: `experiment_lifecycle_ui_20260610` · Beads epic: TBD · Branch: `track/experiment_lifecycle_ui_20260610`

Methodology: TDD per `conductor/workflow.md`. Gates: `tsc -b`, `lint`, vitest (`CI=true`), `build`.

## Phase 1: Transition client + action helper

- [x] Task 1.1 (TDD): Failing tests — `transitionExperiment` posts `{new_status,reason}`
      to the transitions URL and returns the experiment; pure `allowedTransitions(status)`
      helper returns the correct action set per status (draft/running/paused/concluded).
      <!-- files: admin/src/lib/api.experiments.test.ts, admin/src/pages/experiments/lifecycleHelpers.test.ts -->
- [x] Task 1.2 (Green): Implement `transitionExperiment` in api.ts and
      `allowedTransitions` + action metadata in `experiments/lifecycleHelpers.ts`.
      <!-- files: admin/src/lib/api.ts, admin/src/pages/experiments/lifecycleHelpers.ts -->
- [x] Task: Conductor - User Manual Verification 'Phase 1' (Protocol in workflow.md)

## Phase 2: Header transition controls

- [x] Task 2.1 (TDD): Render/source tests — header offers only valid actions per
      status; no dead Stop/Ship-winner buttons remain (source assertion).
      <!-- files: admin/src/pages/experiments/ExperimentDetail.test.tsx -->
- [x] Task 2.2 (Green): Replace the dead header buttons with a `LifecycleActions`
      control (confirm dialog + optional reason; org_admin-gated) that calls
      `transitionExperiment` and refreshes experiment state on success. Keep Recompute.
      <!-- files: admin/src/pages/experiments/ExperimentDetail.tsx, admin/src/pages/experiments/LifecycleActions.tsx -->
- [x] Task: Conductor - User Manual Verification 'Phase 2' (Protocol in workflow.md)

## Phase 3: Real Configuration + Lifecycle tab

- [x] Task 3.1 (TDD): Render tests for `ExpConfig` given a real `ExperimentSummary`
      — shows flag/model/status/variants/metrics/context-types/timestamps; asserts
      the fabricated allocation/MDE/targeting/CUPED strings are gone.
      <!-- files: admin/src/pages/experiments/ExpConfig.test.tsx -->
- [x] Task 3.2 (Green): Rewrite `ExpConfig` to source rows from real data; replace
      the mock Lifecycle card with a timestamp-derived timeline (omit absent stages).
      <!-- files: admin/src/pages/experiments/ExperimentDetail.tsx -->
- [x] Task: Conductor - User Manual Verification 'Phase 3' (Protocol in workflow.md)

## Phase 4: Events + Metrics tab integrity + verify

- [x] Task 4.1: Replace the aspirational Events placeholder with an honest note
      (or remove the tab); ensure the Metrics tab has no all-`—` columns (enrich
      from metric definitions or drop the columns).
      <!-- files: admin/src/pages/experiments/ExperimentDetail.tsx -->
- [x] Task 4.2: Full gate — `tsc`, `lint`, vitest (`CI=true`), `build`. Fix
      fallout. Update `learnings.md` + file backend follow-ups in beads.
- [x] Task: Conductor - User Manual Verification 'Phase 4' (Protocol in workflow.md)
