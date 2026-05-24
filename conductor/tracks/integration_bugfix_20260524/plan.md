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

- [ ] Task 1: Superadmin + org management flows
  - Login as superadmin → verify `/superadmin/*` route access
  - Create a new org → verify redirect to org detail
  - Superadmin: list all orgs, view org detail, manage org users
  - Document layout/UX issues in superadmin UI (empty states, table alignment)

- [ ] Task 2: Org user auth flows
  - Org user password login → `/org/:orgId/*` routing
  - Password reset: request email, use reset link, set new password, login
  - User invite: send invite email, accept invite link, complete registration, login
  - Session expiry: let token expire (or manipulate exp), verify correct error/redirect

- [ ] Task 3: OIDC / SAML / MFA flows
  - OIDC provider config form: fill, save, verify persisted correctly; load + edit
  - SAML provider config form: fill, save, verify; load + edit
  - MFA enrollment: enable TOTP, scan QR (or copy secret), enter verification code
  - MFA login: login with password + TOTP code
  - Recovery codes: display, copy, use one to bypass TOTP

- [ ] Task 4: RBAC + SDK key management
  - Assign roles (admin/member/viewer) to org users; verify UI hides restricted actions per role
  - SDK key: create key scoped to project+env, list keys, revoke; verify revoked key is rejected
  - Min-1-active enforcement: attempt to revoke last key, verify error
  - SDK key rotation: create second key, revoke first, verify service still works with new key
  - Environment CRUD: create, rename, delete environment; verify cascade to SDK keys

- [ ] Task: Conductor - User Manual Verification 'Bug Discovery — Auth + Org Management' (Protocol in workflow.md)

## Phase 3: Bug Discovery — Flags + Segments

- [ ] Task 1: Flag CRUD for all five types
  - Create flag of each type (bool, string, int, double, json); verify type shown in list
  - Edit flag: name, description, type change restrictions
  - Variant management: add/edit/delete variants; verify type mismatch is rejected
  - Default variant assignment; flag-level default-rule distribution config
  - Archive flag → verify archived state in list; restore flag
  - Flag list pagination: navigate to page 2+, verify URL updates to `?page=N`

- [ ] Task 2: Rule builder — condition trees + segment picker
  - Add a rule with AND conditions (multiple conditions all must match)
  - Add an OR group within a rule
  - Toggle NOT on a condition; verify evaluation inversion
  - Condition operators: string (equals/contains/starts_with/ends_with/regex), int/double (=/</>), bool (is), semver (=/</>)
  - "Is in Segment" condition: open segment picker, search/filter, select segment
  - "Flag evaluated with variant X" condition: select flag + variant
  - Rule ordering: add 3+ rules, drag/reorder, verify persistence

- [ ] Task 3: Hash-input selector + cross-context percentage allocation
  - Add percentage-rollout output to a rule; verify allocation slider and total validation
  - Open HashInputSelectorList; add a ContextKey selector (context_type only)
  - Add a ContextParameter selector (context_type + parameter name); verify autocomplete
  - CRITICAL — cross-context mix: add selectors from 2 different context types
    (e.g. `user.key` + `device.params.os`); verify live worked-example banner updates
  - CRITICAL — key+param mix within same context:
    (e.g. `user.key` + `user.params.tier`); verify bucket changes when tier changes
  - Drag-reorder selectors; verify worked-example updates order
  - Keyboard reorder (arrow keys if implemented)
  - Default-rule distribution: enable on a flag; add same cross-context hash-input spec
  - Save rule; open evaluate-preview with matching multi-context bundle; verify variant + bucket

- [ ] Task 4: Evaluate-preview test panel
  - Open test panel on flag with a percentage-rollout rule
  - Provide a matching context → verify correct variant + full rule trace
  - Provide a non-matching context → verify fallthrough to default rule
  - Provide context with missing required fields → verify OR/AND missing-context error message
  - Multi-context input: provide user + device context bundle
  - CRITICAL: use cross-context key+param combination (e.g. `user.key` + `device.params.os`);
    verify evaluated bucket is stable across multiple submissions of same context
  - Rollout debug: verify bucket number and threshold are displayed
  - Compare result with same evaluation run via Rust SDK (record for Task 1 in Phase 5)

- [ ] Task 5: Segment CRUD — rule-based + list-based
  - Rule-based segment: create with conditions (reuses same operator set as flag rules)
  - List-based segment: select context type, add include keys, add exclude keys
  - List-based: add 10+ keys; verify display/scroll; delete individual keys
  - Segment soft-delete (archive); verify deleted segment removed from flag rule segment picker
  - Restore segment; verify it reappears in picker
  - Segment list pagination; verify `?page=N`

- [ ] Task: Conductor - User Manual Verification 'Bug Discovery — Flags + Segments' (Protocol in workflow.md)

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
