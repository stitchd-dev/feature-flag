# Revisions: flag_lifecycle_20260604

## Revision #1 — 2026-06-05
- **Type:** Both (plan-heavy: new Phase 10; light spec: Revision History + Last-Revised marker)
- **When:** After Phases 1–9 completed (track had been marked `[x]`; re-opened to `[~]`).
- **Trigger:** Three behaviours shipped in a deferred/partial form during implementation and were
  filed as follow-ups; the user chose to fold them into the track plan for completion rather than
  leave them as loose beads.
- **Changes:**
  - **plan.md:** added **Phase 10 — Follow-Up Completions** (3 tasks + manual-verification),
    `<!-- depends: phase9 -->`, sequential (tasks 1 & 2 both touch experimentation-service).
  - **spec.md:** added a Revision History section + Last-Revised marker. No requirement removed;
    the three items refine A4 (segment list-gen activation) and C4 (flag_variant exact verify +
    start-prereq readability), which were already in scope but partially implemented.
  - **tracks.md:** status `[x]` → `[~]` (more work to do).
- **Phase 10 tasks:**
  1. Flag-service variant-UUID exposure → `flag_variant` start-prereqs verify exactly (was
     `feature-flag-bun`; previously fail-closed).
  2. Experiment start-prerequisite read RPC → populate the dependency-graph experiment branch +
     Admin UI (was `feature-flag-coe`; previously write/enforce-only).
  3. Segment list-generation activation RPC → scheduled `list_generation` segment changes fire
     (previously rejected — no activation RPC existed).
- **Impact:** +1 phase (3 implementation tasks). Beads: `feature-flag-bun` + `feature-flag-coe`
  closed as folded-in; new milestone `feature-flag-hp5.10` + 3 stories created under the epic.
  No existing code changed by this revision (planning only); implement via `/conductor-implement`.
