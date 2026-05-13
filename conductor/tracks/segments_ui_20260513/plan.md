# Plan: Segments UI (segments_ui_20260513)

## Phase 1: Backend API Audit & Extensions [checkpoint: 0bb16e5]
<!-- depends: -->

- [x] Task 1: Audit existing segment gateway routes and domain model (d03f2bd)
  <!-- files: crates/stitchd-gateway/src/routes/, crates/stitchd-core/src/, crates/stitchd-proto/ -->
  - Inspect `crates/stitchd-gateway/src/routes/` for any existing `/segments` handlers
  - Inspect `crates/stitchd-core/src/` for `Segment` domain types
  - Inspect proto definitions for `Segment` message and service RPCs
  - Document gaps: missing endpoints, missing proto fields, missing RBAC permissions

- [x] Task 2: Implement missing Segment CRUD endpoints (d03f2bd)
  <!-- files: crates/stitchd-gateway/src/routes/segments.rs, crates/stitchd-gateway/src/routes/mod.rs -->
  <!-- depends: task1 -->
  - Ensure all five routes exist and are wired: GET list, POST create, GET one, PUT update, DELETE
  - Each handler: validate env-scoped ownership, map proto ↔ `AdminSegmentJson` response shape
  - Return HTTP 409 if segment name already exists in environment
  - Return HTTP 400 if condition JSON is malformed

- [x] Task 3: Extend flag rule domain model for segment rule type (d03f2bd)
  <!-- files: crates/stitchd-core/src/domain/, crates/stitchd-proto/proto/, crates/stitchd-gateway/src/mapping.rs -->
  <!-- depends: task1 -->
  - `InSegment`/`NotInSegment` already existed in condition engine; extended `RuleJson` with `segment_ids`
  - Extended proto `AdminSegment` message and 5 admin RPCs
  - Updated `AdminFlagJson` rule serialisation to include `segment_ids` field

- [x] Task 4: Wire RBAC — segment permissions (d03f2bd)
  <!-- files: crates/stitchd-auth-service/src/rbac.rs, crates/stitchd-gateway/src/middleware/ -->
  <!-- depends: task1 -->
  - Added `segment:read`, `segment:write` to `stitchd-auth-service/src/rbac.rs`
  - `org_admin` gets both; `org_member` gets `segment:read` only
  - `require_permission()` enforced in each handler

- [x] Task 5: Tests for backend segment endpoints (d03f2bd)
  <!-- files: crates/stitchd-gateway/src/routes/segments.rs -->
  <!-- depends: task2, task3, task4 -->
  - 15 integration tests: list, create, get, update, delete — happy paths + error cases
  - Duplicate name → 409; bad condition JSON → 400; org_member → 403 on writes
  - All 118 gateway tests pass; clippy clean

- [x] Task: Conductor — User Manual Verification 'Phase 1: Backend API Audit & Extensions' (Protocol in workflow.md)

## Phase 2: Segments CRUD UI [checkpoint: 9de2306]
<!-- depends: -->

- [x] Task 1: Add Segments nav item to environment sidebar (a9b07b6)
  <!-- files: admin/src/components/Sidebar.tsx, admin/src/App.tsx -->
  - Sidebar nav item pre-existing and correct; routes registered in App.tsx — no changes needed

- [x] Task 2: Segments list page (a9b07b6)
  <!-- files: admin/src/pages/segments/SegmentsList.tsx -->
  <!-- depends: task1 -->
  - Table: Name, Description, Tags (chips), Conditions (#), Created
  - Loading skeleton + empty state + search + modal triggers

- [x] Task 3: Create Segment modal (a9b07b6)
  <!-- files: admin/src/pages/segments/CreateSegmentModal.tsx -->
  <!-- depends: task2 -->
  - Fields: Name (required), Description, Tags, User List textarea
  - Submit → `POST /v1/segments`; inline validation; closes + refreshes list on success

- [x] Task 4: Edit Segment modal (a9b07b6)
  <!-- files: admin/src/pages/segments/EditSegmentModal.tsx -->
  <!-- depends: task2 -->
  - Pre-populated from `GET /v1/segments/:id`; submit → `PUT /v1/segments/:id`

- [x] Task 5: Delete Segment confirmation modal (a9b07b6)
  <!-- files: admin/src/pages/segments/DeleteSegmentModal.tsx -->
  <!-- depends: task2 -->
  - Shows flag reference count warning; confirms → `DELETE /v1/segments/:id`

- [x] Task 6: TypeScript type-check + lint for Phase 2 (a9b07b6)
  <!-- files: admin/src/pages/segments/ -->
  <!-- depends: task3, task4, task5 -->
  - `node_modules/.bin/tsc --noEmit -p tsconfig.app.json` — zero errors
  - `npm run lint` — zero warnings
  - 13 Vitest unit tests passing

- [x] Task: Conductor — User Manual Verification 'Phase 2: Segments CRUD UI' (Protocol in workflow.md)

## Phase 3: Flag Rule Builder — "Match Segment" Integration
<!-- depends: phase1, phase2 -->

- [~] Task 1: Add "Match Segment" rule type to the rule builder
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
