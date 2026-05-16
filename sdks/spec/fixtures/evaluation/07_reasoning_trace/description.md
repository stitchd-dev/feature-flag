# 07 — Reasoning trace shape

Same flag as scenario 04 (rule-based segment InSegment) but called via
`evaluate_with_reasoning()`. Asserts the `reasoning` field's shape:
- `outcome` matches
- `matched_rule` is populated with rule_id + rule_name
- `segment_evaluations` contains one entry: the consulted rule-based segment
  with `source = "snapshot"` and `matched = true`
- `rollout` is `null` (no percentage allocation in this scenario)

Runners MAY ignore `reasoning.evaluated_at` when diffing (the SDK fills its
own clock).
