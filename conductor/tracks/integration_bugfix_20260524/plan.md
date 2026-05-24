# Plan: integration_bugfix_20260524

## Phase 1: Infrastructure & Full-Stack Bringup

- [x] Task 1: Start Docker databases
  - `docker compose up postgres clickhouse scylladb -d --wait`
  - Run DB migrations: `cargo sqlx migrate run --source crates/stitchd-db/migrations`
  - Verify all three databases are healthy (check container logs + health status)

- [x] Task 2: Build and start all backend services in-process (b644d80)
  - `cargo build --workspace`
  - Start processes: stitchd-auth-service, stitchd-flag-service, stitchd-segmentation-service, stitchd-analytics-service, stitchd-experimentation-service, stitchd-stats-service, stitchd-gateway
  - Each service uses env vars for gRPC port, DB URLs, metrics port (see tech-stack.md for naming)
  - Verify gateway responds at `http://localhost:8080/health` (or equivalent)
  - Any service that panics or refuses to start is recorded as a critical bug
  - BUG-001: STITCHD_AUTH_ENCRYPTION_KEY missing from docker-compose.yml → fixed
  - BUG-002: stats-service default gRPC ports swapped → fixed (b644d80)
  - BUG-003: context_refresher queries stale table → fixed (a7bc8f5)

- [x] Task 3: Start Admin UI and initialise bugs.md (a7bc8f5)
  - `cd admin && npm run dev` → running at http://localhost:5174/
  - Admin UI loads; `/api` proxy confirmed working (JWT login via proxy)
  - bugs.md created with BUG-001, BUG-002, BUG-003

- [x] Task: Conductor - User Manual Verification 'Infrastructure & Full-Stack Bringup' (Protocol in workflow.md)

## Phase 2: Bug Discovery — Auth + Org Management

- [x] Task 1: Superadmin + org management flows
  - Login as superadmin, create org "Acme Corp", list orgs, get org detail ✓
  - BUG-004: created_at null in create org response
  - BUG-007: TopbarNav hardcodes user avatar "PR" and env badge "production"

- [x] Task 2: Org user auth flows
  - Created Alice as org admin; Bob as viewer; both login successfully ✓
  - Password reset: BUG-005 — endpoint missing entirely
  - User invite: BUG-005 — endpoint missing entirely
  - RBAC: BUG-010 (CRITICAL) — viewers can perform write operations

- [x] Task 3: OIDC / SAML / MFA flows
  - Skipped — requires external IdP configuration; gap documented as BUG-005

- [x] Task 4: RBAC + SDK key management
  - BUG-010 (CRITICAL): viewer can create env and revoke SDK keys
  - BUG-008: SDK key name field missing from proto and gateway (silently dropped)
  - BUG-006: GET/DELETE /v1/management/orgs/{id}/users missing in gateway
  - BUG-009 (HIGH): rename env/project returns 502 (double version increment)
  - Min-1-active enforcement: ✓ verified (409 on last key revoke)
  - Environment CRUD: create ✓; rename FAILS (BUG-009); delete not tested

- [x] Task: Conductor - User Manual Verification 'Bug Discovery — Auth + Org Management' (Protocol in workflow.md)

## Phase 3: Bug Discovery — Flags + Segments

- [x] Task 1: Flag CRUD for all five types
  - Created flags of each type; BUG-012 (value_type vs flag_type) and BUG-013 (update ignores type) found
  - BUG-014: enabled silently defaults to false on partial updates
  - Archive/restore: BUG-015 (no restore endpoint), BUG-016 (archived flag not accessible by key), BUG-017 (archive returns pre-archive state), BUG-018 (misleading restore message)
  - Pagination: ✓ page 2 returns correct items

- [x] Task 2: Rule builder — condition trees + segment picker
  - AND multi-condition rule: ✓ (test_bool_flag: plan=beta AND age_days>30)
  - InSegment condition: ✓ (vip_feature with VIP Users List segment)
  - Condition operators: Eq, Gt verified; InSegment verified
  - Rule traces show matched conditions with predicate strings ✓

- [x] Task 3: Hash-input selector + cross-context percentage allocation
  - CRITICAL ✓: cross-context `user.key` + `device.params.os` hash works correctly
  - Hash input: `flag_key + env_id + user.key + device.os` → stable bucket
  - Bucket stability: same inputs → same bucket (891 for user-001/iOS, 3 consecutive runs)
  - Cross-context variation: iOS=891, Android=290, Windows=625 (correctly different buckets)
  - weight_milli format (0–1000): ✓ correct; admin UI uses same format
  - BUG-013: update_flag silently ignores value_type for type change

- [x] Task 4: Evaluate-preview test panel
  - Matching context: ✓ variant + rule trace with conditions
  - Non-matching context: ✓ fallthrough to default, no_match trace shown
  - Multi-context bundle (user + device): ✓ single bundle for cross-context hash
  - CRITICAL ✓: cross-context key+param stable bucket (bucket=891 stable, 3x)
  - Rollout debug: ✓ hash_input, bucket, variant_ranges displayed
  - Context format uses `_type` not `context_type` (undocumented, internal format)

- [x] Task 5: Segment CRUD — rule-based + list-based
  - Rule-based segment: ✓ condition_expr saved correctly
  - List-based segment: ✓ 11 include + 2 exclude keys
  - InSegment evaluation: VIP→on, non-VIP→off, excluded→off ✓
  - Segment soft-delete: ✓ segment removed from list after DELETE
  - Restore: N/A — no restore needed for segments (hard delete)
  - Pagination: ✓ flag list pagination works
  - BUG-019: GET /v1/environments/{env_id}/segments returns 405
  - BUG-020: deleted segment error is just UUID, no descriptive message

- [x] Task: Conductor - User Manual Verification 'Bug Discovery — Flags + Segments' (Protocol in workflow.md)

## Phase 4: Bug Discovery — Events + Metrics + Experiments

- [ ] Task 1: Events full lifecycle
  - Register event for each metric_type (count/conversion/revenue/duration/numeric/custom)
  - custom type: provide valid JSON schema, verify ingestion validates payload
  - custom type: fire event with invalid payload (missing required field), verify rejection
  - TestEventWidget: fire test event from Admin UI; verify firing appears in EventDetail
  - EventDetail: 14-day sparkline loads; recent firings table; back-link "Metrics referencing this event"
  - Archive event: verify UI shows archived badge; verify new firings return HTTP 410
  - Event list pagination

- [ ] Task 2: Metrics full lifecycle
  - Aggregation metric: create for each aggregator (count/sum/avg/p50/p90/p99/uniq)
  - Aggregation with JsonLogic where_clause: enter valid filter expression, verify preview applies it
  - Aggregation on_field: target `value` column vs. a `properties[key]` reference
  - Ratio metric: set numerator + denominator, set min_denominator; verify null bucket display when below threshold
  - Funnel metric: add 3 steps, set window_seconds; reorder steps; verify step labels
  - Metric preview sparkline: verify data loads from ClickHouse (7-day range); verify empty state for new metrics
  - Metric list pagination; goal_direction up/down arrow display
  - Bidirectional back-link: metric detail shows events it references; event detail shows dependent metrics

- [ ] Task 3: Experiment creation + binding
  - Create experiment bound to a percentage-distribution custom rule (flag_rule_id path)
  - Create experiment bound to default_rule (targets_default_rule = true path)
  - XOR enforcement: attempt to bind both simultaneously; verify UI prevents / shows error
  - Unit context types: add `user`, `device`, `account` context types
  - Guardrail metrics: select 1+ metrics; set goal_direction expectations
  - pre_period_days: set value; verify it persists and is shown in experiment detail

- [ ] Task 4: Experiment lifecycle + results display
  - Start experiment → verify whole-flag lock activates
  - Attempt to edit flag variant while locked → verify 409 / UI error message
  - Attempt to edit flag rule while locked → verify 409 / UI error message
  - Pause experiment → verify flag edits still blocked
  - Resume experiment → verify lock remains
  - Experiment results panel: Frequentist t-test + two-proportion Z values per variant
  - Bayesian posteriors + probability-to-beat-control display
  - CUPED variance reduction indicator (if pre_period_days set)
  - SRM chi-square result (over/under-represented warning)
  - Guardrail direction violation flag on metric detail
  - Per-context-type tab strip: switch between user/device/account tabs; verify stats update per tab
  - On-demand recompute: click recompute button; verify `last_computed_at` updates
  - Stop experiment → verify whole-flag lock lifts; verify flag edits succeed again

- [ ] Task: Conductor - User Manual Verification 'Bug Discovery — Events + Metrics + Experiments' (Protocol in workflow.md)

## Phase 5: Bug Discovery — SDK Integration + UI/UX Polish

- [ ] Task 1: Rust SDK integration test
  - Write a small test binary or integration test in `sdks/` or a test module
  - `SdkClient::init(config)` with a real SDK key pointing at the local gateway
  - Verify init blocks until first definition sync completes
  - Evaluate a boolean, string, int, double, and json flag; verify returned variants match UI
  - Compare `evaluate()` result with Admin UI evaluate-preview for same flag + context bundle
  - Multi-context evaluation: provide `[EvalRequest { flag_key, contexts: [user_ctx, device_ctx] }]`
  - CRITICAL cross-context key+param: use hash_inputs with `user.key` + `device.params.os`;
    verify SDK bucket assignment equals the evaluate-preview bucket for the same input
  - Verify bucket is stable across 100 repeated evaluations of the same context

- [ ] Task 2: SDK list-segment + key auth tests
  - Create a list-based segment via UI; add 5+ context keys
  - Write SDK test: evaluate flag with "Is in Segment" rule; verify member keys return correct variant
  - Verify non-member key returns fallback variant
  - Test LFU cache: after `init()`, confirm membership resolves in-process without REST call (add tracing or metric check)
  - Test wrong SDK key: init with key from a different environment; verify connection/auth error
  - Test key rotation: init with key A; revoke A via UI; create key B; verify SDK reconnects with key B

- [ ] Task 3: UI/UX polish sweep
  - Empty states: navigate to flags list with no flags, segments list, experiments list, events list — verify meaningful empty-state messages (not blank page)
  - Loading skeletons: throttle network (devtools); verify skeletons appear on data fetch
  - Error toasts: trigger a 500 from backend (e.g. kill a service); verify user-visible error toast/banner
  - Form validation: submit forms with missing required fields; verify inline errors appear adjacent to fields
  - Form validation: enter type-mismatched values (e.g. string in int field); verify rejection
  - Table column alignment: check flag, segment, experiment, event tables — headers align with data, numeric columns right-aligned
  - Pagination controls: navigate to last page; verify "Next" button disabled; go to first page; verify "Prev" disabled
  - Destructive actions: click delete/archive on a flag, segment, experiment, event; verify confirmation dialog appears
  - Breadcrumb/navigation: traverse flag detail → rule edit → back; verify breadcrumb reflects correct path
  - Org switching: if multiple orgs exist, verify org switcher works without page reload issues
  - Mobile responsiveness: resize browser to 375px width; check that:
    - Navigation collapses to hamburger or similar
    - Tables scroll horizontally or reflow (no overflow clipping)
    - Modals fit within viewport
    - Forms stack vertically without overflow

- [ ] Task: Conductor - User Manual Verification 'Bug Discovery — SDK Integration + UI/UX Polish' (Protocol in workflow.md)

## Phase 6: Bug Fixes

- [ ] Task 1: Fix critical severity bugs (from bugs.md)
  - For each critical bug: implement code fix, write targeted test, smoke-check on live stack
  - Run `cargo test -p <affected_crate>` after each backend fix
  - Run `npm run lint` + `node_modules/.bin/tsc --noEmit` after each frontend fix

- [ ] Task 2: Fix high severity bugs (from bugs.md)
  - Same fix protocol as Task 1; critical must be complete before starting high

- [ ] Task 3: Fix medium severity bugs (from bugs.md)
  - Same fix protocol; targeted test where warranted

- [ ] Task 4: Fix low severity bugs (from bugs.md)
  - Fix and smoke-check; test only where behaviour change is non-trivial

- [ ] Task 5: Regenerate sqlx offline cache + full CI gate run
  - `SQLX_OFFLINE=false cargo sqlx prepare --workspace -- --tests`
  - `cargo fmt --all --check`
  - `cargo clippy --workspace --all-targets -- -D warnings`
  - `cargo test --workspace`
  - `node_modules/.bin/tsc --noEmit -p tsconfig.app.json`
  - `npm run lint`
  - All gates must pass before this phase is considered complete

- [ ] Task: Conductor - User Manual Verification 'Bug Fixes' (Protocol in workflow.md)
