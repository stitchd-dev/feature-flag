# Initial Concept
Stitchd Feature Flag is a self-hosted platform for feature flagging and experimentation.
<!-- Last refreshed: 2026-06-04 (post seqtest_20260603 — sequential testing + live per-metric stats compute pass; xexp/nway cross-experiment interaction. Status rows 76-78 + experimentation detail 130-133 synced during the tracks; verified current.) -->

# Product Guide

## Vision

Stitchd Feature Flag is a Feature Flagging & Experimentation platform focused on 
self-hosted deployment. It targets internal engineering teams, SaaS product teams, 
and data/growth teams who need reliable flag evaluation and statistically rigorous 
A/B experimentation. Admin UI is coming later as a separate project.

## Target Users
- Internal engineering teams (self-hosted deployments)
- SaaS product teams (multi-tenant)
- Data / growth teams running A/B and multivariate experiments

## Deployment Model
- **Current:** Self-hosted (primary focus) — eight-service Docker Compose stack
- **Internal Architecture:** Eight gRPC microservices (`auth`, `flag`, `segmentation`, `analytics`, `experimentation`, `stats`, `schedule`) + REST gateway; `stitchd-server` monolith removed (2026-04-21); `stitchd-event-writer` library handles ClickHouse writes; `stitchd-sdk-rust` library is the server-side Rust SDK
- **Future:** Cloud SaaS offering

## Multi-Tenancy
Each tenant → multiple environments → each environment has SDK keys (min 1 active; 
supports rotation via create/revoke).

## Scoping Model
- **Project level:** Feature Flag definitions, Variant configurations
- **Environment level:** Rules, Segments, Experiments, Events

## Core Context Model
Each evaluation context: `{_type, key, parameters: Map<String, int|double|semver|string|boolean>, privateParameters: List<String>}`
`privateParameters` identifies fields that must be excluded from all logging.

## Data Persistence & Integrity
- **Optimistic Concurrency:** All mutable entities use version-based optimistic locking 
  to prevent lost updates in highly concurrent environments.
- **Audit Logging:** Every mutation (create, update, soft-delete) is automatically 
  recorded in a central audit log, capturing the actor, resource, and specific changes.
- **Soft Deletion:** Business-critical entities use soft-deletion to maintain data 
  relationships and auditability.

## Context Intelligence Layer
A dedicated layer that observes contexts flowing through the system and maintains 
a registry of known context types, their properties, and observed value ranges/enums.
Exposed as an API for the Admin UI (coming later) to power dropdown/autocomplete 
behaviour (e.g. when building segment rules or flag targeting conditions).

## Implementation Status (as of 2026-06-08)

| Module | Status |
|---|---|
| Domain model + DB scaffold | ✅ Complete |
| Segmentation (rule-based + list-based) | ✅ Complete |
| Feature Flags + Rule Engine | ✅ Complete |
| Events + Experimentation (Frequentist/Bayesian) | ✅ Complete |
| Server-side Rust SDK | ✅ Complete |
| Human Auth (JWT, Password, OIDC, SAML, MFA, Invites, Rate Limiting) | ✅ Complete |
| Microservice decomposition (6 services + gateway) | ✅ Complete |
| Admin UI — Superadmin + Org Management | ✅ Complete |
| Admin UI — Environments & SDK Keys RBAC | ✅ Complete |
| Admin UI — Feature Flags Full CRUD + Rule Builder | ✅ Complete |
| Admin UI — Segments Full CRUD (rule-based + list-based) | ✅ Complete |
| Flag Evaluation Preview (rule traces, rollout debug, OR/AND missing-context fix) | ✅ Complete |
| Context Intelligence (eval telemetry, context registry, autocomplete, explorer) | ✅ Complete |
| Database & Query Optimizations (PG indexes, N+1 elimination, SDK key cache, ClickHouse MVs, offset pagination) | ✅ Complete |
| ScyllaDB list-segment storage (generation swap, sweeper, metrics, OTel spans) | ✅ Complete |
| Boundary Hardening Refactor (boundaries_20260518) | ✅ Complete |
| Events Module — full admin UI + SDK ingestion + per-env quota | ✅ Complete |
| Metrics Layer — composable definitions (aggregation/ratio/funnel) + experiment cutover | ✅ Complete |
| **Experimentation as a whole — complete UI + Backend with eval-log-based first-exposure attribution, whole-flag lock, per-context-type stats, default-rule experiments, Frequentist + Bayesian + CUPED + SRM + Guardrails** | ✅ Complete |
| Flag-Evaluation Unification — single `stitchd-core::evaluation::evaluate_flag` orchestrator drives preview + SDK; canonical `hash_inputs` selector list (cross-context Key + Parameter mixing) end-to-end through Admin UI → REST → PG → preview AND snapshot → SDK | ✅ Complete |
| Schema Hard Cutover — collapsed 44 Postgres, 14 ClickHouse, and 5 ScyllaDB migrations into single V1 baselines; retired segment_rules, dual-write hash_inputs, and flag_evaluation_log_v2; migrated rollout percentages to u32 basis points (0.01% precision) | ✅ Complete |
| Domain-Boundary Refactor (domain_boundaries_20260530) — lean gateway (REST↔gRPC translation + cross-cutting only; domain logic moved into owning services), de-duplication, canonical error/pagination conventions, dead-code removal (~1,575 lines), dropped dead `frozen` column. Behavior-preserving; backward-compatible proto additions only | ✅ Complete |
| **Cross-Experiment Interaction (xexp_interaction_20260602)** — (1) **Mutual-exclusion groups** ("layers"): per-env `exclusion_groups` with immutable salt; disjoint bucket-range allocation per member sized by traffic allocation; gate rides on the rule's percentage allocation as in-memory snapshot data (`ExclusionGate`) so `evaluate_flag` gates enrollment with **zero DB lookup** and stays experiment-unaware. (2) **Interaction analysis**: pairwise two-way significance test (binary + continuous) over shared-context populations from `experiment_assignments`, results in ClickHouse `experiment_interactions`, surfaced in the Admin UI Interactions tab + Results warning banner | ✅ Complete |
| **N-Way Interaction (nway_interaction_20260603)** — generalized cross-experiment interaction from pairwise to **3-way** (order capped at 3) on one unified path: `experiment_ids Array(UUID)` + `interaction_order` + `term` schema (ReplacingMergeTree, reader `FINAL`); **full hierarchical decomposition** (main + all 2-way + 3-way terms) over **all four metric families** (aggregation/conversion + funnel via log-linear, continuous via multi-factor ANOVA, ratio via delta method); **both** Frequentist (per-term) **and Bayesian** (Beta-Binomial / Normal-Normal) inference per term; one BH-FDR pass across the sweep family; proto/reader/gateway/Admin-UI surfacing generalized (order/term/Bayesian columns). Built via a 5-worker parallel wave (stats core) + 2-worker wave (sweep + transport) + 1 UI worker | ✅ Complete |
| **Sequential Testing (seqtest_20260603)** — **always-valid inference** so experiments can be peeked any time without α inflation: **mSPRT always-valid p-values** (running-minimum, persisted across ticks) + **mSPRT-dual confidence sequences**, over one normal-mixture core (`stitchd-core::…::stats::sequential`) covering **all four metric families** (count/conversion, continuous, funnel, ratio via delta method). **Opt-in per experiment** (α / τ² / min-sample, snapshotted onto iterations); per-variant `sequential_result` JSON blob in `experiment_results` → `VariantResult` → REST → Admin UI **Sequential** Results view (always-valid p + anytime CI columns) + **"safe to stop"** advisory badge. Math validated by Monte-Carlo (peeking-under-H₀ ≤ α vs naive ~5× inflation). Built via worker-waves (core ∥ schema; compute → read; form ∥ Results). The scheduled per-metric stats pass (`stitchd-stats-service`) now computes live Frequentist + Bayesian + sequential + SRM end-to-end (the previously-deferred compute orchestration; `feature-flag-k1l`/`-2lh`, proven by a live-ClickHouse integration test). CUPED variance reduction (numeric metrics, pooled-θ), percentile-metric bootstrap significance, ratio analyzers in `stitchd-core`, and dedicated SRM surfacing followed (`feature-flag-z7m`/`-r07`/`-nsh`/`-891`). | ✅ Complete |
| **Multi-Armed Bandit (bandit_20260608)** — experiment **mode** (A/B-test ↔ bandit) that reallocates traffic toward better arms instead of holding a fixed split: bandit algorithms (**Thompson sampling**, **ε-greedy**, **UCB**, **contextual** linear/Thompson) in `stitchd-core::experimentation::bandit`; two propagation modes — **Static** (the scheduled `stitchd-stats-service` tick recomputes arm weights from live ClickHouse rewards and writes them onto the bound rule via a privileged lock-bypass flag-service RPC) and **Realtime** (a per-arm posterior **snapshot model** evaluated **DB-free / in-memory** inside `evaluate_flag` — non-bandit eval is byte-for-byte unchanged, enforced by `evaluation::purity`); **autonomous lifecycle** (Advisory → AutoCommit → AutoRollout: detect convergence, persist `bandit_converged_variant`/`_prob`, commit 100% to the winner, then stop the experiment and release the whole-flag lock); **campaigns** (auto-spawn successive bandit iterations); **multi-objective** rewards (scalar / scalarized-weighted / constrained); cross-experiment **interaction order generalized to 4+**. Full REST surfacing (bandit state, allocation history, campaigns) + Admin UI (bandit config form, Results allocation/posterior/convergence view, order-4+ interactions). Proven end-to-end by self-seeding live-ClickHouse integration tests (reallocate → converge → rollout). | ✅ Complete |
| **Flag Lifecycle Automation (flag_lifecycle_20260604)** — (1) **Scheduled changes** to flags/segments/experiments: one-shot + recurring (RRULE + IANA-tz, **DST-aware**) mutations applied by a new gRPC-only `stitchd-schedule-service` whose tokio interval loop claims due rows (`FOR UPDATE SKIP LOCKED`, restart-safe + idempotent, missed-tick catch-up) and dispatches each to the owning service's canonical mutation RPC, honoring the experiment lock + transition validity at fire time. (2) **Flag prerequisites** — an eval-time gate (in `evaluate_flag`, before rule iteration) returning a configured **fallback variant** when an upstream prerequisite flag is unmet (absent / disabled / wrong variant), transitive across chains with write-time + eval-time cycle detection; resolves identically in preview AND the Rust SDK. (3) **Cross-entity dependency integrity** — referential `409 dependency_exists` delete/archive-blocking when a flag/segment/experiment is referenced (flag→flag prereq, flag→segment, segment→segment, experiment→flag, experiment-start prereqs) + a gateway dependency-graph read API. Full Admin UI (schedule builder w/ tz picker + run history, prerequisites editor w/ live cycle warning, dependency graph + delete-blocked UX). | ✅ Complete |

## Modules

### 1. Segmentation
- Rule-Based Segments: rules evaluated against client Contexts
- List-Based Segments: per context-type include/exclude key lists
  - Persistence: **ScyllaDB** — wide-row tables partitioned by `(segment_id, context_type)`;
    atomic generation swap via LWT CAS; orphaned generations cleaned up by background sweeper.
    PostgreSQL retains segment metadata (name, type, counts, audit log) only.

### 2. Feature Flags
- Typed flags: `int | double | bool | string | json`; variants must match flag type
- States: enabled (default rule + custom rules) / disabled
- Output: specific variant OR percentage allocation (0.1% granularity)
  hash(targeted context keys/params, flag key, project id, environment)
- **Unified evaluation orchestrator (post-`flag_eval_unify_20260522`):** every variant assignment — preview path, SDK path, default-rule fallthrough, and default-rule distribution — funnels through the SOLE `stitchd-core::evaluation::evaluate_flag(...)` entry point. It owns rule iteration, percentage allocation, default-rule fallthrough, and trace emission. `evaluate_preview` is a thin wrapper that calls it with `TraceLevel::Full`; the Rust SDK's `evaluate(...)` calls it with the caller-requested trace level. There is no per-caller orchestration loop anywhere else in the codebase — the only legitimate path to a variant assignment goes through this function.
- **Cross-context percentage hashing:** a rule's percentage allocation carries an ordered `hash_inputs: Vec<HashSelector>` list. Each selector is either `ContextKey { context_type }` (hash the `key` field of the named context) or `ContextParameter { context_type, parameter }` (hash a named parameter of the named context). Selectors mix freely across context types — a single rule can hash on `user.key + user.params.tier + device.params.os + application.key`. Authored once in the Admin UI's `HashInputSelectorList`, persisted to PG as a JSONB column, sent over proto and REST in declaration order, and consumed by `evaluate_flag` via the `HashInputSpec` resolver. The hash algorithm and bucket math (Murmur3 → 0–999) remain unchanged.
- **Evaluate-Preview:** `POST /flags/{key}/evaluate-preview` accepts a mock context and returns
  the evaluated variant plus a full rule trace (which rule matched, why), rollout debug info,
  and OR/AND missing-context resolution details — used by the Admin UI "Test" panel

### 3. Events

- **Pre-registered only.** Each event has a unique `event_key` per environment and a `metric_type` classifier — one of:
  - `count` (occurrence marker, no value required),
  - `conversion` (bool),
  - `revenue` / `duration` / `numeric` (numeric value),
  - `custom` (free-form, optional JSON-schema validation on payload).
- **Optional JSON Schema** on the definition validates event payloads at ingestion (e.g. require `currency` ∈ `{USD, EUR, GBP}` for purchase events).
- **Multi-context attribution:** every firing carries a flat `contexts: {type: key, ...}` map so a single event can be attributed to multiple dimensions simultaneously (e.g. `{user: alice, account: acme, session: s99}`) without inflating count metrics. Stored in ClickHouse as `Array(Tuple(String, String))`.
- **Soft-delete (archive)** rather than hard delete; archived events reject new firings with HTTP 410 while ClickHouse history remains queryable.
- Backed by a `verify_track_event` admin-auth path (`POST /v1/admin/events/track`) for SDK debugging from the UI.

### 4. Metrics

- **Composable primitives** — three kinds, persisted in PostgreSQL `metric_definitions`:
  - **Aggregation** — `count / sum / avg / p50 / p90 / p99 / uniq` over one event stream, optionally filtered by a JsonLogic `where_clause` on `properties[...]`. The `on_field` references either the canonical numeric column (`value`) or a property key.
  - **Ratio** — `numerator / denominator` where both are existing aggregation metrics; below `min_denominator` the bucket emits null (insufficient-data semantics).
  - **Funnel** — ordered list of event-key steps with a `window_seconds` conversion deadline; ClickHouse `windowFunnel` evaluates per `(day, dedup_key)` and the final-step rate is reported as the bucket value.
- **Preview pipeline (Phase 4):** `POST /v1/metrics/{id}/preview` runs the kind-specific ClickHouse query against `events_v2` and returns a zero-filled daily time-series (days clamped to [1, 90]; default 7). Sparkline-ready.
- **Bidirectional UI back-link:** EventDetail page lists every metric that references the event (aggregation by `config.event_key` + funnel step matches; ratio metrics surface transitively through the aggregations they wrap).
- **Goal direction** (`increase` / `decrease` / `neutral`) drives experiment winning-variant logic and the up/down arrow shown in the metric list.

### 5. Experimentation
- Experiments reference **metric_ids** (cutover from raw event_key in migration `20260520000002_experiment_metrics_cutover.sql`); the per-iteration `metric_ids` column lives in `experiment_iterations`.
- **Attribution model (post-`experimentation_full_20260521`):** first-exposure intent-to-treat (ITT), derived server-side from `flag_evaluation_log_v2`. SDKs are experiment-unaware — they do NOT tag events with `(experiment, iteration, variant)` tuples. Eval-log rows route through `experiment_assignments_mv` into `experiment_assignments`; stats queries JOIN `events_v2` ⨝ `experiment_assignments` on `(env_id, context_type, context_key)` and filter `e.occurred_at >= a.assigned_at` for strict ITT.
- **Binding model:** an experiment binds to either (a) a percentage-distribution custom rule via `flag_rule_id`, OR (b) the flag's default-rule fallthrough via `targets_default_rule = true` (requires `feature_flags.default_rule_distribution`). XOR-constrained at the PG layer.
- **Whole-flag lock:** while running/paused, every flag/variant/rule mutation (including default-rule-distribution updates) returns HTTP 409 `FLAG_LOCKED_BY_EXPERIMENT` with the experiment ID in the body. Replaces the old per-rule `frozen` flag (whose dead column + write path were removed in `domain_boundaries_20260530`).
- **Per-context-type analysis:** every experiment carries `unit_context_types text[] NOT NULL` (default `{user}`). All stats (Frequentist t-test / two-proportion Z, Bayesian posteriors, CUPED, SRM chi-square, guardrail direction) compute independently per context type and surface in the Admin UI via a context-type tab strip.
- **Models:** Frequentist (Welch's t-test, two-proportion Z, Bonferroni correction for >2 variants) and Bayesian (Beta-Binomial / Normal-Normal posteriors, probability-to-beat-control, expected lift). CUPED variance reduction via per-experiment `pre_period_days`. Guardrail metrics flagged on direction violation.
- **Recompute** is scheduled (60-min via `stitchd-stats-service`) plus event-driven via the `TriggerRecompute` gRPC RPC; on-demand from the Admin UI via `POST /v1/.../experiments/{id}/recompute`.
- **Mutual-exclusion groups (layers):** experiments may join a per-environment `exclusion_group`. Each group pins a single **diversion (randomization) `unit_context_type`** (e.g. `user`), set at creation; every member must randomize on that unit, enforced at assignment (a unit mismatch is rejected) — this is what guarantees a context maps to ONE shared bucket across the group's flags. Members are allocated disjoint sub-ranges of a deterministic `[0,10000)` bp bucket space (sized by traffic allocation), so a given context is enrolled in at most one member experiment — preventing interaction by construction. The gate (`group_salt` + group `unit_context_type` + allocated range) rides on the bound rule's percentage allocation as static snapshot data; `evaluate_flag` hashes `group_bucket(unit_key, salt)` in-memory and holds out off-range/missing-unit contexts with **no DB lookup**, staying experiment-unaware. The experimentation-service stamps/clears the gate on assign/unassign/stop and frees the range; group membership is locked while running/paused.
- **Cross-experiment interaction analysis (N-way, post-`nway_interaction_20260603`):** for experiments that *do* overlap (different groups / ungrouped), `stitchd-stats-service` enumerates candidate **pairs AND triples** (`candidate_pairs`/`candidate_triples`; a triple needs all three pairs valid + a common metric — pairwise window-overlap implies a common live window by Helly's theorem), self-joins `experiment_assignments` on `(env_id, context_type, context_key)` across the k experiments, and aggregates each shared metric per **variant-tuple cell** bounded to first-exposure ITT (`occurred_at ≥ greatest(a…k.assigned_at)`). Each candidate tuple emits a **full hierarchical decomposition** — one row per main effect, every pairwise interaction, and (order 3) the three-way term — over **all four metric families**: aggregation/conversion + funnel (`windowFunnel`) via a log-linear contingency decomposition, continuous via multi-factor ANOVA, and ratio via the delta method. Every term carries **both** a Frequentist result (estimate/p_value/df/significant) and a **Bayesian posterior** (Beta-Binomial for binary/funnel, Normal-Normal for continuous/ratio → `prob`/`expected`/credible interval). Significance is decided after one **Benjamini–Hochberg (FDR 0.05) correction** across the whole sweep's Frequentist family (Bayesian is independent of FDR); under-powered terms are flagged `insufficient_data` (0.0 sentinels, never shown significant). The unified `experiment_interactions` table keys on `experiment_ids Array(UUID)` + `interaction_order` + `term` (ReplacingMergeTree, reader uses `FINAL`); results surface via `GetExperimentInteractions` → REST → the Admin UI Interactions tab (order/term/Frequentist + Bayesian columns) + a Results-tab warning banner (fires on a significant OR high-probability interaction of any order). Interaction order generalizes to **4+** (post-`bandit_20260608`): the order-≥4 path uses the inclusion-exclusion SS partition for ANOVA, the `2^k`-corner DiD hypercube contrast for ratio, and `k_way` IPF (no-top-way log-linear) for aggregation/funnel; the operator-set cap is `STITCHD_STATS_MAX_INTERACTION_ORDER` (default 3) and cap truncation is logged.
- **Sequential testing (always-valid inference, post-`seqtest_20260603`):** opt-in per experiment (`sequential_testing_enabled` + `sequential_alpha` / `sequential_tau_squared` / `sequential_min_sample_size`, snapshotted onto iterations). For each treatment-vs-control comparison the stats pass computes an **mSPRT always-valid p-value** (the running minimum of `1/Λ_t` under a N(0,τ²) mixing prior, persisted across 60-min ticks so it is monotone non-increasing — valid under any peeking schedule) and the **mSPRT-dual confidence sequence** (anytime-valid CI), over all four metric families (count/conversion, continuous, funnel, ratio via delta method) via `stitchd-core::experimentation::stats::sequential`. Results persist as a per-variant `sequential_result` JSON blob in `experiment_results`, surface through `VariantResult` → REST → the Admin UI **Sequential** Results view (always-valid p + anytime-CI columns) with a **"safe to stop"** advisory badge (fires when the boundary is crossed in the metric's goal direction; advisory only — no auto-stop). Computed by the same scheduled per-metric stats pass as Frequentist/Bayesian.
- **Multi-Armed Bandit (post-`bandit_20260608`):** an experiment **mode** (`experiment_mode = 'ab_test' | 'bandit'`) that reallocates traffic toward better-performing arms instead of holding a fixed split.
  - **Algorithms** (`stitchd-core::experimentation::bandit`): Thompson sampling (Beta / Normal posteriors), ε-greedy, UCB1, and **contextual** bandits (per-context-feature linear / Thompson) — all selected via the experiment's `bandit_config`.
  - **Two propagation modes.** **Static** — the scheduled `stitchd-stats-service` tick reads live ClickHouse rewards, recomputes arm weights (honoring a `min_exploration_bp` exploration floor so no arm is starved), and writes the new distribution onto the bound rule via a **privileged lock-bypass** flag-service write (the whole-flag lock blocks human edits but not the autonomous reallocation). **Realtime** — a compact per-arm posterior **snapshot model** is published onto the rule and evaluated **DB-free, in-memory** inside `evaluate_flag`; non-bandit evaluation is **byte-for-byte identical** to the static path (guaranteed by a golden-bits test and enforced structurally by `stitchd-core::evaluation::purity`, which keeps `evaluate_flag` free of any I/O).
  - **Autonomous lifecycle** (`stitchd-stats-service::lifecycle`, opt-in per experiment): **Advisory** records the converged winner (`bandit_converged_variant` / `bandit_converged_prob`) for a "ready to commit" badge with no traffic change; **AutoCommit** additionally commits 100% to the winner (flag stays running); **AutoRollout** commits then **stops** the experiment — which releases the whole-flag lock. Convergence is detected over the objective posteriors at a configurable probability threshold; the commit→stop sequence is idempotent/restart-safe. Every autonomous action records a `bandit_allocation_runs` history row (`reallocate` / `commit` / `rollout`).
  - **Campaigns** chain successive bandit iterations autonomously (`stitchd-stats-service::campaign`), spawning the next iteration when the current converges.
  - **Multi-objective rewards:** scalar (single metric), scalarized (weighted blend of metrics), or constrained (optimize one metric subject to guardrail constraints) — `RewardObjective`.
  - **Surfacing:** REST `GET …/experiments/{id}/bandit` (state: current allocation, posteriors, convergence, committed flag), `…/bandit/history` (allocation timeline), and `…/bandit-campaigns`; Admin UI bandit config in the create/edit form + a **Bandit** Results view (allocation-over-time chart, posteriors, convergence badge).
- Future: warehouse-backed event ingestion; on-demand interaction recompute (currently 60-min tick only — tracked as `feature-flag-uga`).

### 6. Rule Engine
- Core: ordered rule list (first true = exit); AND combinator; per-rule NOT
- Segmentation rules: inherit core
- Feature flag rules: inherit core + "Is in Segment" + "Flag evaluated with variant X"

### 7. Flag Lifecycle Automation (flag_lifecycle_20260604)

- **Scheduled changes (`stitchd-schedule-service`):** a new gRPC-only service (mirroring `stitchd-stats-service`) that owns scheduled-change lifecycle. A tokio interval loop (`STITCHD_SCHEDULE_SCHEDULER_INTERVAL_SECS`, default 60) claims due rows from PostgreSQL `scheduled_changes` (`WHERE status IN ('pending','active') AND next_run_at <= now() … FOR UPDATE SKIP LOCKED`, batched by `STITCHD_SCHEDULE_CLAIM_BATCH`), applies each inside the same claim transaction, appends a `scheduled_change_runs` history row, then advances (recurring: recompute `next_run_at` via RRULE) or finalizes (one-shot: terminal status). Restart-safe + idempotent: a crash mid-apply rolls back and is re-claimed next tick; a concurrent replica `SKIP LOCKED`s past in-flight rows. **One-shot** changes fire once at `scheduled_at`; **recurring** changes carry an RFC-5545 RRULE + an IANA timezone (the `DTSTART;TZID=…` zone is authoritative for the math) and are **DST-aware** (a weekday-09:00 local rule shifts its UTC fire hour across spring-forward). Each entity kind dispatches to its owner's canonical RPC: flag → flag-service `MutateFlag`, experiment → experimentation-service `TransitionExperiment` (start/pause/resume/stop/archive, validated at fire time), segment → segmentation-service `UpdateAdminSegment`. The scheduler honors the **whole-flag experiment lock** (locked flag → run recorded `skipped`, never errors; recurring still advances) and is attributed as the system/scheduler actor by the owning service's existing audit + version-bump path.
- **Flag prerequisites (eval-time gate w/ fallback):** a flag may declare `prerequisites` — upstream `(prerequisite_flag, required_variant)` pairs plus a `fallback_variant`. The gate slots into `stitchd-core::evaluation::evaluate_flag` after the disabled-flag check and **before** rule iteration: if any prerequisite resolves to absent / disabled / a non-required variant, the flag short-circuits to its fallback variant (or the off/disabled default) and the trace **names the failing prerequisite** (identifiers only — no context/parameter values, so prerequisite traces never leak `privateParameters`). Prerequisites are transitive (a prerequisite's own gate is folded into the resolved map in topological order, so a deep-chain failure propagates) and cycles are rejected both at write time (`SetPrerequisites` → 400 with the cycle path) and at eval time (existing topo-sort cycle detection). The same gate resolves **identically in preview and the Rust SDK** — the SDK snapshot carries prerequisites + fallback by key, resolved locally over the transitive closure.
- **Cross-entity dependency integrity (delete-block):** deleting/archiving an entity that is still referenced returns HTTP **409 `dependency_exists`** with the dependent IDs. Producers: flag-service (flag referenced as a prerequisite), segmentation-service (segment referenced by a flag rule or another segment), experimentation-service (experiment referenced by another experiment's start-prerequisite). The `dependency_exists:<ids>` status sentinel (mirroring `flag_locked_by_experiment:`) is decoded source-agnostically in the gateway. A gateway **dependency-graph read API** (`/{entity_kind}/{entity_id}/dependencies`) returns upstream + downstream edges computed over existing RPCs. Experiment **start-prerequisites** (flag-in-variant / experiment-stopped) are enforced on both manual AND scheduled start (unmet → 409).
- **Admin UI:** schedule builder on flag/segment/experiment pages (one-shot + recurring, IANA tz picker, pending/active list, cancel/pause/resume, diff preview, run status), a prerequisites editor on the flag page (add/remove with required-variant + fallback pickers, live cycle warning), and a dependency-graph visualization with delete-blocked (409) surfacing + has-prerequisite / is-prerequisite badges.

## Admin UI

The admin console (`admin/`) is a React 19 + Vite SPA with full feature parity:

- **Flags:** Create/edit/archive flags; variant management; rule builder (AND/OR/NOT condition trees, segment picker, percentage rollout). Percentage-rollout outputs (both per-rule allocations and the flag-level default-rule distribution) authored via the `HashInputSelectorList` component — ordered list of cross-context selectors with drag + keyboard reorder, live worked-example banner, context-type + parameter autocomplete sourced from the registry. Evaluate-Preview "Test" panel with rule trace output
- **Segments:** Rule-based (condition expression builder) + list-based (context-typed include/exclude key lists); full CRUD; segment picker in flag rule builder
- **Events:** Full CRUD (`/v1/events*`) — register key + name + metric_type + optional JSON schema; archive (soft-delete); EditEventModal exposes name/metric_type/description/schema (event_key is immutable). EventDetail page surfaces recent firings, 14-day sparkline, the TestEventWidget (admin-auth `POST /v1/admin/events/track`), the back-link "Metrics referencing this event", and "Experiments depending on this event".
- **Metrics:** Full CRUD (`/v1/metrics*`) — kind picker (Aggregation/Ratio/Funnel), event-key autocomplete bound to registered events (strict — unknown keys flagged inline), aggregator + on_field + JsonLogic where_clause for aggregations, numerator/denominator dropdowns for ratios, FieldArray steps for funnels. Detail page calls `POST /v1/metrics/{id}/preview` for the ClickHouse-backed sparkline.
- **Context Explorer:** Browse observed context types and their parameter registry (autocomplete source for rule builder)
- **Eval Analytics:** Evaluation stats per flag via ClickHouse `eval_stats` route; sparklines in flag list
- **Experiments / Environments / SDK Keys / Org Users / Audit Log:** Full management UI
- **Pagination:** Top-level resource list views use **cursor pagination** (post-`platform_hardening_20260608`): `?cursor=<opaque>&limit=N` → `{items, next_cursor}`, with Previous/Next navigation (no page numbers). The Admin UI shares `usePaginatedList` (cursor stack for back-nav, URL-synced via `?cursor=`) + a Prev/Next `Pagination` component; the gateway shares `CursorParams`/`CursorPage` (`gateway::pagination`). The opaque token is implemented as an encoded offset over each service's existing `(offset, limit) → (items, total)` RPC (true keyset internals — dropping `OFFSET`/`COUNT(*) OVER()` — are a tracked follow-up `feature-flag-cj5` that changes only the token payload, not the contract). Experiment-detail **sub-lists** (iterations, exposures) intentionally remain page-based (`PaginationParams`/`PaginatedResponse`) since they back numbered detail views and the exposure-count stat needs the `total` the cursor envelope omits. (This supersedes `domain_boundaries_20260530`'s page-based canonical, per the `product-guidelines.md` cursor mandate.)

## Server-Side SDK (Rust — initial)
- `SdkClient::init(config)` blocks until first definition sync via gRPC, then polls at a configurable interval.
- Flag evaluation (`evaluate(&[EvalRequest], TraceLevel)`) is in-process: it delegates straight to `stitchd-core::evaluation::evaluate_flag`, so the SDK shares the exact orchestration the gateway's preview endpoint uses — same rule iteration, same cross-context hashing, same default-rule-distribution support. Each `EvalRequest` carries `flag_key` + `contexts: Vec<Context>`; multi-context bundles drive cross-context percentage hashing identically across SDK and preview.
- Rule-based segments evaluated locally; list-based segments resolved via REST lookup or optional LFU cache.
- Optional LFU membership cache pre-warms list-segment lookups for frequently-evaluated contexts (batch REST refresh on each poll cycle).
- Client-side SDKs (browser/mobile) and server-sent events are out of scope for the initial implementation.
- Future: streaming layer for server-pushed flag updates; direct event submission via SDK key.

## Data Stores
- PostgreSQL: flag/segment configuration, tenants, environments, SDK keys, audit logs
- ScyllaDB: list-segment entry storage (include/exclude lists, up to millions of entries per segment)
- ClickHouse: events, experiment results, metric aggregations

## ClickHouse Query Optimisations (Completed — db_optim_20260516)

The ClickHouse overhaul was completed as part of `db_optim_20260516`:

- **Injection fix:** `eval_stats` route now uses parameterized ClickHouse queries (no `format!()` SQL)
- **Experiment MVs:** `events_experiment_daily` (AggregatingMergeTree, keyed on `env_id, experiment_id, variant_key, metric_key, day`) + backfill migration; `experiment_queries.rs` reads from MVs
- **Partition tuning:** `events_v2` and `flag_evaluation_log_v2` use weekly `toMonday()` partitions + TTL
- **Scheduled stats:** 60-minute interval via `stitchd-stats-service`; Results API reads from pre-computed `experiment_results` table only
