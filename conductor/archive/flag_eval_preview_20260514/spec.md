# Spec: Flag Evaluation Preview

## Overview

Add an evaluation preview tab to the Flag Detail page in the Stitchd admin UI.
Admins can supply an array of evaluation contexts, trigger a simulated flag
evaluation, and see a full rule-trace breakdown — which rule fired, why each
rule passed or failed, and what variant was returned. This closes the feedback
loop between writing targeting rules and verifying they behave as intended.

## Functional Requirements

### 1. Preview Tab

- New "Preview" tab on the Flag Detail page, alongside "Targeting Rules" and "Variants"
- Only visible when the flag exists (not on the create flow)

### 2. Context Input — Dual Mode

Users enter evaluation contexts as an **array of context objects**
(`[{ "_type": "user", "key": "u-123", "parameters": { ... } }, ...]`).

**JSON Editor mode (default):**
- Syntax-highlighted textarea pre-populated with `[{ "_type": "", "key": "", "parameters": {} }]`
- Validates JSON on submit; surface parse errors inline

**Form Builder mode:**
- Toggle button switches between modes; switching syncs state bidirectionally
- Each array element rendered as a card: `_type` text field, `key` text field,
  `parameters` as key/value pair rows (add/remove rows)
- "Add Context" button appends a new empty context card
- "Remove" button per card (min 1 card enforced)

### 3. Evaluate Action

- "Evaluate" button sends context array to the backend
- Shows a loading state; disables the button during in-flight request
- Backend endpoint: `POST /v1/flags/:flagKey/evaluate-preview` (see §6)

### 4. Results Panel

Rendered per context object in the input array (one result card per context):

**a. Resolved Variant**
- Prominent display of the variant value (e.g. `"blue"`, `true`, `42`)
- Badge indicating `enabled` / `disabled` flag state

**b. Firing Rule**
- Which rule fired: rule index (1-based) + rule name if set, or "Default rule"
- If flag is disabled: show "Flag is disabled — default rule applied regardless of targeting"

**c. Rule-by-Rule Trace**
- Collapsible list of all rules evaluated (in order)
- Per rule: pass ✓ / fail ✗ / skipped (short-circuit after first match)
- Per condition in the firing rule: individual pass/fail with the predicate displayed
  (e.g. `user.country == "US" → true`)

**d. Percentage Rollout (if applicable)**
- When the firing rule uses percentage allocation: show the computed hash input,
  the resulting bucket number (0–999), and which variant bucket it fell into

**e. Disabled-Flag Warning**
- If `flag.status == disabled`, show a banner: "This flag is disabled. All contexts
  return the default rule variant regardless of targeting rules."

### 5. UX Details

- Results clear when the context input changes (stale results are not shown)
- Each context result card is labelled by `_type + key` from the input
- Empty state when no evaluation has been run yet: "Enter contexts above and click Evaluate"
- Error state for API failures: inline error message with the raw error

### 6. Backend API Extension

New gateway endpoint:

```
POST /v1/flags/:flagKey/evaluate-preview
Body: { "contexts": [ EvaluationContext, ... ] }
Response: {
  "flag_status": "enabled" | "disabled",
  "results": [
    {
      "context_index": 0,
      "variant": <typed value>,
      "fired_rule_index": 2 | null,   // null = default rule
      "fired_rule_name": "EU users" | null,
      "rule_traces": [
        {
          "rule_index": 0,
          "outcome": "no_match",
          "conditions": [{ "predicate": "user.country == 'US'", "result": false }]
        },
        ...
      ],
      "rollout_debug": {             // only present when fired rule uses % rollout
        "hash_input": "u-123:my-flag:proj-abc:env-xyz",
        "bucket": 412,
        "variant_ranges": [{ "variant": "blue", "from": 0, "to": 499 }]
      }
    },
    ...
  ]
}
```

- Endpoint performs evaluation in-process using the existing rule engine logic
- Does NOT record an event or affect any counters — preview only
- Requires `flag:read` permission (no write required)
- Returns results in the same order as the input `contexts` array

## Non-Functional Requirements

- JSON↔Form sync must be lossless (round-tripping does not drop fields)
- Preview results are not persisted — in-memory only, cleared on page reload
- RBAC: `flag:read` sufficient; no `flag:write` required

## Acceptance Criteria

- [ ] Preview tab visible on any existing flag's detail page
- [ ] JSON editor and form builder modes sync bidirectionally without data loss
- [ ] Submitting a valid context array returns results with per-rule traces
- [ ] Disabled flag shows warning banner and correct default-rule result
- [ ] Percentage rollout rules show hash + bucket debug info
- [ ] Parse errors in JSON mode are shown inline before submission
- [ ] API errors are shown inline in the results area

## Out of Scope

- Saving / persisting named test contexts (deferred)
- Context type selector / field autocomplete (deferred to Context Intelligence Layer track)
- Evaluating multiple flags at once
- Using real historical contexts from the event log
