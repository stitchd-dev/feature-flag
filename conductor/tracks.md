# Project Tracks

This file tracks all major tracks for the project.

---

<!-- Archived:
- schema_cutover_20260525 (Schema Hard Cutover — V1 Baseline + Legacy Cleanup) — merged 2026-05-25
- scaffold_20260411 (Workspace Scaffold & Project Foundation)
- domain_20260411 (Core Domain Model & Database Schema)
- rule_engine_20260412 (Rule Engine)
- segmentation_20260412 (Segmentation)
- fix_errors_20260412 (fix the errors across workspaces)
- feature_flags_20260416 (Implement the core Feature Flags module)
- fix_lint_errors_20260417 (fix the lint errors.)
- sdk_20260417 (Rust Server-Side SDK)
- fix_ci_clippy_20260417 (Fix CI Clippy Failure — telemetry.rs map_unwrap_or)
- coverage_20260417 (Increase Test Coverage to >90% Across All Crates)
- mdbook_docs_20260418 (mdBook Documentation Site (API Docs + SDK Guide))
- events_20260419 (Events Layer)
- experimentation_20260419 (Experimentation Module — Experiment CRUD)
- stats_20260420 (Experiment Statistical Analysis)
- auth_20260421 (JWT / Multi-Mechanism Human Auth)
- microservices_20260421 (Microservice Architecture Decomposition)
- docs_microservices_20260422 (mdBook Docs — Microservice Architecture Update)
- org_oidc_saml_20260422 (Org-Level OIDC & SAML with On-the-Fly Provider Instantiation)
- scheduled_stats_20260423 (Scheduled Stats Processing Microservice)
- admin_ui_20260427 (Stitchd Admin UI — Standalone Vite + React admin console)
- admin_ui_multitenant_20260428 (Admin UI — Multi-Tenant Routing & API Integration)
- env_sdk_rbac_20260429 (Environments & SDK Keys — Full-Stack Functional UI with RBAC)
-->

<!-- Archived (continued):
- flags_crud_20260512 (Feature Flags Full CRUD + Rule Builder)
-->

<!-- Archived (continued):
- segments_ui_20260513 (Segments UI — full CRUD admin UI for environment-scoped segments, plus Match Segment rule type in flag rule builder)
- flag_eval_preview_20260514 (Flag Evaluation Preview — evaluate-preview endpoint with rule traces, rollout debug, OR/And missing-context fix)
- context_intel_20260515 (Context Intelligence & Evaluation Telemetry — ClickHouse eval log, context registry, analytics tab, autocomplete, explorer)
- db_optim_20260516 (Database & Query Optimizations — PostgreSQL indexes, N+1 elimination, SDK key cache, ClickHouse MV overhaul, partition tuning, offset pagination)
- sdk_rewrite_20260516 (Clean SDK Implementation — sdks/ Foundation + Rust Server-Side SDK)
-->


<!-- Archived (continued):
- segment_scylla_20260516 (List-Based Segment Storage on ScyllaDB)
-->


<!-- Archived (continued):
- gateway_lean_20260518 (Gateway Lean Refactor — strip DB connections, new analytics-service, retire event-service)
- boundaries_20260518 (Boundary Hardening Refactor — DDD service boundaries, URL canonical rewrite, STITCHD_ env vars, admin UI primitives + Formik migration, SDK spec alignment)
-->


<!-- Archived (continued):
- events_metrics_20260519 (Events + Metrics composable primitives — admin UI CRUD + tester, REST batch ingestion + multi-context flat-map shape, Rust SDK track() + buffer, composable metric_definitions (aggregation/ratio/funnel) with property-based filters + JsonLogic where_clause, ClickHouse-backed metric preview, experiment cutover to metric_ids)
-->


<!-- Archived (continued):
- rust_deps_upgrade_20260520 (Upgrade Rust MSRV to 1.95 + bring every workspace dep to latest cross-compatible major.minor — tonic 0.14 with tonic-prost split, clickhouse 0.15 async insert, openidconnect 4 endpoint type-state migration, rand 0.10, reqwest 0.13, OTel 0.32, sha2 0.11, plus inline let-chain + Duration::from_mins fixes for Rust 1.95 clippy)
-->

<!-- Archived (continued):
- experimentation_full_20260521 (Experimentation — Complete UI + Backend with eval-log-based first-exposure attribution (rule-scoped, context-type-scoped); whole-flag lock while running; default-rule percentage distribution; Frequentist/Bayesian/CUPED/SRM/Guardrails math; per-context-type result views)
-->


<!-- Archived (continued):
- docs_refresh_20260522 (Docs Refresh & Autogeneration Pipeline — bulk-replace 22 stale mdBook narrative pages against current code via 5 parallel topic workers; extend cargo xtask docs with env-vars scraper + cargo-rdme + custom in-xtask link-checker; delete orphan docs/src/internal/* subtree + api/rest.md; wire idempotency CI gate. Surfaced 4 production-affecting findings for follow-up: CH migrations not auto-run on service boot, CH dictionary hard-codes host.docker.internal, OpenAPI ghost event-ingest routes, dead 9080 metrics port mapping.)
-->


<!-- Archived (continued):
- flag_eval_unify_20260522 (Unify Feature-Flag Variant Evaluation — single pure stitchd-core entry point for flag-service preview + Rust SDK; canonical HashInputSpec end-to-end Admin UI → REST → proto → PG → core → SDK; multi-context SDK input enabling cross-context percentage hashing; closes SDK default_rule_distribution gap. Includes post-merge wisp fixing utp/yrj/7yc backend eval, jte service-boot retry, and 8 admin UI bugs (tcy/zh9/042/42f/hw9/e7v/xf2/2zv) with 4 regression tests.)
-->

<!-- ARCHIVED: integration_bugfix_20260524 (2026-05-24) — Integration Bug Hunt & Fix: 34 bugs found and fixed across gateway, db, experimentation-service, admin UI, analytics, and SDK. Full run of all functional use cases against a live stack. See conductor/archive/integration_bugfix_20260524/ -->

<!-- ARCHIVED: domain_boundaries_20260530 (2026-05-31) — Domain-Boundary Refactor: Lean Gateway, Dedup, Dead-Code Audit. Audit-first (5 categories: 14 gateway leaks, 5 boundary violations, 8 dup groups, 16 consistency issues, 10 dead-code items). Phase 2 moved 11/14 leaks into owning services (backward-compatible proto: enabled_override, REPLACE_VARIANTS/RULES, mark_test); Phase 3 dedup+canonical conventions→patterns.md; Phase 4 removed ~1,575 dead lines; PROP-001 dropped feature_flag_rules.frozen. Follow-ups: f70/GL-06/GL-07 + admin UI realignment. Merged to main 7be900e. CI green (tests 1988/0, clippy 0, admin 735/735, docs idempotent). See conductor/archive/domain_boundaries_20260530/ -->

<!-- ARCHIVED: xexp_interaction_20260602 (2026-06-02) — Cross-Experiment Interaction: Exclusion Groups + Interaction Analysis. (1) Mutual-exclusion groups (layers): per-env exclusion_groups with a pinned diversion unit_context_type + immutable salt; disjoint bucket-range allocation per member; in-memory rule-resident ExclusionGate on the rule's percentage allocation so evaluate_flag gates enrollment with ZERO DB lookup and stays experiment-unaware; experimentation-service stamps/clears the gate (atomic FOR UPDATE + audit), rejects default-rule-bound experiments, locks membership while running. (2) Pairwise two-way interaction analysis: self-join experiment_assignments (ITT-bounded to joint exposure), binary (2x2 Wald / RxC Pearson chi-square) + continuous (two-way ANOVA F), Benjamini-Hochberg FDR correction, insufficient_data flagged; experiment_interactions CH table → GetExperimentInteractions RPC → REST → Admin UI Interactions tab + Results warning banner. 9 phases (worker-wave) + 2 review rounds. Merged to main bdf1cef. CI green (tests 2149/0, clippy clean, admin vitest 799, sqlx check, docs idempotent, contract covered). Follow-up: feature-flag-uga (on-demand interaction recompute). See conductor/archive/xexp_interaction_20260602/ -->

<!-- ARCHIVED: nway_interaction_20260603 (2026-06-03) — N-Way (3-way) Cross-Experiment Interaction + Funnel/Ratio Metrics + Bayesian Modeling. Generalized pairwise→3-way (order capped at 3) on a unified experiment_interactions schema (experiment_ids Array(UUID) + interaction_order + term, ReplacingMergeTree/FINAL); full hierarchical decomposition (main + 2-way + 3-way) over aggregation/funnel/ratio metrics; Frequentist (log-linear / multi-factor ANOVA / ratio delta-method) + Bayesian (Beta-Binomial / Normal-Normal) per term; one Benjamini-Hochberg pass over the interaction family (main effects excluded). Built via worker-waves (5 stats-core + 2 sweep/transport + 1 UI). Max-effort code review surfaced 15 findings — ALL fixed (dedup-key collision, ratio cartesian fan-out, funnel JOIN-ON, dropped where_clause, FDR/banner main-effect pollution, anova error term, …) + 2 ef5 follow-ups (aggregation query consolidation 4→1 fetches w/ equivalence proof; Bayesian contrast-driver unification, behavior-preserving). Merged to main e1d394c. CI green (workspace 2275/0, clippy --features test-util clean, admin vitest 825, sqlx-check, docs idempotent, contract 23/23, 9 live-CH integration). Follow-up: feature-flag-uga (on-demand interaction recompute). See conductor/archive/nway_interaction_20260603/ -->

<!-- ARCHIVED: seqtest_20260603 (2026-06-04) — Sequential Testing (Always-Valid Inference) + LIVE per-metric stats compute pass + 3 review rounds. (1) Sequential testing: mSPRT always-valid p-values + mSPRT-dual confidence sequences over one normal-mixture core (stitchd-core::…::stats::sequential), all four metric families; opt-in per experiment (α/τ²/min-sample, snapshotted onto iterations); looks ride the 60-min tick with a persisted running-minimum p; per-variant sequential_result JSON blob → VariantResult → REST → Admin UI Sequential view + "safe to stop" advisory badge. 6 phases (worker-wave, 8 sub-agents). (2) Wired the deferred scheduled per-metric compute (was a scaffold write_results(exp,&[]) with no service calling any stitchd-core stats fn): compute.rs run_stats_compute + queries/variant_stats.rs run ITT sufficient-stats ClickHouse queries → frequentist(+Bonferroni)/bayesian/sequential/SRM/CUPED/recommendation → non-empty experiment_results; CUPED (single per-unit query, honors on_field), percentile bootstrap significance + empirical-quantile point, weighted-allocation SRM (expected split from configured rule/default-rule distribution). (3) Three max-effort review rounds: fixed funnel-cells bind-order (all funnel queries errored), phantom variant_stats[cuped/srm] leak, analyze_numeric n<2 false-significance, Uniq on_field SQL injection + on_field/var charset validation, per-experiment enrich-RPC elimination (settings ride ListRunningExperiments), stats-math de-dup (one erf/norm_cdf/Z95/ratio_delta_var, pinned). Merged to main 74a2810 (local, not pushed). CI green (workspace 2424/0, clippy --all-targets -Dwarnings, sqlx-check, fmt, docs idempotent, OpenAPI contract, live-CH). Follow-up: feature-flag-uga (interaction on-demand recompute). See conductor/archive/seqtest_20260603/ -->

<!-- ARCHIVED: flag_lifecycle_20260604 (2026-06-05) — Flag Lifecycle Automation: Scheduled Changes + Prerequisites + Dependency Integrity. New stitchd-schedule-service (8th gRPC svc): one-shot + recurring/DST-aware (rrule + chrono-tz) scheduled changes for flags/segments/experiments, restart-safe FOR UPDATE SKIP LOCKED claim, experiment-lock-aware skip. Flag prerequisites: eval-time gate in core evaluate_flag with author-configurable fallback variant, transitive + cycle-detected, carried on the definition snapshot so preview + Rust SDK gate identically (transitive-fallback fold fixed in core). Cross-entity dependency integrity: write-time cycle reject (400 + path), delete/archive blocked while referenced (409 dependency_exists) across flags/segments/experiments, dependency-graph read API. Experiment start-prerequisites (flag-in-variant EXACT-verify via new Variant.id / experiment-done) enforced on manual + scheduled start. Segment list-generation activation RPC. Full Admin UI (schedule builder, prerequisites editor w/ live cycle warning, dependency-graph viz, delete-blocked UX, badges, preview-trace surfacing). 10 phases (worker-wave) + revision #1 (Phase 10 follow-up completions). Merged to main e646405 (local, not pushed). CI green (workspace tests 2607/0, clippy -Dwarnings, fmt, sqlx-check, docs idempotent, OpenAPI contract 116 routes, admin vitest 925/925). Beads epic feature-flag-hp5 (all 10 milestones closed). See conductor/archive/flag_lifecycle_20260604/ -->

<!-- ARCHIVED: bandit_20260608 (2026-06-08) — Multi-Armed Bandit (Adaptive & Autonomous Experiment Allocation). Bandit mode on the experiment entity: Thompson Sampling / epsilon-greedy / UCB / contextual (LinUCB, hand-rolled ridge — no new deps) over all four reward families; scalar + multi-objective (scalarization + constrained-guardrail) reward. Two propagation paths: static-rewrite (eval untouched — privileged system-actor lock-bypass write) AND real-time snapshot-resident (evaluate_flag bandit-aware but PURE + zero-DB + deterministic, gated to realtime rules; model rides the polling snapshot — enforced by the evaluation/purity.rs grep-test; static path byte-identical, preview==SDK parity). Autonomous lifecycle (advisory / auto_commit / auto_rollout, idempotent) + optimization campaigns (spawn-on-convergence/drift, atomic try_claim_spawn cap). Bandit-aware SRM + cross-experiment interaction generalized to operator-bounded order 4+ (N-way IPF / inclusion-exclusion ANOVA / 2^k contrasts; env STITCHD_STATS_MAX_INTERACTION_ORDER). Full gateway REST surfacing + Admin UI (config form, allocation-over-time chart, per-objective posteriors, convergence/commit badge, campaign timeline). 2 migrations (bandit_foundation, bandit_lifecycle); 11 new stitchd-core::experimentation::bandit modules. 13 phases (worker-wave, ~16 sub-agents). Merged to main 4319e31 (local, not pushed). CI green vs fresh-from-scratch DB: workspace tests (110 ok-blocks/0 fail), clippy -Dwarnings, fmt, 13 live-CH ignored (8 existing + bandit_reallocation/contextual/multiobjective/interaction_order4/e2e_lifecycle), sqlx-check (--all-targets --features test-util), OpenAPI contract 23/120, admin 994 vitest + tsc + lint + build, docs idempotent. Beads epic feature-flag-2an (all 23 issues closed). Follow-ups: feature-flag-uga (interaction on-demand recompute), feature-flag-7rp (dev Postgres migration drift). See conductor/archive/bandit_20260608/ -->


---

<!-- ARCHIVED: platform_hardening_20260608 (2026-06-09) — Platform Hardening: Idempotency Keys + On-Demand Interaction Recompute + Cursor/Keyset Pagination + Fresh-DB Tooling. (1) Idempotency-Key middleware on ALL gateway mutations (PG idempotency_keys ledger, replay stored 2xx w/ Idempotent-Replayed header, 422 on key-reuse, 409 in-flight, fail-open, 24h TTL sweeper; gateway gains a narrowly-scoped PgPool — its first DB access) + SDK exactly-once (stamps Idempotency-Key on both event POST paths). (2) On-demand interaction recompute wired into TriggerRecompute (closed feature-flag-uga). (3) Cursor pagination: contract via opaque encoded-offset (Rev #1) then TRUE keyset across 8 top-level list entities — flags/experiments/segments/events/metrics/sdk-keys/org-users/exclusion-groups — clean proto cutover (page/per_page/total → cursor/limit/next_cursor), repos OFFSET→keyset (created_at,id; org-users on email,id to preserve order), stitchd_db::KeysetCursor opaque token, Admin UI unchanged (cursor opaque), proven live via grpcurl (closed feature-flag-cj5). Detail sub-lists (iterations/exposures) stay page-based. (4) scripts/reset_dev_db.sh + cargo xtask ch-migrate (closed feature-flag-7rp). Merged to main 57031fb (local). CI-green vs fresh DB: fmt, workspace clippy -D warnings, sqlx-check, 1386 workspace tests, docs idempotent, OpenAPI contract 23/120, admin tsc+lint+vitest 994+build, live-CH stats. Beads epic feature-flag-0aq (all closed). Follow-up filed: pre-existing evaluation_id CH schema drift (event-writer baseline lacks the column EvalLogRow writes). See conductor/archive/platform_hardening_20260608/ -->


---

<!-- ARCHIVED: clean_cutover_20260609 (2026-06-09) — Clean Cutover to Final State (system not live; no migration path / no backward compatibility). (1) Single fresh dated V1 baselines per store: collapsed 10 PostgreSQL + 3 ClickHouse migrations into one 20260609000001_v1_baseline.sql each (PG built from pg_dump + VERIFIED functionally identical via round-trip pg_dump diff = zero diff; sqlx cache valid unchanged). (2) Single canonical ClickHouse `events` table (events_v2 long-retired) + `evaluation_id` folded into flag_evaluation_log; event_writer MIGRATIONS registry → 1 entry. (3) Proto/API compat-shim removal + TAG COMPACTION: removed flags context_hash_specs/ContextHashSpec (hash_inputs sole input), analytics TrackEvent/EventFiring reserved tags, segments AdminSegment always-empty user_list/excluded_keys; updated all consumers (gateway/services/SDK) + contract tests; SDK conformance hashes preserved via canonical-order hash_inputs rebuild. (4) Dead-code removal: deleted pre-cutover crates/stitchd-db/clickhouse-migrations/ dir + unused legacy MfaChallengeRepository; pruned orphaned .sqlx entry. (5) Docs synced (product.md/tech-stack.md; pg_partman→pgcrypto fix). Built autonomously, 5 sequential phases / 21 tasks / 12 commits. Merged to main 6883e37 (local, not pushed). Gate GREEN vs fresh-from-scratch DBs: fmt, clippy -D warnings, cargo test --workspace 2958/0 (39 ignored live-CH validated separately), sqlx-check, xtask docs idempotent, admin tsc/lint/vitest 994. Beads epic feature-flag-3xq (all closed). REST/admin contract byte-unchanged (gateway emits user_list/excluded_keys empties from its own DTO). See conductor/archive/clean_cutover_20260609/ -->

---

<!-- ARCHIVED: members_roles_20260610 (2026-06-10) — Members & Roles page real data (Conductor Wisp Area 1; backend was ahead of UI). Replaced the 100%-mock admin Members page (/org/:orgId/members) with live integration: Members tab lists real org users via the org-scoped management API (/v1/management/orgs/{org_id}/users — NOT superadmin), deterministic avatar initials, role badge, joined date, loading/empty/error states; Add member (modal) + remove member (confirm) gated to org_admin; labelled "Add member" (CreateUser provisions a credentialed account, not an email invite). SSO providers tab: real OIDC/SAML CRUD + SAML SP-metadata download — fixed the STALE/unwired auth-provider client (gateway returns id/enabled + {xml} envelope, not auth_provider_id/is_enabled/string; modelled create/update as provider_type-tagged discriminated unions). Roles tab now honestly documents the fixed org_admin/org_member model; removed the fake custom-role card, "Pending invites" tab, "Bulk invite", and all "Coming soon" placeholders; decommissioned MEMBERS mock + migrated Members out of stubs.tsx. Backend audit: no role-change/invite/custom-role/bulk-invite/MFA API exists (filed as follow-up candidates). Built autonomously, 5 phases, TDD. Merged to main (local, not pushed). Gate GREEN: admin tsc -b clean, eslint 0 errors (2 set-state-in-effect warnings matching Environments.tsx), vitest 1028/1028 (34 new), vite build. Beads epic feature-flag-63g (all milestones + area feature-flag-kui closed). Live E2E (needs full backend stack) noted as manual step in learnings. See conductor/archive/members_roles_20260610/ -->

---

<!-- ARCHIVED: experiment_lifecycle_ui_20260610 (2026-06-10) — Experiment lifecycle UI (Conductor Wisp Area 3; backend ahead of UI). The gateway exposed POST .../transitions (TransitionExperiment) but the experiment detail page never called it — the header had a DEAD "Stop" button + a "Ship winner" button (no onClick), and the only status change was via the Schedule tab. Wired real transitions: new transitionExperiment() client + LifecycleActions control showing only valid transitions per status (draft→Start, running→Pause/Conclude, paused→Resume/Conclude, concluded=terminal), confirm dialog, org_admin-gated, refreshes via setApiExp(updated). Replaced the heavily-fabricated Config tab (hardcoded 50/50 allocation, 380,000 min-sample, MDE 2%, α/β, CUPED 18%, beta-customers targeting) with rows sourced entirely from ExperimentSummary (flag/status/model/metrics/variants/unit-context/exclusion-group/timestamps); replaced the mock Lifecycle card (fake actors Marco G./Priya R.) with lifecycleTimeline() derived from real created/started/ended timestamps. De-mocked the Metrics tab (dropped all-dash Type/Aggregation/Threshold columns) and the Events tab (aspirational "live tail" → honest pointer to Events/Metrics pages). UI-only, no backend change. Built autonomously, 4 phases, TDD. Merged to main (local, not pushed). Gate GREEN: admin tsc -b clean, eslint 0 errors, vitest 1049/1049 (21 new), vite build. Beads epic feature-flag-6qh (all milestones + area feature-flag-dx6 closed). Follow-ups (no backend source): allocation/targeting/MDE/α-β/CUPED config + lifecycle audit history. See conductor/archive/experiment_lifecycle_ui_20260610/ -->

---

<!-- ARCHIVED: flags_list_honest_20260610 (2026-06-10) — Honest flags list (Conductor Wisp Area 5). Area 5 was scoped as "UpdateFlagHashing has no UI / flags-list sparklines placeholder / PreviewMetric unused"; on inspection PreviewMetric was ALREADY wired (EditMetricModal → POST /v1/metrics/{id}/preview) and a hash-inputs UI ALREADY existed (HashInputSelectorList + hashInputSchema in the default-rule editor). The genuine remaining gap was the flags TABLE rendering three fabricated columns with no list-level backing: 30d evals (<Sparkline data={[]}/> + —), Segments (—), Owner (—). Removed all three (header + FlagTableRow) and the now-unused Sparkline import; card/grouped layouts already showed only real data. Real per-flag analytics remain on the detail Analytics tab (/eval-stats). UI-only, no backend change. 1 phase, TDD (source-contract test). Merged to main (local, not pushed). Gate GREEN: tsc -b clean, eslint 0 errors, vitest 1053/1053 (4 new), vite build. Beads epic feature-flag-rjb (+ area feature-flag-u42 closed). Follow-up filed feature-flag-b78: list-level 30d-eval summary needs a batch/summary eval-stats endpoint. See conductor/archive/flags_list_honest_20260610/ -->

---

<!-- ARCHIVED: bandit_campaign_ui_20260610 (2026-06-10) — Bandit campaign management (Conductor Wisp Area 4; gateway behind the experimentation-service). The experimentation-service had all 4 bandit-campaign RPCs (Create/Get/List/Stop) but the GATEWAY only exposed the two GETs, the admin client was read-only, and NO campaign UI existed (listBanditCampaigns was unused). Closed end-to-end: (1) Gateway — added POST /v1/environments/{env}/bandit-campaigns (create_bandit_campaign; config rides as serde_json::Value → to_string()) + POST .../{id}/stop (stop_bandit_campaign), registered in router.rs + test_router + openapi.rs paths/components (CreateBanditCampaignBody). (2) Admin client — createBanditCampaign/stopBanditCampaign + typed BanditCampaignConfigInput. (3) UI — BanditCampaignsPanel on the Experiments page (env-scoped): lists campaigns, New-campaign modal (flag picker, max_iterations, drift_threshold, variant_discovery, optional budget cap → buildCampaignConfig), Stop (confirm) on non-terminal campaigns; org_admin-gated; loading/empty/error. 4 phases, TDD. Merged to main (local, not pushed). Gate GREEN: gateway cargo test (stub, 3 new + openapi contract; clippy -D warnings + fmt clean) — pre-existing idempotency::pg_store_* lib tests need live DATABASE_URL and fail DB-less (pass in CI, not a regression); admin tsc -b clean, eslint 0 errors, vitest 1069/1069 (16 new), vite build. Beads epic feature-flag-tfq (+ area feature-flag-j38 closed). Follow-ups (no RPC): pause/resume + edit campaign config. See conductor/archive/bandit_campaign_ui_20260610/ -->

---

## [ ] Track: Audit log end-to-end — gateway-edge capture + read + real UI
*Link: [./conductor/tracks/audit_log_20260611/](./conductor/tracks/audit_log_20260611/)*
