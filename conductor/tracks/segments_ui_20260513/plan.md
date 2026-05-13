# Plan: Segments UI (segments_ui_20260513)

## Phase 1: Backend API Audit & Extensions
<!-- depends: -->

- [ ] Task 1: Audit existing segment gateway routes and domain model
  <!-- files: crates/stitchd-gateway/src/routes/, crates/stitchd-core/src/, crates/stitchd-proto/ -->
  - Inspect `crates/stitchd-gateway/src/routes/` for any existing `/segments` handlers
  - Inspect `crates/stitchd-core/src/` for `Segment` domain types
  - Inspect proto definitions for `Segment` message and service RPCs
  - Document gaps: missing endpoints, missing proto fields, missing RBAC permissions

- [ ] Task 2: Implement missing Segment CRUD endpoints
  <!-- files: crates/stitchd-gateway/src/routes/segments.rs, crates/stitchd-gateway/src/routes/mod.rs -->
  <!-- depends: task1 -->
  - Ensure all five routes exist and are wired: GET list, POST create, GET one, PUT update, DELETE
  - Each handler: validate env-scoped ownership, map proto ↔ `AdminSegmentJson` response shape
  - Return HTTP 409 if segment name already exists in environment
  - Return HTTP 400 if condition JSON is malformed

- [ ] Task 3: Extend flag rule domain model for segment rule type
  <!-- files: crates/stitchd-core/src/domain/, crates/stitchd-proto/proto/, crates/stitchd-gateway/src/mapping.rs -->
  <!-- depends: task1 -->
  - Add `SegmentRule { segment_id: Uuid }` variant to `RulePayload` / condition enum in core
  - Extend proto `FlagRule` with a `segment_id` field (oneof or separate field)
  - Update `mapping.rs` proto ↔ domain conversions
  - Update `AdminFlagJson` rule serialisation to include `segment_id` + `segment_name`

- [ ] Task 4: Wire RBAC — segment permissions
  <!-- files: crates/stitchd-auth-service/src/rbac.rs, crates/stitchd-gateway/src/middleware/ -->
  <!-- depends: task1 -->
  - Add `segment:read`, `segment:write` to the permission enum in `stitchd-auth-service`
  - Expand `org_admin` role to include both; `org_member` gets `segment:read` only
  - Gate segment write routes behind `require_permission("segment:write")`

- [ ] Task 5: Tests for backend segment endpoints
  <!-- files: crates/stitchd-gateway/src/routes/segments.rs -->
  <!-- depends: task2, task3, task4 -->
  - Integration tests: list, create, get, update, delete — happy paths + error cases
  - Test: duplicate name → 409; bad condition JSON → 400
  - Test: `org_member` cannot create/update/delete (403); can list (200)
  - `cargo clippy` + `cargo test -p stitchd-gateway`

- [ ] Task: Conductor — User Manual Verification 'Phase 1: Backend API Audit & Extensions' (Protocol in workflow.md)

## Phase 2: Segments CRUD UI
<!-- depends: -->

- [ ] Task 1: Add Segments nav item to environment sidebar
  <!-- files: admin/src/components/Sidebar.tsx, admin/src/App.tsx -->
  - Add "Segments" entry to the env-scoped sidebar (between Flags and SDK Keys or similar)
  - Route: `/org/:orgId/project/:projectId/env/:envId/segments`
  - Active-state highlight follows existing sidebar pattern

- [ ] Task 2: Segments list page
  <!-- files: admin/src/pages/segments/SegmentList.tsx -->
  <!-- depends: task1 -->
  - Table columns: Name, Description, Tags (chips), Conditions (#), Created
  - Loading skeleton + empty state ("No segments yet. Create your first segment.")
  - "New Segment" button top-right → opens Create modal
  - Per-row action menu: Edit, Delete

- [ ] Task 3: Create Segment modal
  <!-- files: admin/src/pages/segments/CreateSegmentModal.tsx -->
  <!-- depends: task2 -->
  - Fields: Name (required), Description (optional), Tags (chip input)
  - Condition builder: reuse existing `RuleList` / condition components
  - User List section: textarea for explicit user keys (one per line)
  - Submit → `POST /v1/segments`; inline validation; success → close modal + refresh list

- [ ] Task 4: Edit Segment modal
  <!-- files: admin/src/pages/segments/EditSegmentModal.tsx -->
  <!-- depends: task2 -->
  - Pre-populate from `GET /v1/segments/:id`
  - Same form as Create; submit → `PUT /v1/segments/:id`
  - Optimistic list update on success

- [ ] Task 5: Delete Segment confirmation modal
  <!-- files: admin/src/pages/segments/SegmentList.tsx -->
  <!-- depends: task2 -->
  - Include flag reference count in GET one response or separate endpoint
  - Warning copy: "This segment is used by N flag(s). Deleting it will remove those rules."
  - Confirm → `DELETE /v1/segments/:id`; remove from list

- [ ] Task 6: TypeScript type-check + lint for Phase 2
  <!-- files: admin/src/pages/segments/ -->
  <!-- depends: task3, task4, task5 -->
  - `node_modules/.bin/tsc --noEmit -p tsconfig.app.json`
  - `npm run lint`
  - Vitest unit tests for new components

- [ ] Task: Conductor — User Manual Verification 'Phase 2: Segments CRUD UI' (Protocol in workflow.md)

## Phase 3: Flag Rule Builder — "Match Segment" Integration
<!-- depends: phase1, phase2 -->

- [ ] Task 1: Add "Match Segment" rule type to the rule builder
  <!-- files: admin/src/components/rules/RuleList.tsx, admin/src/lib/ruleTypes.ts -->
  - Extend `RuleType` union / discriminated union in `ruleTypes.ts`
  - New `SegmentRuleRow` component: renders `User is in segment [dropdown]`
  - Dropdown lazy-loads `GET /v1/segments?env_id=<envId>` on first open; searchable

- [ ] Task 2: Save & load segment rules from API
  <!-- files: admin/src/pages/flags/FlagDetail.tsx, admin/src/lib/ruleTypes.ts -->
  <!-- depends: task1 -->
  - Serialise `SegmentRuleRow` → `{ type: "segment", segment_id: "..." }` in rule payload
  - Deserialise existing flag rules that contain `segment_id` → render `SegmentRuleRow`
  - Display segment name as a coloured badge; clicking opens segment in new tab

- [ ] Task 3: Tests for rule builder segment integration
  <!-- files: admin/src/lib/ruleTypes.test.ts, admin/src/components/rules/ -->
  <!-- depends: task2 -->
  - Vitest: `SegmentRuleRow` renders correct dropdown options
  - Vitest: serialise/deserialise round-trip for segment rule payload
  - `node_modules/.bin/tsc --noEmit -p tsconfig.app.json`

- [ ] Task: Conductor — User Manual Verification 'Phase 3: Flag Rule Builder Integration' (Protocol in workflow.md)

## Phase 4: Quality Gates & Polish
<!-- depends: phase3 -->

- [ ] Task 1: Rust coverage & final clippy
  - `cargo tarpaulin -p stitchd-gateway -p stitchd-core` — verify ≥90%
  - `cargo clippy --workspace --all-targets -- -D warnings` — zero warnings
  - `cargo fmt --all --check`

- [ ] Task 2: Frontend build verification
  - `npm run build` — zero errors
  - `npm run lint` — zero warnings
  - Full `node_modules/.bin/tsc --noEmit` pass

- [ ] Task 3: End-to-end smoke test
  - Start gateway + admin UI; create a segment, attach to a flag rule, verify evaluation

- [ ] Task: Conductor — User Manual Verification 'Phase 4: Quality Gates & Polish' (Protocol in workflow.md)
