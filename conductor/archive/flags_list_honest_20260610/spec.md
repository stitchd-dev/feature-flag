# Spec: Honest Flags List (Flag capability parity)

**Track ID:** `flags_list_honest_20260610`
**Type:** Chore (UI honesty / dead-placeholder removal)
**Beads:** `feature-flag-u42` (wisp `feature-flag-wisp-99m`)

## Overview

Area 5 was scoped as "flag capability parity: `UpdateFlagHashing` has no UI,
flags-list sparklines are placeholders, `PreviewMetric` unused." On inspection,
two of the three are **already implemented**:

- **`PreviewMetric` is wired** — `admin/src/pages/metrics/EditMetricModal.tsx`
  posts to `POST /v1/metrics/{id}/preview` and renders the preview series.
- **Hash-inputs UI exists** — `HashInputSelectorList` + `hashInputSchema` drive
  the default-rule distribution's `hash_inputs` in `EditFlagDefaultRule.tsx`.

The genuine remaining gap is the **flags list table** (`FlagsList.tsx`,
`FlagTableRow`): three columns render fabricated/empty placeholders with no
backend source on the list:

- **30d evals** → `<Sparkline data={[]} />` + a literal `—` (no list-level
  eval-stats; per-flag time-series would be 50 requests/page and is wasteful).
- **Segments** → literal `—` (no segments-referencing-flag data on the list).
- **Owner** → literal `—` (flags have no owner concept in the backend).

These imply data the page never loads. Real per-flag analytics already live on
the flag detail **Analytics** tab (`AnalyticsTab.tsx`, `/eval-stats`). This
track makes the list honest by removing the three placeholder columns.

## Functional Requirements

### FR1 — Remove the placeholder columns (table layout)
Remove the `30d evals`, `Segments`, and `Owner` `<th>`s from the table header
and the corresponding `<td>`s from `FlagTableRow`. Keep: toggle, Key (+badges),
Type, Status, Updated, chevron. Remove the now-unused `Sparkline` import if no
other usage remains in the file.

### FR2 — No regressions to the cards / grouped layouts
The card layout (`FlagCard`) already shows only real data (version + updated).
Confirm grouped layout reuses `FlagTableRow` and inherits the cleanup; no
fabricated placeholders remain in any layout.

### FR3 — No fabricated data anywhere on the list
After the change, every column on every layout maps to a real
`AdminFlagResponse` field (or a real derived value like prerequisite badges).

## Non-Functional Requirements

- **TDD/where feasible:** a `?raw` source-contract test asserting the fabricated
  columns/headers are gone and `Sparkline` is no longer imported by FlagsList.
- Type-safe; matches existing list conventions; no behavioural change to
  toggle/pagination/search/filter.

## Acceptance Criteria

1. The flags table no longer shows `30d evals` / `Segments` / `Owner` columns.
2. No `<Sparkline data={[]} />` (or any empty-data sparkline) renders on the list.
3. `tsc`, `lint` (0 errors), vitest (green), `build` pass.

## Out of Scope

- A real list-level 30d-eval summary (needs a batch/summary eval-stats endpoint
  — filed as a follow-up). Per-flag analytics remain on the detail Analytics tab.
- Hashing UI and metric-preview (already implemented).
