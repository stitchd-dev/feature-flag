# Implementation Plan: Clean SDK Implementation — sdks/ Foundation + Rust Server-Side SDK

Track: `sdk_rewrite_20260516`

---

## Phase 1: SDK Spec Foundation (`sdks/spec/`)

Establishes the language-agnostic contract that backend changes (Phase 3-4) and
the Rust SDK (Phase 5) will both consume. Must come first so proto/OpenAPI shapes
are pinned before downstream phases reference them.

- [x] Task 1: Scaffold `sdks/spec/` directory structure
  - Create `sdks/spec/{docs,proto,openapi,schemas,fixtures}/`
  - Add top-level `sdks/spec/README.md` explaining the contract model
  - Add `sdks/README.md` explaining the overall `sdks/` layout

- [x] Task 2: Write Markdown behavioral spec (`sdks/spec/docs/`)
  - `01-overview.md` — SDK responsibilities, gateway trust boundary
  - `02-evaluation-semantics.md` — eval flow, reasoning shape, batch semantics
  - `03-caching.md` — definition snapshot lifecycle, LRU for list-segment membership, miss/hit/refresh behaviour
  - `04-polling.md` — definition poll interval, list-segment refresh interval, exponential backoff on failure
  - `05-events.md` — event schema, batching/flush semantics, at-least-once delivery
  - `06-errors.md` — error taxonomy, retry policy

- [x] Task 3: Author Protobuf contracts (`sdks/spec/proto/`)
  - `sdk_service.proto` — `SdkService.SyncDefinitions(SyncRequest) -> DefinitionsSnapshot`
    + `IngestSdkEvalLog(SdkEvalBatch) -> Empty` (gateway-hosted surface)
  - `segmentation_sdk.proto` — `BatchCheckListMembership({env_id, queries}) -> {results}`
    (backend RPC called by gateway)
  - Reuse types from `proto/flags/v1/*` + `proto/segments/v1/*` where possible
  - TDD: schema-validation test that compiles every `.proto` with `protoc`

- [x] Task 4: Author OpenAPI 3.1 contracts (`sdks/spec/openapi/sdk.yaml`)
  - `POST /v1/sdk/segments/list:batch` — request/response schemas
  - `POST /v1/sdk/events:batch` — request schema (202 Accepted, no body)
  - Reference the JSON Schemas from Task 5 via `$ref`

- [x] Task 5: Author JSON Schemas (`sdks/spec/schemas/`)
  - `eval_request.schema.json`
  - `eval_result.schema.json`
  - `eval_result_with_reasoning.schema.json`
  - `reasoning_trace.schema.json`
  - `flag_evaluation_event.schema.json`
  - `sdk_config.schema.json`
  - TDD: `jsonschema` library validates each schema is itself well-formed

- [x] Task 6: Author initial conformance fixtures (`sdks/spec/fixtures/`)
  - Directory layout: `fixtures/{evaluation,caching,events}/<scenario>/`
  - Each scenario contains: `flag_definitions.json`, `segment_definitions.json`,
    `requests.json` (inputs), `expected.json` (outputs)
  - Cover at minimum: bool flag with default rule; string flag with one custom rule;
    percentage rollout; rule referencing rule-based segment; rule referencing
    list-based segment (hit + miss); reasoning trace shape
  - TDD: fixture-validity test that round-trips each fixture through JSON Schema validation

- [x] Task: Conductor - User Manual Verification 'SDK Spec Foundation' (Protocol in workflow.md) [checkpoint: 2d21beb]

---

## Phase 2: Workspace Cleanup — Delete Old SDK + Scaffold New [checkpoint: 2d21beb]

Removes `crates/stitchd-sdk` and creates the empty `sdks/rust/` crate shell so
later phases have a place to land.

- [x] Task 1: Delete `crates/stitchd-sdk/` entirely
  - `git rm -r crates/stitchd-sdk`
  - Remove from workspace `[workspace.members]` in root `Cargo.toml`
  - Remove `stitchd-sdk` from `dev-dependencies` in `crates/stitchd-db/Cargo.toml`
  - Delete any tests in `crates/stitchd-db/tests/` that depend on it
  - Verify `cargo check --workspace` passes

- [x] Task 2: Scaffold `sdks/rust/` crate skeleton
  - Create `sdks/rust/Cargo.toml` with `package.name = "stitchd-sdk"`,
    workspace inheritance, dependencies (`stitchd-core`, `stitchd-proto`,
    `tonic`, `reqwest`, `tokio`, `moka`, `arc-swap`, `thiserror`)
  - Create `sdks/rust/src/lib.rs` — empty module declarations only
  - Create `sdks/rust/README.md`
  - Add `sdks/rust` to workspace `[workspace.members]`
  - Verify `cargo check -p stitchd-sdk` passes (compiles an empty crate)

- [x] Task: Conductor - User Manual Verification 'Workspace Cleanup' (Protocol in workflow.md) [checkpoint: cbf2be5]

---

## Phase 3: Backend RPCs (No Auth — Trust Gateway Metadata) [checkpoint: cbf2be5]

Implements the backend-side RPCs that the gateway will call. These services do
NOT validate SDK keys; they trust `x-env-id` propagated by the gateway.

- [x] Task 1: Generate Rust bindings from new proto files
  - Wire `sdks/spec/proto/sdk_service.proto` + `segmentation_sdk.proto` into
    `crates/stitchd-proto/build.rs`
  - Verify `cargo check -p stitchd-proto` regenerates without warnings

- [x] Task 2: `stitchd-flag-service::SyncDefinitions` (unary RPC)
  - TDD: write failing test in `crates/stitchd-flag-service/src/grpc/` that
    calls `SyncDefinitions` with `x-env-id` metadata and asserts a full
    snapshot is returned (flags + rule-based segments + list-segment metadata)
  - Implement handler: read `env_id` from metadata, fetch flags via
    `flag_repository.list_by_environment_paginated(env_id, 0, u32::MAX)`,
    fetch segments via `segment_repository.list_by_environment(env_id)`,
    assemble `DefinitionsSnapshot` proto
  - **No SDK-key auth** — handler trusts metadata

- [x] Task 3: `stitchd-flag-service::IngestSdkEvalLog` (unary RPC)
  - TDD: write failing test that sends a batch and asserts rows are appended
    to the `eval_log_writer` MPSC channel
  - Implement handler: convert `SdkEvalBatch` → `Vec<EvalLogRow>` → forward to
    existing `eval_log_writer::EvalLogSender`
  - **No SDK-key auth**

- [x] Task 4: `stitchd-segmentation-service::BatchCheckListMembership`
  - TDD: write failing test that sends 5 `(context_type, key, segment_ids[])`
    queries and asserts a correct membership matrix
  - Add repo method `find_memberships_batch(env_id, queries) -> Vec<MembershipResult>`
    on `PgSegmentRepository` — single SQL query using `WHERE segment_id = ANY($1)
    AND (context_type, entry_key) = ANY($2)`
  - Implement gRPC handler
  - **No SDK-key auth**

- [x] Task: Conductor - User Manual Verification 'Backend SDK RPCs' (Protocol in workflow.md) [checkpoint: 3f4c7c3]

---

## Phase 4: Gateway SDK Surface [checkpoint: 3f4c7c3]

Adds the gateway-side SDK trust boundary: auth middleware, gRPC server, REST
routes. Reuses `SdkKeyCache` from `db_optim_20260516`.

- [x] Task 1: SDK auth middleware
  - TDD: tests for valid key → injects `SdkContext { env_id, project_id, org_id }`
    into request extensions; invalid key → HTTP 401; missing header → HTTP 401
  - Implement `sdk_auth_middleware` in `crates/stitchd-gateway/src/middleware/`
  - Uses existing `SdkKeyCache` from `stitchd-auth-service` (shared via
    `GatewayState` or new injection)

- [x] Task 2: Gateway REST routes for SDK
  - `POST /v1/sdk/segments/list:batch` — extracts `SdkContext`, calls
    `stitchd-segmentation-service::BatchCheckListMembership` with `x-env-id`
    metadata, returns membership matrix
  - `POST /v1/sdk/events:batch` — extracts `SdkContext`, calls
    `stitchd-flag-service::IngestSdkEvalLog` with `x-env-id` metadata, returns 202
  - Both routes registered under `sdk_auth_middleware`
  - TDD: route-level tests using `tower::ServiceExt::oneshot`

- [ ] Task 3: Gateway gRPC server hosting `SdkService`
  - Add tonic server to `stitchd-gateway` (currently REST-only) — run in parallel
    with the Axum server using `tokio::try_join!`
  - Implement `SdkService::SyncDefinitions` as a proxy: read `x-sdk-key` from
    incoming gRPC metadata, validate via `SdkKeyCache`, resolve `env_id`, call
    `stitchd-flag-service::SyncDefinitions` with `x-env-id` metadata, return
    response to caller
  - Add gRPC port to gateway config (e.g., `GATEWAY_GRPC_PORT=50050`)
  - TDD: integration test using a tonic client against the gateway

- [ ] Task: Conductor - User Manual Verification 'Gateway SDK Surface' (Protocol in workflow.md)

---

## Phase 5: Rust SDK Implementation (`sdks/rust/`)

The actual SDK. Built incrementally with TDD.

- [ ] Task 1: `SdkConfig` + `SdkError` + module skeleton
  - TDD: config defaults (poll=30s, refresh=60s, lru=10_000, flush=5s, batch=100)
  - Implement in `sdks/rust/src/config.rs` and `sdks/rust/src/error.rs`

- [ ] Task 2: Definition snapshot + atomic swap
  - TDD: writer task swaps snapshot; reader sees consistent view
  - `DefinitionSnapshot { flags: HashMap, rule_segments: HashMap, list_segments: HashMap }`
  - Use `ArcSwap<DefinitionSnapshot>` for lock-free reads

- [ ] Task 3: LRU cache for list-segment membership
  - TDD: insert / get / eviction at capacity / refresh-in-place
  - `LruCache<(ContextType, String), HashMap<SegmentId, bool>>` keyed on
    `(context_type, context_key)`
  - Use `moka::sync::Cache` with `max_capacity` (consistent with auth-service usage)

- [ ] Task 4: Event queue + flush task
  - TDD: events flushed on interval OR on batch-size threshold; bounded channel
  - MPSC bounded channel, background task drains and batches POSTs to gateway
  - Graceful shutdown drains remaining events before exit

- [ ] Task 5: Definition polling task
  - TDD: polls every `definition_poll_interval` via gRPC unary call;
    on success swaps snapshot; on failure logs + exponential backoff
  - Implement as `tokio::spawn` task launched from `SdkClient::init`
  - Uses tonic client to gateway gRPC port

- [ ] Task 6: LRU background refresh task
  - TDD: refresh task batches all resident LRU keys, calls gateway REST
    `POST /v1/sdk/segments/list:batch`, updates entries in place;
    only requests memberships for segments referenced by current flag defs
  - Filter computed from current `DefinitionSnapshot`

- [ ] Task 7: `SdkClient::init()` wiring
  - TDD: init blocks until first successful definition sync; failure propagates
  - Constructs cache structs, spawns 3 background tasks, returns `Arc<SdkClient>`

- [ ] Task 8: `evaluate()` + `evaluate_with_reasoning()`
  - TDD: bool flag default rule; string flag with rule referencing rule-based segment;
    flag with rule referencing list-based segment (LRU hit); LRU miss → synchronous
    on-demand fetch from gateway → insert into LRU → use; reasoning trace shape
  - Uses `stitchd-core` rule engine internally
  - Emits one `FlagEvaluationEvent` to the event queue per evaluated flag

- [ ] Task 9: Graceful shutdown
  - TDD: `shutdown()` drains event buffer, aborts polling tasks, returns when
    all tasks have exited
  - Use `tokio::sync::Notify` or task `JoinHandle::abort()`

- [ ] Task: Conductor - User Manual Verification 'Rust SDK Implementation' (Protocol in workflow.md)

---

## Phase 6: Integration + Conformance

End-to-end validation against running stack + conformance fixture runner.

- [ ] Task 1: End-to-end integration test
  - Spin up gateway + flag-service + segmentation-service + Postgres + ClickHouse
    via the existing test harness
  - Provision an SDK key + env via test setup
  - SDK init → poll picks up flag defs → `evaluate()` → assert variant
  - Verify cache-miss path: evaluate against an unseen `(context_type, key)`
    → assert HTTP call to gateway → assert LRU populated
  - Verify event ingestion: trigger N evaluations → assert N rows land in
    ClickHouse `flag_evaluation_log_v2` (via the existing eval_log_writer)

- [ ] Task 2: Conformance test runner
  - Implement `sdks/rust/tests/conformance.rs` that walks `sdks/spec/fixtures/`,
    loads each scenario, runs through the SDK's evaluation logic in isolation
    (no network — use in-memory definitions), asserts results match `expected.json`
  - All fixtures from Phase 1 Task 6 must pass

- [ ] Task 3: Coverage gate + CI integration
  - Run `cargo tarpaulin -p stitchd-sdk` — assert ≥90%
  - Wire `sdks/rust` into root CI workflow
  - Document SDK usage in `sdks/rust/README.md` with a runnable example

- [ ] Task: Conductor - User Manual Verification 'Integration + Conformance' (Protocol in workflow.md)
