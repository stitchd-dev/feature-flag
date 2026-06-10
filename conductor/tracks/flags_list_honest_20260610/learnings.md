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
