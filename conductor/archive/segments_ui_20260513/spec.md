# Spec: Segments UI

## Overview

Add a Segments management UI to the Stitchd admin console. Segments are environment-scoped
collections of targeting rules (condition trees and/or explicit user lists) that can be
reused across multiple feature flags. This track covers: full CRUD UI for segments, backend
API extensions where gaps exist, and integration of a "Match Segment" rule type into the
existing flag rule builder.

## Functional Requirements

### 1. Segments List Page
- Accessible from the environment sidebar (environment-scoped, lives alongside Flags)
- Displays all segments for the current org / project / environment
- Shows: segment name, description, tags, condition count, created-at date
- Actions per row: Edit, Delete (with confirmation modal)
- "New Segment" button opens Create Segment modal

### 2. Create Segment Modal
- Fields:
  - Name (required, unique within environment)
  - Description (optional)
  - Tags (optional, multi-value chip input)
- Condition builder section:
  - AND / OR group logic — reuses the existing `RuleList` / `ConditionExpr` component
  - Supports all existing condition operators (eq, neq, gt, lt, in, not_in, contains, wildcard)
- User List section (optional):
  - Textarea / tag input for explicit user IDs / keys
  - Stored alongside conditions (OR-ed with condition tree at evaluation time)
- Submit → POST to segments API

### 3. Edit Segment Page / Modal
- Pre-populates all fields from existing segment
- Same condition builder and user list UI as Create
- Submit → PATCH / PUT to segments API

### 4. Delete Segment
- Confirmation modal: "This segment is referenced by N flags. Deleting it will remove those
  targeting rules. Proceed?"
- DELETE to segments API

### 5. Segment Rule Type in Flag Rule Builder
- In the flag targeting tab's rule builder, add a new rule type: **"Match Segment"**
- Renders as: `User is in segment [dropdown of environment segments]`
- Dropdown is searchable; loads segments via GET /segments?env=<envId>
- When saved, stored as a `SegmentRule` in the flag's rule list (alongside existing `ConditionRule`)
- Display: segment name as a badge/chip; clicking opens the segment detail in a side panel or
  new tab

### 6. Backend API Extensions (where gaps exist)
- Audit existing gateway segment routes; implement any missing:
  - `GET    /v1/segments?env_id=<id>` — list segments
  - `POST   /v1/segments`             — create
  - `GET    /v1/segments/:id`         — get one
  - `PUT    /v1/segments/:id`         — update
  - `DELETE /v1/segments/:id`         — delete
- Extend flag rule proto / domain model to support `segment_id` rule variant if not present
- Wire RBAC: `segment:read`, `segment:write` permissions (org_admin only for write)

## Non-Functional Requirements

- Segment dropdown in rule builder loads lazily (only when rule type = "Match Segment")
- Delete operation checks flag references and warns before proceeding
- All API calls use the existing auth + env-scoped request pattern
- Consistent with existing admin UI design system (same modals, tables, buttons, badges)

## Acceptance Criteria

- [ ] Segments list page loads and shows all segments for current environment
- [ ] Can create a segment with name, description, tags, AND/OR condition groups, and user list
- [ ] Can edit an existing segment; changes persist via API
- [ ] Can delete a segment; deletion blocked or warned if flags reference it
- [ ] Flag rule builder has a "Match Segment" rule type; saves and displays correctly
- [ ] Backend endpoints (list, create, get, update, delete) all respond correctly with RBAC
- [ ] No TypeScript errors, no Clippy warnings, all new code covered by tests

## Out of Scope

- Segment usage analytics / evaluation metrics
- Bulk import of user lists from CSV
- Segment versioning / history
- Cross-environment segment cloning
