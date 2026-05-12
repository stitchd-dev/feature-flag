# Plan: Feature Flags Full CRUD + Rule Builder
# Track: flags_crud_20260512

---

## Phase 1: Backend — Admin Flag API Extension

> **Context:** The gateway currently returns `FlagJson {key, enabled}` only.
> `create_variant` and `update_rules` handlers are stubs (202 no-op).
> `feature_flags` table has no `name` or `description` columns.
> Rules are stored as `rule_def JSONB` in `feature_flag_rules` and serialized
> to opaque bytes in the proto. The admin API needs to expose rich flag data.

- [x] Task 1: DB migration — add name + description to feature_flags
  - Write failing test: flag repo load with name/description
  - Migration: `ALTER TABLE feature_flags ADD COLUMN name TEXT NOT NULL DEFAULT ''`,
    `ADD COLUMN description TEXT NOT NULL DEFAULT ''`
  - Update `FlagRecord` in `stitchd-core/src/flag.rs` to include `name`,
    `description` fields
  - Update flag service DB repository queries to read/write name + description
  - Update `MutateFlagRequest` flow in `service.rs` to persist name/description

- [~] Task 2: Extend proto + mapping for full admin flag data
  - Write failing test: `build_feature_flag_proto` includes name/description
  - Add `name` and `description` string fields to `FeatureFlag` in
    `proto/flags/v1/flag_sync.proto`
  - Regenerate protobuf (proto change triggers `cargo build`)
  - Update `build_feature_flag_proto` in `mapping.rs` to populate name/description
  - Run: `cargo test -p stitchd-flag-service`

- [ ] Task 3: Admin gateway response type + wired GET endpoints
  - Write failing test: GET flag returns AdminFlagJson shape
  - Add `AdminFlagJson` struct in `routes/flags.rs`:
    `{flag_id, key, name, description, flag_type, enabled, status, version,
    variants: [{key, value}], rules: [{condition: Value, output: Value}],
    default_variant_key, created_at, updated_at}`
  - Rule serialization: decode `rule_payload` bytes → `serde_json::Value`
    (already JSON-encoded `ConditionExpr`); include rule output from proto oneof
  - Update `get_flag` handler to return `AdminFlagJson`
  - Update `list_flags` handler to return `Vec<AdminFlagJson>`
  - Run: `cargo test -p stitchd-gateway`

- [ ] Task 4: Implement create_flag with full fields
  - Write failing test: POST flag with name/description/value_type/variants → 201
  - Update `FlagMutateRequest` to include `name`, `description`, `value_type`,
    `variants: Option<Vec<VariantBody>>`
  - Implement `create_flag` to forward all fields via `MutateFlagRequest`
  - Run: `cargo test -p stitchd-gateway`

- [ ] Task 5: Implement update_variants handler
  - Write failing test: PUT /variants replaces variant list on flag
  - The handler must: GET current flag, replace variants list, call MutateFlag(Update)
  - Understand flag service update semantics (does MutateFlag replace all variants?
    If not, add a dedicated `ReplaceVariants` RPC or handle in gateway via
    get-then-mutate)
  - Input: `{variants: [{key, value}], version}`
  - Run: `cargo test -p stitchd-gateway`

- [ ] Task 6: Implement update_rules handler
  - Write failing test: PUT /rules replaces rule list on flag
  - Input JSON: `{rules: [{condition: <ConditionExpr JSON>, output: {variant_key}
    | {allocation: [{variant_key, weight_milli}]}}, ...], version}`
  - Gateway: deserialize condition JSON → bytes (pass through as-is);
    map output variant keys → VariantKey proto; call MutateFlag(Update)
  - Validate: rules sum to 100% for percentage outputs; at least 1 variant
    exists for variant outputs
  - Run: `cargo test -p stitchd-gateway`

- [ ] Task 7: Archive flag endpoint
  - Write failing test: POST /archive → 200 (soft-delete, MutationKind::Archive)
  - Add `POST /v1/projects/{project_id}/flags/{flag_id}/archive` handler
    using `MutationKind::Archive`
  - Update list_flags to exclude archived flags by default; support
    `?include_archived=true` query param
  - Run: `cargo test -p stitchd-gateway`

- [ ] Task: Conductor - User Manual Verification 'Phase 1' (Protocol in workflow.md)

---

## Phase 2: Frontend — Flag Lifecycle CRUD
<!-- depends: phase1 -->

> All components go in `admin/src/pages/flags/`.
> TypeScript interfaces defined before implementation.
> Verification: `node_modules/.bin/tsc --noEmit -p tsconfig.app.json` + `npm run lint`

- [ ] Task 1: Update TypeScript types to match AdminFlagJson
  - Define `AdminFlagResponse`, `VariantBody`, `RuleJson`, `ConditionJson`
    in `admin/src/lib/types.ts` (new shared types file)
  - Update `FlagsList.tsx` and `FlagDetail.tsx` to use new types
  - Run: `tsc --noEmit`

- [ ] Task 2: Key auto-generation utility
  - Add `slugify(name: string): string` in `admin/src/lib/utils.ts`
    (lowercase, replace spaces/specials with `-`, strip leading/trailing dashes)
  - Verify with manual test in browser console

- [ ] Task 3: CreateFlagModal component
  - Fields: name (required), key (auto-generated, editable, locked after submit),
    description, flag type selector (bool/string/int/double/json),
    initial variant(s) with typed value inputs
  - POST to `/v1/projects/{projectId}/flags`; on success navigate to FlagDetail
  - Show inline field validation errors
  - TypeCheck + lint

- [ ] Task 4: Edit flag metadata (inline on FlagDetail)
  - Add edit mode to FlagDetail header (click-to-edit name + description)
  - PUT `/v1/projects/{projectId}/flags/{key}` with `{name, description, version}`
  - Optimistic update; revert on error with toast

- [ ] Task 5: Edit variants component
  - `VariantEditor` component: list of existing variants; add/remove/rename;
    typed value input based on flag's value_type
  - Prevent removing last variant; warn if removing a variant referenced in rules
  - PUT `/v1/projects/{projectId}/flags/{key}/variants`

- [ ] Task 6: Archive and clone flows
  - Archive: confirmation dialog → POST `/archive`; redirect to FlagsList
  - Clone: modal to enter new key → POST create flag copying variants
  - Add "Show archived" toggle to FlagsList (pass `?include_archived=true`)

- [ ] Task 7: Fully wire enable/disable toggle (optimistic)
  - Toggle flips immediately in UI state
  - PUT to set `enabled` field
  - On error: revert state + toast "Failed to update flag"
  - TypeCheck + lint

- [ ] Task: Conductor - User Manual Verification 'Phase 2' (Protocol in workflow.md)

---

## Phase 3: Frontend — Rule Builder
<!-- depends: phase1 -->

> Rule builder lives in `admin/src/components/rules/`.
> TypeScript condition types defined before any component code.

- [ ] Task 1: TypeScript rule type definitions
  - Define `ConditionExpr`, `Condition` (all variants: Eq/Ne/Lt/Lte/Gt/Gte/
    Contains/StartsWith/EndsWith/InSegment/FlagEvaluated/etc.), `RuleOutput`
    in `admin/src/lib/ruleTypes.ts`
  - These mirror the Rust domain types (serde_json compatible)
  - TypeCheck

- [ ] Task 2: ConditionClauseEditor component
  - Inputs: context_type (free-text + autocomplete), attribute/param (free-text
    + autocomplete), operator selector (enum), value input
  - Operator list driven by inferred value type where possible
  - Context Intelligence: try `GET /v1/context-types` on mount; if 404/error,
    fall back silently to free-text only

- [ ] Task 3: Segment and dependent-flag clause components
  - `SegmentClause`: searchable dropdown fetching
    `GET /v1/environments/{envId}/segments`; shows key + name
  - `DependentFlagClause`: flag selector → then variant selector populated from
    chosen flag's variants
  - Both support negation toggle

- [ ] Task 4: PercentageRolloutEditor component
  - List of (variant, weight%) rows; total must equal 100%
  - Inputs use step=0.1 (0.1% granularity); real-time sum validation
  - "Distribute evenly" helper button

- [ ] Task 5: RuleCard component (single rule)
  - Condition tree display + editor (AND/OR/NOT nesting)
  - Add condition / add group buttons
  - Per-rule NOT toggle at the top level
  - Output selector: "serve variant" or "percentage rollout"
  - Drag handle (HTML5 draggable or pointer events)

- [ ] Task 6: RuleList component with drag-to-reorder
  - Ordered list of RuleCards
  - Drag-to-reorder via `onDragStart`/`onDragOver`/`onDrop` with visual
    drop indicator
  - DefaultRule always rendered last, cannot be moved or deleted
  - "Add rule" button prepends a new blank rule

- [ ] Task 7: Wire rules to API + integrate into FlagDetail
  - Load rules from `AdminFlagJson.rules` on FlagDetail mount
  - "Save rules" button: PUT `/v1/projects/{projectId}/flags/{key}/rules`
    with serialized condition JSON + outputs
  - Track dirty state for unsaved-changes guard
  - TypeCheck + lint

- [ ] Task: Conductor - User Manual Verification 'Phase 3' (Protocol in workflow.md)

---

## Phase 4: Frontend — UX Polish & RBAC
<!-- depends: phase2, phase3 -->

- [ ] Task 1: Unsaved-changes guard
  - Use React Router v7 `useBlocker` in FlagDetail when `isDirty` is true
  - Show confirmation modal before navigating away: "Unsaved changes — leave
    anyway?"

- [ ] Task 2: Reusable ConfirmDialog + Toast system
  - `ConfirmDialog` component (modal with message, confirm/cancel)
  - `useToast` hook + `ToastContainer` in Sidebar/App root
  - Wire all destructive actions and API errors to these primitives

- [ ] Task 3: Empty state for no-rules flag
  - When `rules.length === 0`, show a centered prompt card:
    "No targeting rules yet. All contexts will receive the default variant."
    with an "Add first rule" CTA

- [ ] Task 4: RBAC gating
  - Wrap create/edit/archive/clone/save-rules actions with `usePermissions()`
  - `flag:write` missing → buttons disabled + opacity 0.35
  - No `flag:read` → render LockOverlay over the flags section
  - Follow existing RBAC gating pattern from env_sdk_rbac track

- [ ] Task 5: Optimistic concurrency conflict handling
  - When API returns 409/ABORTED (version mismatch), show toast:
    "Flag was modified by someone else — refresh to reload"
  - Offer a "Refresh" action in the toast that reloads the flag

- [ ] Task 6: FlagsList enhancements
  - Populate `flag_type` badge, `name`, `updated_at` from real AdminFlagJson data
  - Filter bar: type filter chips (bool/string/int/double/json/all)
  - TypeCheck + lint

- [ ] Task: Conductor - User Manual Verification 'Phase 4' (Protocol in workflow.md)
