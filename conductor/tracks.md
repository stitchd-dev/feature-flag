# Project Tracks

This file tracks all major tracks for the project.

---

## [x] Track: Schema Hard Cutover — V1 Baseline + Legacy Cleanup
*Link: [./conductor/tracks/schema_cutover_20260525/](./conductor/tracks/schema_cutover_20260525/)*

<!-- Archived:
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
