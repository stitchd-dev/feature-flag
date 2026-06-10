# Implementation Plan: Honest Flags List

Track: `flags_list_honest_20260610` · Beads epic: TBD · Branch: `track/flags_list_honest_20260610`

Gates: `tsc -b`, `lint`, vitest (`CI=true`), `build`.

## Phase 1: Remove fabricated columns + verify

- [ ] Task 1.1 (TDD): `?raw` source-contract test for FlagsList — asserts no
      `30d evals` / `Segments` / `Owner` headers, no `Sparkline` import, no
      `data={[]}` sparkline. Confirm red.
      <!-- files: admin/src/pages/flags/FlagsList.test.ts -->
- [ ] Task 1.2 (Green): Remove the three `<th>`s and matching `<td>`s in
      `FlagTableRow`; drop the unused `Sparkline` import. Verify cards/grouped
      layouts carry no fabricated placeholders.
      <!-- files: admin/src/pages/flags/FlagsList.tsx -->
- [ ] Task 1.3: Full gate — `tsc`, `lint`, vitest (`CI=true`), `build`. Update
      learnings; file the list-level eval-summary follow-up in beads.
- [ ] Task: Conductor - User Manual Verification 'Phase 1' (Protocol in workflow.md)
