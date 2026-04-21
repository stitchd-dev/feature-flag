# Spec: Microservice Architecture Decomposition

## Overview

The current `stitchd-server` is a monolith handling auth, flags, segments,
events, and experiments in a single binary. This track decomposes it into six
independent microservices, each a separate Cargo workspace crate with its own
binary. All inter-service communication uses gRPC. A shared PostgreSQL instance
is used with per-service logical schemas; ClickHouse remains for events and
experiments.

## Services

| Service | Crate | Responsibility |
|---|---|---|
| Auth Service | `stitchd-auth-service` | Validate JWT/SDK keys, return RBAC context (tenant, env, roles) |
| Flag Service | `stitchd-flag-service` | Flag definitions, variant config, gRPC sync stream |
| Segmentation Service | `stitchd-segmentation-service` | Segment CRUD, rule/list evaluation |
| Experimentation Event Service | `stitchd-event-service` | Event ingestion into ClickHouse |
| Experimentation Service | `stitchd-experimentation-service` | Experiment CRUD + stats; calls Flag Service internally |
| Orchestration Service | `stitchd-gateway` | Pure API gateway — auth via Auth Service, delegates to domain services |

## Functional Requirements

1. **Auth Service**
   - Accepts a credential (Bearer JWT or `x-sdk-key` header value) via gRPC
   - Returns an RBAC context: `{tenant_id, environment_id, roles[], permissions[]}`
   - Handles token expiry, revocation checks (SDK key active status from DB)
   - Owns the `auth` schema in PostgreSQL (sessions, SDK keys, users)

2. **Flag Service**
   - Exposes gRPC endpoints: `GetFlagDefinitions` (stream for SDK sync), `GetFlag`, `ListFlags`, `MutateFlag`
   - Owns the `flags` schema (flags, variants, rules)
   - Does NOT evaluate flags — returns definitions only; evaluation stays in-SDK

3. **Segmentation Service**
   - Exposes gRPC: `GetSegment`, `ListSegments`, `EvaluateMembership`, `MutateSegment`
   - Owns the `segments` schema (segment definitions, list-segment partitioned tables)
   - Handles both rule-based and list-based segment types

4. **Experimentation Event Service**
   - Exposes gRPC: `IngestEvent` (validated against pre-registered event definitions)
   - Owns event definitions in `events` schema (PostgreSQL); writes raw events to ClickHouse
   - Rejects unknown event keys at ingestion boundary

5. **Experimentation Service**
   - Exposes gRPC: `CreateExperiment`, `GetExperiment`, `ListExperiments`, `GetResults`
   - Calls Flag Service (gRPC) internally to lock/verify flag state during experiment lifecycle
   - Owns `experiments` schema (PostgreSQL); reads aggregated results from ClickHouse

6. **Orchestration Service (`stitchd-gateway`)**
   - Exposes the public-facing REST API (Axum) — same surface as current `stitchd-server`
   - For every request: calls Auth Service (gRPC) to obtain RBAC context, then proxies to the appropriate domain service (gRPC)
   - Carries no business logic; maps gRPC responses to REST JSON responses
   - Also exposes the gRPC endpoint the SDK uses (proxied through to Flag/Segmentation services)

## Non-Functional Requirements

- All internal service-to-service calls use gRPC (tonic + prost)
- Each service has its own `main.rs`, independent port, and can be deployed/scaled independently
- All services live in the Cargo workspace under `crates/` (monorepo)
- Docker Compose wires all services + PostgreSQL + ClickHouse for local development
- Each service owns a distinct PostgreSQL schema; no cross-schema queries at runtime
- Existing `.proto` definitions in `stitchd-proto` are extended to cover all new service contracts
- SDK clients connect only to `stitchd-gateway` — never to internal services directly

## Acceptance Criteria

- [ ] Six separate service binaries compile independently from the workspace
- [ ] All inter-service calls use gRPC with proto-defined contracts in `stitchd-proto`
- [ ] Auth Service correctly validates both JWT and SDK key credentials
- [ ] Orchestration Service rejects unauthenticated requests before forwarding
- [ ] Experimentation Service calls Flag Service internally (no direct DB access to flags schema)
- [ ] SDK `SdkClient::init()` connects to `stitchd-gateway` and syncs flag definitions
- [ ] `docker-compose.yml` starts all six services with correct service discovery wiring
- [ ] All services have >95% unit test coverage
- [ ] No existing external API contract is broken (REST surface identical to current)

## Out of Scope

- Physical database-per-service separation (deferred)
- Service mesh / Kubernetes deployment (deferred)
- Client-side SDKs (already out of scope)
- Streaming flag updates via SSE (already deferred)
- ClickHouse MV optimisations (already deferred)
- Admin UI
