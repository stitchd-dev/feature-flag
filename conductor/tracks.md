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

---

## [ ] Track: Sequential Testing (Always-Valid Inference)
*Link: [./conductor/tracks/seqtest_20260603/](./conductor/tracks/seqtest_20260603/)*

Add always-valid inference — mSPRT always-valid p-values + mSPRT-dual confidence sequences over a single normal-mixture core — so experiments can be peeked safely (continuous dashboard looks) without inflating false positives. All four metric families (conversion/count, continuous, ratio via delta-method, funnel); opt-in per experiment with advanced knobs (α, τ², min-sample-before-first-look); looks ride the existing 60-min tick with a persisted running-minimum p-value; "safe to stop" advisory surfaced in the Results tab. Natural extension of the mature Frequentist + Bayesian engine. Priority: 🟡 Medium. Execution: parallel (worker-wave). 6 phases.

---

