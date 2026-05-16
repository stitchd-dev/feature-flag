# Specification: Clean SDK Implementation — sdks/ Foundation + Rust Server-Side SDK

## Overview

A clean greenfield SDK implementation that establishes `sdks/` as the home for all
language SDKs. The first deliverable is `sdks/spec/` — a language-agnostic
contract directory that every future SDK (Rust today, JS/Python/Go later) must
conform to. The second deliverable is `sdks/rust/`, a server-side Rust SDK built
on top of `stitchd-core` for in-process flag evaluation.

The existing `crates/stitchd-sdk` is **deleted entirely** as part of this track —
not migrated, not deprecated, not kept alongside. The new SDK is greenfield.

All SDK ↔ Backend traffic flows **exclusively** through `stitchd-gateway`, which
becomes the SDK trust boundary. The gateway validates the SDK key once (reusing
`SdkKeyCache` from `db_optim_20260516`), resolves `(env_id, project_id, org_id)`,
and forwards requests downstream with that resolved context in gRPC metadata.
Backend services (`stitchd-flag-service`, `stitchd-segmentation-service`)
**do not** re-validate the SDK key; they trust the gateway-propagated context.

All SDK ↔ Gateway communication is **poll-based**. Streaming (server-streaming
gRPC, SSE, WebSocket push) is explicitly out of scope and will be evaluated as a
future enhancement track.

## Functional Requirements

### 1. `sdks/spec/` Foundation (Language-Agnostic Contract)

Top-level `sdks/spec/` directory containing:

| Subdirectory | Contents |
|---|---|
| `sdks/spec/docs/` | Markdown behavioral spec: evaluation semantics, caching rules, polling lifecycle, event schema, error handling, retry/backoff policy |
| `sdks/spec/proto/` | Protobuf `.proto` files for gRPC contracts: `SyncDefinitions`, `IngestSdkEvalLog`, `BatchCheckListMembership` |
| `sdks/spec/openapi/` | OpenAPI 3.1 YAML for SDK REST endpoints: batch list-segment, event ingestion |
| `sdks/spec/schemas/` | JSON Schema definitions for: `EvalRequest`, `EvalResult`, `ReasoningTrace`, `FlagEvaluationEvent`, `SdkConfig` |
| `sdks/spec/fixtures/` | Conformance test vectors: canonical `(context, flag-def, expected-variant, expected-reasoning)` tuples that every SDK implementation must pass |

### 2. Gateway as SDK Trust Boundary

New route group on `stitchd-gateway`: `/v1/sdk/*` and a parallel gRPC server.

- **Auth middleware:** validates `x-sdk-key` header → looks up via `SdkKeyCache`
  → on success, resolves `(env_id, project_id, org_id)` and injects into:
  - REST: request extensions (`Extension<SdkContext>`)
  - gRPC: outbound metadata when calling backend services (`x-env-id`, `x-project-id`)
- **Authorization:** failure returns HTTP 401 / gRPC `Unauthenticated`
- **Backend services trust the metadata** — no per-request key validation downstream
- Gateway hosts a **tonic gRPC server** alongside its Axum REST server for SDK gRPC traffic
  (the gateway becomes the SDK gRPC endpoint; it acts as a thin proxy to flag-service)

### 3. Gateway SDK Surface

**gRPC (`/sdk.v1.SdkService/...` — hosted by gateway):**
- `SyncDefinitions(SyncRequest) -> DefinitionsSnapshot` — unary RPC. Returns the
  full current snapshot of flag definitions + rule-based segment definitions +
  list-based segment metadata (segment_id, name, context_type — but NOT entries).
  Called by the SDK on its `definition_poll_interval`. Gateway forwards to
  `stitchd-flag-service::SyncDefinitions` with `x-env-id` metadata.

**REST (gateway, auth-middleware-protected):**
- `POST /v1/sdk/segments/list:batch` — body: `{queries: [{context_type, key, segment_ids: [uuid]}]}`
  → returns `{results: [{context_type, key, memberships: {segment_id: bool}}]}`
  (used by SDK's LRU background refresh and on-miss fetch)
- `POST /v1/sdk/events:batch` — body: `{events: [FlagEvaluationEvent]}`
  → forwarded to `stitchd-flag-service::IngestSdkEvalLog` (no response body, 202 Accepted)

### 4. Backend Service Changes

**`stitchd-flag-service`:**
- New gRPC: `SyncDefinitions(env_id) -> DefinitionsSnapshot` — unary. Reads flag
  + segment definitions from repositories and returns a full snapshot. Reads
  `env_id` from gRPC metadata propagated by gateway. (No streaming / no deltas —
  every poll returns the full snapshot. Streaming is a deferred future enhancement.)
- New gRPC: `IngestSdkEvalLog(SdkEvalBatch) -> Empty` — converts batch to
  `EvalLogRow`s and forwards to the existing `eval_log_writer` MPSC sender.
- **No SDK key validation** in either RPC.

**`stitchd-segmentation-service`:**
- New gRPC: `BatchCheckListMembership({env_id, queries}) -> {results}`
  - One DB query per batch: `SELECT segment_id, entry_key FROM segment_list_entries
    WHERE segment_id = ANY($1) AND (context_type, entry_key) = ANY($2)`
  - Returns membership matrix; missing entries default to `false`
- Reads `env_id` from gRPC metadata.

### 5. Rust Server-Side SDK (`sdks/rust/`)

New crate at `sdks/rust/` with `package.name = "stitchd-sdk"` (the old crate name is reclaimed).

**Public API:**
```rust
pub struct SdkClient { /* internal */ }

impl SdkClient {
    pub async fn init(config: SdkConfig) -> Result<Arc<Self>, SdkError>;
    pub async fn evaluate(&self, requests: &[EvalRequest]) -> Vec<EvalResult>;
    pub async fn evaluate_with_reasoning(&self, requests: &[EvalRequest]) -> Vec<EvalResultWithReasoning>;
    pub async fn shutdown(self: Arc<Self>);
}

pub struct SdkConfig {
    pub gateway_url: String,                     // single endpoint for both REST + gRPC
    pub sdk_key: String,
    pub definition_poll_interval: Duration,      // default 30s
    pub list_segment_refresh_interval: Duration, // default 60s
    pub lru_max_entries: usize,                  // default 10_000
    pub event_flush_interval: Duration,          // default 5s
    pub event_batch_size: usize,                 // default 100
}

pub struct EvalRequest {
    pub flag_key: String,
    pub context: Context,
}

pub struct EvalResult {
    pub flag_key: String,
    pub variant: Variant,                        // from stitchd-core
}

pub struct EvalResultWithReasoning {
    pub flag_key: String,
    pub variant: Variant,
    pub reasoning: ReasoningTrace,               // which rule matched, why
}
```

**Internal architecture:**
- **Definition cache** (`ArcSwap<DefinitionSnapshot>`) — flag defs + rule-based segment defs + list-segment metadata
- **LRU cache** (`moka::sync::Cache<(ContextType, String), HashMap<SegmentId, bool>>`) — list-segment membership keyed on `(context_type, key)`
- **Three background tasks** (spawned on `SdkClient::init`):
  1. **Definition polling task:** every `definition_poll_interval`, makes a unary
     gRPC call to gateway `SdkService::SyncDefinitions`, swaps the in-memory snapshot atomically
  2. **LRU refresh task:** every `list_segment_refresh_interval`, batches all LRU keys, sends one REST batch request (filtered to segments referenced by current flag definitions), updates entries in place
  3. **Event flush task:** drains MPSC channel, batches up to `event_batch_size`, flushes every `event_flush_interval` via REST
- **Evaluation flow** (`evaluate` / `evaluate_with_reasoning`):
  1. Look up flag def in snapshot
  2. Use `stitchd-core` rule engine to evaluate; for any `InSegment` condition referencing a list-based segment:
     - Check LRU for `(context.type, context.key)`
     - **On miss:** synchronously fetch from gateway via batch endpoint (with single query), insert into LRU
     - **On hit:** use cached membership
  3. Emit `FlagEvaluationEvent` to MPSC channel (one per evaluated flag)
  4. Return result (with or without reasoning depending on entry point)

**LRU eviction:** `moka::sync::Cache` with `max_capacity` (consistent with auth-service usage); when capacity exceeded, evict least-recently-used. The background refresh task only refreshes entries currently resident in the LRU.

**List-segment filtering:** the LRU refresh task inspects the current flag
definition snapshot and only requests membership for segments referenced by at
least one flag rule (avoids polling for segments that are defined but not used).

### 6. Event Lifecycle

- Every successful `evaluate()` / `evaluate_with_reasoning()` call emits exactly one
  `FlagEvaluationEvent` per evaluated flag
- Event shape (defined in `sdks/spec/schemas/`):
  ```
  {
    "flag_key": "...",
    "environment_id": "...",
    "variant_key": "...",
    "context_type": "...",
    "context_key": "...",
    "evaluated_at": "RFC3339 timestamp",
    "matched_rule_id": "..." | null,
    "reasoning_included": false
  }
  ```
- Events queued in MPSC channel (bounded, configurable capacity)
- Background flush task drains, sends batch to `POST /v1/sdk/events:batch`
- Gateway forwards to `stitchd-flag-service::IngestSdkEvalLog`
- flag-service writes to existing `eval_log_writer` MPSC → ClickHouse `flag_evaluation_log_v2`

## Non-Functional Requirements

- **Configurability:** all intervals, batch sizes, LRU capacity exposed in `SdkConfig`
- **Error handling:** zero `unwrap()` / `panic!` in SDK runtime paths; all errors typed via `SdkError`
- **Concurrency:** `SdkClient` is `Send + Sync`, cloneable via `Arc`
- **Bounded memory:** LRU enforces `max_entries`; event channel is bounded
- **Graceful shutdown:** `SdkClient::shutdown()` drains the event buffer and aborts polling tasks
- **No panics on backend unavailability:** polling tasks log errors and retry with exponential backoff
- **Test coverage:** ≥90% on `sdks/rust/src/*` (CI tarpaulin gate)

## Acceptance Criteria

1. `crates/stitchd-sdk/` deleted; workspace `Cargo.toml` updated; downstream `dev-dependencies` references removed
2. `sdks/spec/` populated with all 5 artifact types (docs, proto, openapi, schemas, fixtures)
3. `sdks/rust/` crate compiles cleanly; `package.name = "stitchd-sdk"`
4. Gateway hosts both REST routes (`/v1/sdk/*`) and a tonic gRPC server for `SdkService`
5. SDK auth middleware uses existing `SdkKeyCache`; validates once, propagates `x-env-id` via gRPC metadata + REST extensions
6. Backend services (`stitchd-flag-service`, `stitchd-segmentation-service`) compile without SDK-key validation logic in their SDK-facing RPCs
7. Three new RPCs: `SdkService::SyncDefinitions` (gateway → flag-service), `SegmentationService::BatchCheckListMembership`, `FlagService::IngestSdkEvalLog`
8. SDK `evaluate()` cache miss on `(context_type, key)` triggers a synchronous on-demand fetch + LRU insert (verified by integration test against running gateway)
9. SDK background refresh task only requests membership for segments referenced by current flag defs (filter verified by test)
10. Eval events flow end-to-end: SDK → gateway → flag-service → `eval_log_writer` → ClickHouse (verified by integration test)
11. Conformance fixtures in `sdks/spec/fixtures/` are executable; Rust SDK passes 100% of them
12. ≥90% test coverage on `sdks/rust/src/*` enforced by CI

## Out of Scope

- Non-Rust SDK implementations (JavaScript, Python, Go) — explicitly deferred to
  future tracks. `sdks/spec/` is the foundation that ensures forward compatibility.
- Streaming definition sync (server-streaming gRPC, SSE, or WebSocket push). All
  SDK ↔ gateway communication is poll-based for this track. Streaming will be
  evaluated as a separate enhancement track once polling load characteristics are
  observed in production.
- Client-side browser/mobile SDKs (different security model — out of scope)
- SDK observability hooks (Prometheus metrics inside the SDK process) — beyond the
  eval event flow
- Migration tooling for callers of the old `crates/stitchd-sdk` — there are none
  in production app code (only dev-dependencies in `stitchd-db` tests); those
  references will be deleted as part of acceptance criterion 1
