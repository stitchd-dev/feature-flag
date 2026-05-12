# Spec: Feature Flags Full CRUD + Rule Builder

## Overview

Wire the Feature Flags admin UI to the real backend APIs. Currently `FlagsList`
is connected but `FlagDetail` falls back to mock data for rules and variants.
This track delivers a complete flag management experience: create/edit/archive/
clone flags, manage typed variants, and build targeting rules visually — all
backed by the live flag service.

## Functional Requirements

### 1. Flag Lifecycle
- **Create flag**: Form with name, key (auto-generated from name, editable),
  description, flag type (bool/string/int/double/json), initial status.
- **Edit flag metadata**: Inline edit of name and description on FlagDetail.
- **Edit variants**: Add/remove/rename variants with typed values (must match
  flag's `flag_type`). At least one variant required.
- **Enable/disable**: Fully wired toggle with optimistic UI (instant flip,
  revert + toast on API error).
- **Archive (soft-delete)**: Confirmation dialog; archived flags hidden from
  default list (add "Show archived" toggle).
- **Clone flag**: Copy with new key, optionally carrying over variants and rules.

### 2. Rule Builder
Rules are evaluated in order; first matching rule wins.

Each rule has:
- **Condition tree**: AND combinator at top level; each clause supports per-rule
  NOT (negate the entire condition).
- **Clause types**:
  - Attribute comparison: `context_type.attribute operator value` — operators:
    `==`, `!=`, `in`, `not_in`, `>`, `<`, `>=`, `<=`, `contains`,
    `starts_with`, `ends_with`, wildcard (`*`)
  - Segment membership: "Is in Segment [key]" (negatable)
  - Dependent flag: "Flag [key] evaluated with variant [variant]"
- **Output**: specific variant OR percentage rollout (0.1% granularity;
  allocations must sum to 100%).
- **Drag-to-reorder**: drag handles on each rule row.
- **Default rule**: always-last fallback rule (cannot be deleted), serves a
  specific variant when no rule matches.

### 3. Context Attribute Autocomplete
- Attribute fields use free-text input.
- When the Context Intelligence API is reachable, suggest known attribute names
  with observed types as a typeahead dropdown.
- Gracefully falls back to plain text if the API is unavailable.

### 4. UX Polish
- **Optimistic toggle**: instant enable/disable; reverts with toast on error.
- **Unsaved-changes guard**: prompt before navigating away from unsaved edits.
- **Confirmation dialogs**: archive flag; delete a variant referenced in a rule.
- **Empty state**: new flag with no rules shows "Add your first targeting rule".
- **Key auto-generation**: slugified from name, editable before first save;
  locked after creation.

## Non-Functional Requirements
- All mutations use optimistic concurrency (`version` field); surface conflicts
  as "Flag was modified by someone else — refresh to reload".
- RBAC: create/edit/archive/enable/disable require `flag:write`; read requires
  `flag:read`. Use existing `usePermissions()` + RBAC gating pattern
  (disabled+opacity for missing perms, LockOverlay for zero read).
- All new TypeScript must pass `node_modules/.bin/tsc --noEmit -p
  tsconfig.app.json` with no errors.
- ESLint must pass with no errors (`npm run lint`).

## Acceptance Criteria
- [ ] Can create a flag of each type (bool, string, int, double, json) with
      at least one variant
- [ ] Key is auto-generated from name, editable before save, locked after
- [ ] Can edit name, description, and variants on the detail page
- [ ] Enable/disable toggle is optimistic with error revert and toast
- [ ] Can archive a flag with confirmation; hidden from default list
- [ ] Can clone a flag
- [ ] Can add, edit, remove, and reorder rules
- [ ] All clause types (attribute, segment, dependent flag) and all operators
      are supported
- [ ] Attribute field shows autocomplete from Context Intelligence API
      (falls back to free-text)
- [ ] Percentage rollout enforces 0.1% granularity and 100% sum
- [ ] Unsaved-changes guard fires on navigation away
- [ ] Confirmation shown before archive and before deleting a variant used
      in rules
- [ ] RBAC gating applied: write ops disabled/dimmed for org_member

## Out of Scope
- Audit log for flag mutations (Track E)
- Experiment creation tied to flag rules (Track D)
- Scheduled flag enable/disable
- ClickHouse-backed evaluation sparklines (backend not yet wired)
- Client-side SDK / streaming flag updates
