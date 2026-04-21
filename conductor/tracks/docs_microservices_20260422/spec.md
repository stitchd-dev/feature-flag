# Spec: mdBook Docs — Microservice Architecture Update

## Overview

Update the mdBook documentation site and `cargo xtask docs` pipeline to reflect the
microservice decomposition. The monolith (`stitchd-server`) has been removed; the new
architecture has a REST gateway (`stitchd-gateway`) fronting six internal gRPC services.
This track covers: fixing the xtask pipeline, auditing and updating all OpenAPI
annotations on gateway routes, ensuring all new microservice endpoints are annotated
and appear in the generated spec, and restructuring the docs into Public/Gateway and
Internal sections with service coordination flows.

## Functional Requirements

### FR-1: Fix xtask OpenAPI Export
- Update `export_openapi()` in `crates/xtask/src/main.rs` to build and invoke
  `stitchd-gateway --export-openapi <path>` instead of `stitchd-server`
- The gateway binary must accept `--export-openapi <path>`, write `openapi.json`,
  then exit 0 (add this CLI flag if not already present)

### FR-2: Audit & Update OpenAPI Annotations on Gateway Routes
- Audit every Axum route handler in `stitchd-gateway` for `#[utoipa::path]`
  annotations
- Add or fix annotations for any route that is missing, incomplete, or still
  references the old monolith surface:
  - Correct request/response schemas
  - Correct security requirements (`sdk_key` vs `bearer_jwt`)
  - Correct tags (group by domain: flags, segments, events, experiments, auth, admin)
  - Summary and description strings accurate to the microservice model
- Run `cargo run --manifest-path crates/xtask/Cargo.toml -- docs` after annotation
  updates and verify the generated `openapi.json` reflects all changes

### FR-3: Document All New Microservice Endpoints
- Identify all REST endpoints added to `stitchd-gateway` during the microservice
  decomposition that were not present in the original monolith surface
- Ensure each new endpoint has a complete `#[utoipa::path]` annotation (covered by FR-2)
- Add a "What's New" note in `docs/src/gateway/overview.md` listing endpoints added
  since the monolith, so existing integrators know what changed

### FR-4: Public / Gateway Endpoints Section — three sub-sections

**FR-4a: Gateway Overview**
- `docs/src/gateway/overview.md`: gateway role, port, routing rules, auth header
  matrix (which routes accept SDK key vs JWT), error envelope format, "What's New"
  endpoint summary

**FR-4b: SDK APIs**
- `docs/src/gateway/sdk-api.md`: REST endpoints authenticated via `x-sdk-key`
  (flag evaluation, list-segment membership); auth model, error envelope, rate limits

**FR-4c: Gateway gRPC**
- `docs/src/gateway/grpc.md`: gRPC interface exposed by the gateway (definition-sync
  server-streaming passthrough for the SDK); service name, RPC(s), metadata auth
  (`x-sdk-key`), streaming behaviour
- Positioned between SDK APIs and Human JWT APIs

**FR-4d: Human JWT APIs (Admin UI)**
- `docs/src/gateway/admin-api.md`: REST endpoints authenticated via
  `Authorization: Bearer <jwt>`; intended consumers are the forthcoming Admin UI
  and direct API clients
- Covers: tenant/org management, flag CRUD, segment CRUD, experiment management,
  event registry, auth (login, MFA, OIDC, SAML, invites)
- Swagger UI embed rendered from `openapi.json`

**FR-4e: OpenAPI Regen**
- `docs/src/gateway/openapi.md`: how to regenerate, link to raw `openapi.json`

### FR-5: Internal gRPC Services Section
- New top-level section: **Internal gRPC Services**
- Domain-grouped sub-sections, each with an overview + one page per service:
  - **Auth & Identity** → `stitchd-auth-service`
  - **Flag & Segmentation** → `stitchd-flag-service`, `stitchd-segmentation-service`
  - **Events & Experimentation** → `stitchd-event-service`,
    `stitchd-experimentation-service`
- Each service page covers: responsibility, port, all RPCs, key message fields,
  auth requirements, link to auto-generated proto Markdown

### FR-6: Service Coordination Flows
- `docs/src/architecture/service-flows.md` with Mermaid sequence diagrams:
  1. **Flag Evaluation** — REST → gateway → auth-service → flag-service →
     segmentation-service (list lookup) → response
  2. **Event Ingestion** — SDK REST POST → gateway → auth-service → event-service →
     ClickHouse
  3. **Definition Sync** — SDK gRPC stream → gateway gRPC passthrough →
     flag-service server-streaming
  4. **Human Auth** — login POST → gateway → auth-service issues JWT →
     subsequent requests validated per RBAC

### FR-7: Updated gRPC Reference
- Regenerate gRPC Markdown from all `.proto` files (existing xtask step)
- Restructure SUMMARY.md gRPC entries under domain groups

### FR-8: SUMMARY.md Restructure
- Final section order:
  1. Introduction
  2. **Public / Gateway Endpoints**
     - Overview
     - SDK APIs
     - Gateway gRPC
     - Human JWT APIs (Admin UI)
     - OpenAPI Spec
  3. **Internal gRPC Services** (domain-grouped)
     - Auth & Identity
     - Flag & Segmentation
     - Events & Experimentation
  4. Service Coordination Flows
  5. Rust SDK
  6. Deployment & Self-Hosting
  7. Architecture

### FR-9: xtask SUMMARY Patching
- Update `patch_summary_grpc()` to emit domain-grouped entries

## Non-Functional Requirements

- `cargo xtask docs` must complete end-to-end without errors after all changes
- `mdbook build` must produce zero warnings
- All diagrams use Mermaid (mdbook-mermaid already installed)
- Service pages link to auto-generated proto Markdown rather than duplicating tables
- OpenAPI spec must pass `contract-check` CI job (`scripts/check_openapi_contract.py`)

## Acceptance Criteria

- [ ] `cargo xtask docs` builds successfully referencing `stitchd-gateway`
- [ ] All gateway route handlers have complete `#[utoipa::path]` annotations
- [ ] All endpoints added during microservice decomposition appear in `openapi.json`
- [ ] SUMMARY.md has "Public / Gateway Endpoints" and "Internal gRPC Services"
      as top-level sections
- [ ] Public section has three auth-model sub-sections: SDK APIs, Gateway gRPC,
      Human JWT APIs (Admin UI)
- [ ] Gateway gRPC page documents the definition-sync passthrough
- [ ] Each of the five internal services has its own documentation page
- [ ] `service-flows.md` contains all four Mermaid sequence diagrams
- [ ] gRPC section is grouped by domain
- [ ] `mdbook build` completes with zero warnings
- [ ] `contract-check` CI job passes

## Out of Scope

- Admin UI documentation
- Client-side SDK (browser/mobile) docs
- GitHub Pages deployment
- Versioned docs
