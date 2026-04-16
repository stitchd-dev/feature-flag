# Initial Concept
Stitchd Feature Flag is a self-hosted platform for feature flagging and experimentation.

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
- **Current:** Self-hosted (primary focus)
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

## Modules

### 1. Segmentation
- Rule-Based Segments: rules evaluated against client Contexts
- List-Based Segments: per context-type include/exclude key lists
  - Persistence: monthly range-partitioned storage for list entries via pg_partman

### 2. Feature Flags
- Typed flags: `int | double | bool | string | json`; variants must match flag type
- States: enabled (default rule + custom rules) / disabled
- Output: specific variant OR percentage allocation (0.1% granularity)
  hash(targeted context keys/params, flag key, project id, environment)

### 3. Experimentation
- Events: pre-registered only; each event has a known key and typed metric value 
  (bool/int/double) — unknown events are rejected at ingestion
- Event payload: `{_type, key}` context + metric key + typed value + timestamp
- Experiments: bound to a flag rule, duration-locked (flag frozen while active)
- Models: Frequentist or Bayesian (with/without CUPED)
- Metrics: event count, numeric aggregation (sum/avg/percentile), funnel/conversion
- Future: warehouse-backed event ingestion

### 4. Rule Engine
- Core: ordered rule list (first true = exit); AND combinator; per-rule NOT
- Segmentation rules: inherit core
- Feature flag rules: inherit core + "Is in Segment" + "Flag evaluated with variant X"

## Client SDK (Rust — initial)
- Init with Contexts → server returns all flag rules/variants + segment data
- All evaluation runs client-side
- Fixed-interval polling for updates
- Future: streaming layer for server-pushed evaluated flags
- Direct event submission via SDK key (scoped to project/environment)

## Data Stores
- PostgreSQL: flag/segment configuration, tenants, environments, SDK keys
- ClickHouse: events, experiment results, metric aggregations
