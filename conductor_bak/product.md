# Initial Concept
Stitchd Feature Flag is a high-performance Feature Flagging & Experimentation platform focused on self-hosted deployment and statistical rigor.

# Product Guide

## Vision
Stitchd Feature Flag targets internal engineering teams, SaaS product teams, and data/growth teams who need reliable flag evaluation and statistically rigorous A/B experimentation. The platform is designed for high-concurrency, self-hosted environments with a future path toward a Cloud SaaS offering. *Note: Admin UI is a separate, upcoming project.*

## Target Users
- **Internal Engineering Teams:** Focus on self-hosted deployments and infrastructure control.
- **SaaS Product Teams:** Multi-tenant support for managing features across diverse customer bases.
- **Data / Growth Teams:** Running A/B and multivariate experiments with rigorous statistical models.

## Deployment & Multi-Tenancy
- **Deployment Model:** Self-hosted (primary focus), with a future Cloud SaaS offering.
- **Tenancy Structure:** Each Tenant → Multiple Environments → SDK Keys (min 1 active; supports rotation via create/revoke).

## Scoping Model
- **Project Level:** Feature Flag definitions, Variant configurations.
- **Environment Level:** Rules, Segments, Experiments, Events.

## Core Context & Intelligence
- **Evaluation Context:** `{_type, key, parameters: Map<String, int|double|semver|string|boolean>, privateParameters: List<String>}`.
- **Privacy:** `privateParameters` identifies fields excluded from all logging.
- **Context Intelligence Layer:** Observes contexts to maintain a registry of known types, properties, and value ranges/enums. Powers Admin UI autocomplete/dropdowns.

## Data Persistence & Integrity
- **Optimistic Concurrency:** All mutable entities use version-based optimistic locking to prevent lost updates.
- **Audit Logging:** Every mutation (create, update, soft-delete) records the actor, resource, and specific changes.
- **Soft Deletion:** Business-critical entities use soft-deletion to maintain data relationships and auditability.

## Modules

### 1. Segmentation
- **Rule-Based Segments:** Evaluated against client Contexts.
- **List-Based Segments:** Include/exclude key lists per context-type.
- **Persistence:** Monthly range-partitioned storage via `pg_partman`.

### 2. Feature Flags
- **Typed Flags:** `int | double | bool | string | json`; variants must match flag type.
- **States:** Enabled (default rule + custom rules) / Disabled.
- **Output:** Specific variant OR percentage allocation (0.1% granularity) based on deterministic hashing.

### 3. Experimentation
- **Strict Events:** Pre-registered only; unknown events are rejected. Payload includes context, metric key, and typed value.
- **Experiments:** Bound to a flag rule; duration-locked (flag frozen while active).
- **Statistical Models:** Frequentist or Bayesian (with/without CUPED).
- **Metrics:** Event count, numeric aggregation (sum/avg/percentile), and funnel/conversion.

### 4. Rule Engine
- **Core Logic:** Ordered rule list (first true = exit); AND combinator; per-rule NOT.
- **Capabilities:** Inherits core logic + "Is in Segment" + "Flag evaluated with variant X".

## Client SDK (Rust Initial)
- **Local Evaluation:** Server returns all rules/variants/segments; evaluation runs client-side.
- **Updates:** Fixed-interval polling (Future: server-pushed streaming layer).
- **Ingestion:** Direct event submission via SDK key scoped to project/environment.

## Data Stores
- **PostgreSQL:** Configuration (flags, segments, tenants, environments, SDK keys, audit logs).
- **ClickHouse:** High-volume events, experiment results, and metric aggregations.
