# Track Learnings: flags_list_honest_20260610

## Context
- Area 5 reassessment: PreviewMetric already wired (EditMetricModal.tsx, POST
  /v1/metrics/{id}/preview); hash-inputs UI already exists (HashInputSelectorList
  + hashInputSchema in the default-rule editor). Only genuine gap = flags-list
  fabricated columns (30d evals empty sparkline, Segments —, Owner —).
- No batch/summary eval-stats endpoint; per-flag time-series fetch on a 50-row
  list is wasteful → remove the column rather than fan out 50 requests. Real
  per-flag analytics live on the detail Analytics tab (/eval-stats).

<!-- impl notes below -->

## Impl (2026-06-10)
Removed the 30d-evals (empty Sparkline + —), Segments (—), Owner (—) columns
from the flags table header + FlagTableRow; dropped the unused Sparkline import.
Card/grouped layouts already showed only real data. Gate: tsc clean, lint 0
errors, vitest 1053 (4 new), build. Follow-up filed: list-level 30d eval summary
needs a batch/summary eval-stats endpoint (per-flag time-series fetch on a
50-row list is wasteful; real analytics remain on the detail Analytics tab).
