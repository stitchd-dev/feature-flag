# Implementation Plan: Rust Server-Side SDK

## Phase 1: gRPC Proto & REST API Extensions

- [x] Task 1: Define `FlagSyncService` proto <!-- 802e1e1 -->
  - `proto/flag_sync/v1/service.proto`
  - `SyncResponse`: flags (rules, variants, hashing), rule-based segment defs,
    list-based segment metadata (key + id only)
  - Export from `stitchd-proto/src/lib.rs`

- [x] Task 2: Add list-check REST endpoints to stitchd-server <!-- 802e1e1 -->
  - `POST /v1/environments/{env_id}/segments/list-check`
    - Body: `{ context_type, context_key, segment_keys }`
    - Auth: `x-sdk-key` header
    - Logic: for each segment_key, check `segment_list_entries` table
  - `POST /v1/environments/{env_id}/segments/list-check/batch`
    - Body: `{ contexts: [{type,key}], segment_keys }`
    - Returns membership matrix

- [x] Task 3: Add SDK key auth middleware for REST (list-check routes) <!-- 802e1e1 -->
  - Extract `x-sdk-key` header, validate + resolve environment
  - Apply to list-check routes only

- [ ] Task: Conductor - User Manual Verification 'Phase 1: gRPC Proto & REST API Extensions' (Protocol in workflow.md)

## Phase 2: Server-Side gRPC Sync Endpoint

- [ ] Task 1: Implement `FlagSyncServiceImpl` gRPC handler
  - `crates/stitchd-server/src/grpc/flag_sync.rs`
  - Auth SDK key from `x-sdk-key` metadata, resolve environment
  - Load flags (variants, rules, hashing) + segments (rules + list meta)
  - Map domain → proto response

- [ ] Task 2: Wire into tonic server in `main.rs`

- [ ] Task 3: Integration tests for gRPC sync
  - Valid key → correct payload
  - Invalid/revoked key → UNAUTHENTICATED

- [ ] Task: Conductor - User Manual Verification 'Phase 2: Server-Side gRPC Sync Endpoint' (Protocol in workflow.md)

## Phase 3: SDK Client Core (Definitions + Rule-Based Evaluation)

- [ ] Task 1: Define SDK config and cache types
  - `SdkConfig`, `LfuConfig`
  - `DefinitionCache { flags, rule_segments, list_segments_meta }`

- [ ] Task 2: gRPC client + `fetch_definitions()`
  - Tonic channel, inject `x-sdk-key` metadata
  - Deserialize proto → DefinitionCache

- [ ] Task 3: `SdkClient::init()` + background polling task
  - Block on first `fetch_definitions()`, spawn polling loop
  - `CancellationToken` drops polling task on `SdkClient` drop

- [ ] Task 4: `evaluate()` core — rule-based segments + list-check fallback
  - Local rule evaluation via `stitchd-core`
  - For list segments: single `list-check` API call (no LFU yet)

- [ ] Task: Conductor - User Manual Verification 'Phase 3: SDK Client Core' (Protocol in workflow.md)

## Phase 4: LFU Segment Membership Cache

- [ ] Task 1: LFU frequency tracker
  - Track evaluation frequency per `context_key` within `window`
  - Identify hot set (top `capacity` contexts)

- [ ] Task 2: Proactive batch refresh
  - On each poll cycle, call `list-check/batch` for hot contexts × all list segments
  - Store results in `HashMap<(ContextKey, SegmentKey), bool>`

- [ ] Task 3: Wire LFU into `evaluate()`
  - Check LFU cache before calling `list-check` API
  - Update frequency counter on each `evaluate()` call
  - Invalidate entries for segments whose definition changed in last poll

- [ ] Task: Conductor - User Manual Verification 'Phase 4: LFU Segment Membership Cache' (Protocol in workflow.md)

## Phase 5: Testing & Validation

- [ ] Task 1: Unit tests
  - Rule-based: correct variant, no network calls
  - List-based (no LFU): 1 API call per evaluation
  - List-based (LFU warm): 0 API calls
  - LFU eviction: cold context removed when capacity exceeded

- [ ] Task 2: Integration tests (SDK against live server)
  - Full flow: init → evaluate with rule-based and list-based segments
  - LFU: warm context → no network; cold → API call
  - Server down after init: rule eval succeeds, list eval surfaces error

- [ ] Task: Conductor - User Manual Verification 'Phase 5: Testing & Validation' (Protocol in workflow.md)
