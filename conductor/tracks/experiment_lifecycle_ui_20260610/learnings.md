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
