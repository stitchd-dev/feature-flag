# Track Learnings: experiment_lifecycle_ui_20260610

## Inherited / context

- Admin vitest env is `node`: pure-helper tests, `renderToString` for
  presentational components, `?raw` source assertions for data-driven containers.
  (members_roles_20260610)
- Gateway experiment status strings: `draft` / `running` (=ACTIVE) / `paused` /
  `concluded` (experiments.rs `experiment_status_str`). Transition body
  `{ new_status, reason? }`; `status_from_str` accepts active|running, paused,
  concluded|stopped|completed, draft.
- No experiment-specific entries in `admin/src/lib/permissions.ts` → gate write
  controls on `roles.includes('org_admin')` (server enforces).

## State machine (UI offers valid-only)

- draft → Start (active)
- running → Pause (paused), Conclude (concluded)
- paused → Resume (active), Conclude (concluded)
- concluded → terminal

<!-- Learnings from implementation appended below -->

## Implementation notes (2026-06-10)

- The detail page splits into `ExperimentDetail` (owns `apiExp` state + fetch)
  and `ExperimentDetailBody` (props). To refresh after a transition, pass
  `onExperimentUpdated={setApiExp}` down; `transitionExperiment` returns the
  updated experiment so we `setApiExp(updated)` directly — no refetch needed.
- The header previously had a dead `Stop` button and a `Ship winner` button
  (no onClick); the latter keyed off `display.confidence`. Both removed. The
  `ExperimentDisplay` type import became unused → TS6133 under the strict config
  (tsc -b catches it, not lint). Remove dead type imports immediately.
- `ExpConfig` was ~10 fabricated rows (50/50, 380,000, MDE 2%, CUPED 18%,
  beta-customers targeting). Now sourced entirely from `ExperimentSummary`.
  Lifecycle card uses `lifecycleTimeline(exp)` (Created/Started/Ended from real
  timestamps; absent stages omitted) — no mock actors/dates.
- Metrics tab dropped the all-`—` Type/Aggregation/Threshold columns (kept
  Metric + derived Role); Events tab placeholder replaced with an honest pointer
  to the Events/Metrics pages (no per-experiment event-stream API exists).

## Follow-up candidates (NOT built — no backend source)
- Allocation %, targeting summary, min-sample/MDE/α-β config, CUPED enablement,
  and a lifecycle *audit* history are not exposed by any RPC. Surfacing them
  needs new backend fields/endpoints first.

## Verification note
UI-only track; no backend change. Live E2E needs the full backend stack +
a seeded experiment. Validated via tsc -b clean, lint 0 errors, full vitest
1049 (21 new), vite build.
