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

- [x] Task 1: Events full lifecycle
  - BUG-021 (CRITICAL): ALL event definition endpoints are stubs — cannot register events; entire TestEventWidget, EventDetail, archive flows all blocked
  - Event list: empty (stub); POST returns 202 drops body; GET by ID returns 501; pagination N/A (blocked)

- [x] Task 2: Metrics full lifecycle
  - Created aggregation (click_count), ratio (conversion_rate), funnel (checkout_funnel) metrics ✓
  - BUG-022 (HIGH): `events_v2` ClickHouse migration missing — table never created; FIXED (applied manually)
  - BUG-023 (HIGH): metric preview fails — `events_v2` table-not-found; after fix: empty sparkline (expected) ✓
  - Metric list pagination: ✓ (tested in env scope)
  - Bidirectional back-links: blocked by BUG-021 (event endpoints are stubs, so events can't reference metrics)
  - `where_clause`, `on_field` tested: accepted in create body ✓

- [x] Task 3: Experiment creation + binding
  - BUG-024 (HIGH): context type registry empty — stats-service couldn't populate without ClickHouse tables; FIXED (seeded manually in PostgreSQL)
  - BUG-025 (CRITICAL): Experiment creation always fails — `Experiment` proto missing binding fields (flag_id, flag_rule_id, targets_default_rule, unit_context_types, guardrail_metric_ids, pre_period_days); service uses random placeholder UUIDs → FK violation
  - BUG-026 (MEDIUM): `map_experiment_db_err` misclassifies all DB constraint violations as unique violations
  - Experiment creation: BLOCKED by BUG-025 — cannot create any experiment

- [x] Task 4: Experiment lifecycle + results display
  - BLOCKED: Depends on successful experiment creation (BUG-025). Cannot test without a created experiment.
  - Flag lock, results panel, guardrail display, SRM, CUPED, recompute — all untestable
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

- [x] Task 1: Rust SDK integration test
  - SDK tests pass: `cargo test --features test-util -p stitchd-sdk-rust` (16 tests)
  - Cross-context hash: iOS→55, Android→818, Windows→802 — stable across contexts ✓
  - SDK events:batch: BUG-028 (wrong ClickHouse env vars in launch.json) + BUG-029 (schema mismatch) found and fixed
  - BUG-030 (CRITICAL): flag_evaluation_log captures one context per row — cross-context bundle membership lost; no evaluation_id to link sibling context rows; SDK FlagEvaluationEvent proto only carries one context_type/context_key field

- [x] Task 2: SDK list-segment + key auth tests
  - VIP Users List segment: user-vip-1→true, user-nonmember-99→false, user-vip-5→true ✓
  - Wrong SDK key → 401 `invalid_sdk_key` ✓
  - Revocation: management.rs fetches key hash + calls sdk_key_cache.invalidate() before DB revoke → immediate cache eviction via API ✓; direct DB bypass preserves cache entry until TTL (1 min) — expected

- [x] Task 3: UI/UX polish sweep
  - Empty states: flag/segment/experiment list with data → filter returns blank area (no "no results" message) → BUG-034
  - Form validation: flag create/edit, rule create, segment create — inline required-field errors appear ✓; experiment form validation works ✓
  - Form validation: Preview tab accepts empty context without validation → BUG-032
  - Pagination controls: flags list "Next" disabled on last page ✓; "Prev" disabled on first page ✓
  - Destructive actions: archive flag shows confirmation dialog ✓; delete segment shows confirmation dialog ✓
  - Breadcrumb/navigation: flag list → flag detail → edit rule → back → breadcrumb correct ✓
  - Org switching: only one org in test env; n/a
  - Loading skeletons: data fetch shows skeleton rows during load ✓
  - Dashboard heading and sidebar org label show raw UUID → BUG-031
  - TopbarNav hardcoded avatar/env badge → BUG-007 (already filed)
  - display_name not required; sidebar shows "Org User" for blank name → BUG-033
  - Mobile responsiveness: skipped (user explicitly excluded mobile UI)
  - Note: Error toast test skipped (service kill would disrupt live test stack)

- [x] Task: Conductor - User Manual Verification 'Bug Discovery — SDK Integration + UI/UX Polish' (Protocol in workflow.md)

## Phase 6: Bug Fixes

- [x] Task 1: Fix critical severity bugs (from bugs.md) (435630c)
  - For each critical bug: implement code fix, write targeted test, smoke-check on live stack
  - Run `cargo test -p <affected_crate>` after each backend fix
  - Run `npm run lint` + `node_modules/.bin/tsc --noEmit` after each frontend fix

- [x] Task 2: Fix high severity bugs (from bugs.md) (5e49d00, de5a233, afc4a43)
  - Same fix protocol as Task 1; critical must be complete before starting high

- [x] Task 3: Fix medium severity bugs (from bugs.md) (5e49d00, de5a233, afc4a43)
  - Same fix protocol; targeted test where warranted

- [x] Task 4: Fix low severity bugs (from bugs.md) (4b01e11)
  - Fix and smoke-check; test only where behaviour change is non-trivial

- [x] Task 5: Regenerate sqlx offline cache + full CI gate run (1c6e426)
  - `SQLX_OFFLINE=false cargo sqlx prepare --workspace -- --tests`
  - `cargo fmt --all --check`
  - `cargo clippy --workspace --all-targets -- -D warnings`
  - `cargo test --workspace`
  - `node_modules/.bin/tsc --noEmit -p tsconfig.app.json`
  - `npm run lint`
  - All gates must pass before this phase is considered complete

- [ ] Task: Conductor - User Manual Verification 'Bug Fixes' (Protocol in workflow.md)
