# Plan: mdBook Docs — Microservice Architecture Update

## Phase 1: Fix xtask Pipeline & Gateway Export Flag [checkpoint: 3c14b35]

- [x] Task 1: Add `--export-openapi <path>` CLI flag to `stitchd-gateway` [4c337af]
  - [x] Add clap/CLI argument parsing for `--export-openapi` in gateway `main.rs`
  - [x] In export mode: generate OpenAPI JSON via utoipa, write to path, exit 0
  - [x] Write test: gateway `--export-openapi` flag produces valid, non-empty JSON file

- [x] Task 2: Update `export_openapi()` in `crates/xtask/src/main.rs` [9008ba1]
  - [x] Replace `stitchd-server` build target with `stitchd-gateway`
  - [x] Replace binary path `target/debug/stitchd-server` with `target/debug/stitchd-gateway`
  - [x] Verify `cargo xtask docs` Step 2 completes without error locally

- [ ] Task: Conductor - User Manual Verification 'Fix xtask Pipeline & Gateway Export Flag' (Protocol in workflow.md)

## Phase 2: Audit & Update OpenAPI Annotations

- [ ] Task 1: Inventory all gateway route handlers
  - [ ] List all Axum routes in `stitchd-gateway/src/`
  - [ ] For each route, record: has `#[utoipa::path]`? correct tag? correct security? correct schema?
  - [ ] Produce checklist of routes needing new or fixed annotations

- [ ] Task 2: Add/fix annotations for SDK-key routes (flag eval, list-segment lookup)
  - [ ] Correct `security` to use `sdk_key` scheme
  - [ ] Correct tags to `flags` / `segments`
  - [ ] Verify request/response schemas match current handler types

- [ ] Task 3: Add/fix annotations for JWT/admin routes
  - [ ] Correct `security` to use `bearer_jwt` scheme
  - [ ] Correct tags: `auth`, `flags`, `segments`, `events`, `experiments`, `admin`
  - [ ] Ensure all endpoints added during microservice decomposition are annotated
  - [ ] Add summaries and descriptions accurate to the microservice model

- [ ] Task 4: Verify generated spec completeness
  - [ ] Run `cargo xtask docs` — confirm `openapi.json` includes all routes
  - [ ] Run `scripts/check_openapi_contract.py` — confirm `contract-check` passes

- [ ] Task: Conductor - User Manual Verification 'Audit & Update OpenAPI Annotations' (Protocol in workflow.md)

## Phase 3: SUMMARY.md Restructure & Gateway Doc Pages
<!-- execution: parallel -->
<!-- depends: -->

- [ ] Task 1: Create `docs/src/gateway/` directory and stub all pages
  - [ ] `overview.md`, `sdk-api.md`, `grpc.md`, `admin-api.md`, `openapi.md`
  <!-- files: docs/src/gateway/overview.md, docs/src/gateway/sdk-api.md, docs/src/gateway/grpc.md, docs/src/gateway/admin-api.md, docs/src/gateway/openapi.md -->

- [ ] Task 2: Rewrite `docs/src/SUMMARY.md` to new section order
  - [ ] Section 2: Public / Gateway Endpoints (Overview, SDK APIs, Gateway gRPC, Human JWT APIs, OpenAPI Spec)
  - [ ] Section 3: Internal gRPC Services (domain-grouped: Auth & Identity, Flag & Segmentation, Events & Experimentation)
  - [ ] Section 4: Service Coordination Flows (link to service-flows.md)
  - [ ] Retain sections 5–7: Rust SDK, Deployment, Architecture
  <!-- files: docs/src/SUMMARY.md -->
  <!-- depends: task1 -->

- [ ] Task 3: Update `patch_summary_grpc()` in `crates/xtask/src/main.rs`
  - [ ] Emit domain-grouped gRPC entries instead of flat per-file listing
  - [ ] Groups: Auth & Identity | Flag & Segmentation | Events & Experimentation
  <!-- files: crates/xtask/src/main.rs -->

- [ ] Task 4: Write `docs/src/gateway/overview.md`
  - [ ] Gateway role, port, routing rules
  - [ ] Auth header matrix (which routes accept SDK key vs JWT)
  - [ ] Error envelope format
  - [ ] "What's New" — endpoints added since the monolith
  <!-- files: docs/src/gateway/overview.md -->
  <!-- depends: task1 -->

- [ ] Task 5: Write `docs/src/gateway/sdk-api.md`
  - [ ] REST endpoints authenticated via `x-sdk-key`
  - [ ] Auth model, error envelope, rate limits
  <!-- files: docs/src/gateway/sdk-api.md -->
  <!-- depends: task1 -->

- [ ] Task 6: Write `docs/src/gateway/grpc.md`
  - [ ] gRPC interface exposed by the gateway (definition-sync passthrough)
  - [ ] Service name, RPCs, `x-sdk-key` metadata auth, streaming behaviour
  <!-- files: docs/src/gateway/grpc.md -->
  <!-- depends: task1 -->

- [ ] Task 7: Write `docs/src/gateway/admin-api.md`
  - [ ] JWT-authenticated REST endpoints for Admin UI consumers
  - [ ] Swagger UI embed (rendered from `openapi.json`)
  <!-- files: docs/src/gateway/admin-api.md -->
  <!-- depends: task1 -->

- [ ] Task 8: Write `docs/src/gateway/openapi.md`
  - [ ] How to regenerate spec, link to raw `openapi.json`
  <!-- files: docs/src/gateway/openapi.md -->
  <!-- depends: task1 -->

- [ ] Task: Conductor - User Manual Verification 'SUMMARY.md Restructure & Gateway Doc Pages' (Protocol in workflow.md)

## Phase 4: Service Coordination Flows
<!-- execution: sequential -->
<!-- depends: -->

- [ ] Task 1: Write `docs/src/architecture/service-flows.md`
  - [ ] Flag Evaluation Mermaid sequence diagram
  - [ ] Event Ingestion Mermaid sequence diagram
  - [ ] Definition Sync Mermaid sequence diagram
  - [ ] Human Auth Mermaid sequence diagram
  <!-- files: docs/src/architecture/service-flows.md -->

- [ ] Task 2: Add `service-flows.md` entry to `docs/src/SUMMARY.md` under Architecture
  <!-- files: docs/src/SUMMARY.md -->

- [ ] Task: Conductor - User Manual Verification 'Service Coordination Flows' (Protocol in workflow.md)

## Phase 5: Internal gRPC Service Pages
<!-- execution: parallel -->
<!-- depends: -->

- [ ] Task 1: Create domain-grouped directory structure under `docs/src/internal/`
  - [ ] `docs/src/internal/auth/`, `docs/src/internal/flag-segmentation/`, `docs/src/internal/events-experimentation/`
  <!-- files: docs/src/internal/ -->

- [ ] Task 2: Write Auth & Identity page — `stitchd-auth-service`
  - [ ] Responsibility, port, all RPCs, key message fields, auth requirements
  <!-- files: docs/src/internal/auth/auth-service.md -->
  <!-- depends: task1 -->

- [ ] Task 3: Write Flag service page — `stitchd-flag-service`
  - [ ] Responsibility, port, all RPCs, key message fields, auth requirements
  <!-- files: docs/src/internal/flag-segmentation/flag-service.md -->
  <!-- depends: task1 -->

- [ ] Task 4: Write Segmentation service page — `stitchd-segmentation-service`
  - [ ] Responsibility, port, all RPCs, key message fields, auth requirements
  <!-- files: docs/src/internal/flag-segmentation/segmentation-service.md -->
  <!-- depends: task1 -->

- [ ] Task 5: Write Event service page — `stitchd-event-service`
  - [ ] Responsibility, port, all RPCs, key message fields, auth requirements
  <!-- files: docs/src/internal/events-experimentation/event-service.md -->
  <!-- depends: task1 -->

- [ ] Task 6: Write Experimentation service page — `stitchd-experimentation-service`
  - [ ] Responsibility, port, all RPCs, key message fields, auth requirements
  <!-- files: docs/src/internal/events-experimentation/experimentation-service.md -->
  <!-- depends: task1 -->

- [ ] Task: Conductor - User Manual Verification 'Internal gRPC Service Pages' (Protocol in workflow.md)

## Phase 6: Final Build Verification
<!-- depends: phase3, phase4, phase5 -->

- [ ] Task 1: Run `cargo xtask docs` end-to-end — fix any remaining issues
- [ ] Task 2: Verify `mdbook build` produces zero warnings
- [ ] Task 3: Run `scripts/check_openapi_contract.py` — confirm contract-check passes
- [ ] Task 4: Smoke-check generated `docs/book/index.html` navigable with correct sections

- [ ] Task: Conductor - User Manual Verification 'Final Build Verification' (Protocol in workflow.md)
