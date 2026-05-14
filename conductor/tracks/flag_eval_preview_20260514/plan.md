# Plan: Flag Evaluation Preview (`flag_eval_preview_20260514`)

## Phase 1: Backend — Evaluate Preview Endpoint

- [x] Task: Define `EvaluatePreviewRequest` / `EvaluatePreviewResponse` types in gateway
  - `contexts: Vec<EvaluationContext>`; response mirrors spec §6 shape
- [x] Task: Write failing tests for the preview handler (TDD red phase)
  - Test: flag disabled → all results use default rule with disabled banner
  - Test: context matches rule N → correct variant + trace
  - Test: percentage rollout rule → `rollout_debug` populated
  - Test: RBAC — `flag:read` sufficient, no write required
- [x] Task: Implement rule-trace evaluation logic in `stitchd-flag-service`
  - Run rule engine per context, collecting per-rule pass/fail + per-condition predicates
  - Emit `rollout_debug` when fired rule uses percentage allocation
- [x] Task: Implement `POST /v1/flags/:flagKey/evaluate-preview` handler in gateway
  - Call flag service, map rule traces to response shape; no event emission
- [x] Task: Verify all Phase 1 tests pass + coverage ≥90% for touched crates
- [ ] Task: Conductor — User Manual Verification 'Backend — Evaluate Preview Endpoint' (Protocol in workflow.md)

## Phase 2: Frontend — Preview Tab

- [ ] Task: Write component tests for context input (JSON mode + form builder mode + sync)
  - Test: valid JSON parses into form state
  - Test: form state round-trips back to identical JSON
  - Test: invalid JSON shows inline parse error, blocks submit
  - Test: adding/removing context cards updates JSON editor
- [ ] Task: Scaffold `PreviewTab` component and add "Preview" tab to `FlagDetail`
  - Tab only renders when `flagId` is set (not in create flow)
- [ ] Task: Implement JSON editor mode
  - Syntax-highlighted textarea, pre-populated with `[{ "_type": "", "key": "", "parameters": {} }]`
  - Validate JSON on change; surface parse errors inline
- [ ] Task: Implement form builder mode
  - Context cards: `_type`, `key`, key/value parameter rows; add/remove context cards
- [ ] Task: Bidirectional JSON↔form sync
  - Mode toggle copies current state to the other representation without data loss
- [ ] Task: Implement results panel
  - Per-context result card: resolved variant badge, firing rule label, disabled-flag banner
  - Collapsible rule trace list (pass ✓ / fail ✗ per rule + per-condition predicates)
  - Rollout debug section (hash input, bucket, variant ranges) when present
  - Stale-results clearing when input changes; empty state and error state
- [ ] Task: Wire results panel to `POST /v1/flags/:flagKey/evaluate-preview`
  - Loading state; disable Evaluate button during in-flight request
- [ ] Task: Conductor — User Manual Verification 'Frontend — Preview Tab' (Protocol in workflow.md)
