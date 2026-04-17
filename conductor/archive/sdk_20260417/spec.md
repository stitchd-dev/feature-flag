# Specification: Rust Server-Side SDK (Feature)

## Overview
Implement `stitchd-sdk` as a Rust server-side SDK. Backend services call `evaluate()` per
request; the SDK evaluates flags in-process. Rule-based segments are evaluated locally
using synced definitions. List-based segments require a server lookup, with an optional
LFU cache to pre-warm membership for frequently-seen contexts.

## Functional Requirements

### 1. SDK Initialization
- `SdkClient::init(config: SdkConfig) -> Result<Arc<SdkClient>>`
  - Blocks until first successful definition sync.
  - Starts background polling task.
  - Returns error on: server unreachable, invalid/revoked SDK key.
- `SdkConfig`:
  - `server_url: String`
  - `sdk_key: String`
  - `poll_interval: Duration` (default 30s) — for definition sync
  - `lfu: Option<LfuConfig>`

### 2. Flag & Segment Definition Sync (gRPC)
- New `FlagSyncService` gRPC service added to `stitchd-proto`.
  - SDK key in `x-sdk-key` gRPC metadata.
  - Response: all flag definitions (rules, variants, hashing config) + all
    **rule-based segment definitions** (segment rules) for the resolved environment.
  - List-based segment definitions (key, id) also returned — but NOT the list entries
    themselves (those stay in DB, queried on demand).
- Server authenticates SDK key, resolves environment, returns data.

### 3. In-Memory Definition Cache
- `Arc<RwLock<DefinitionCache>>` holds:
  - `HashMap<FlagKey, FlagDefinition>` — rules, variants, hashing config
  - `HashMap<SegmentKey, RuleBasedSegmentDef>` — rules for local evaluation
  - `HashMap<SegmentKey, ListSegmentMeta>` — id + key (for API lookup)
- Atomically replaced on each successful poll. Stale-while-revalidate on failure.

### 4. Background Definition Polling
- tokio task polls at `poll_interval`, replaces cache on success.
- Cancelled via `CancellationToken` when `SdkClient` is dropped.

### 5. Flag Evaluation (Per-Call, Lazy Segment Resolution)
- `evaluate(flag_key: &str, context: &EvaluationContext) -> Result<Option<VariantValue>>`
  - Returns `None` if flag not found or disabled.
  - Steps:
    1. Look up `FlagDefinition` from local cache.
    2. Walk flag rules to collect all referenced segment keys.
    3. For each **rule-based segment**: evaluate locally via `stitchd-core`.
    4. For each **list-based segment**: resolve membership (see §6).
    5. Delegate to `stitchd-core::FlagEvaluator` with resolved memberships.
    6. Return matched variant or default variant.
- `evaluate()` is async (list-based fallback may need network).

### 6. List-Based Segment Membership Resolution
Two modes depending on whether the context is in the LFU cache:

**Without LFU (or LFU miss):**
- Call server REST endpoint (new): `POST /v1/environments/{env_id}/segments/list-check`
  - Body: `{ context_type, context_key, segment_keys: [SegmentKey] }`
  - Response: `{ memberships: { [segment_key]: bool } }`
- Result used for this evaluation; not cached (unless LFU is enabled and context becomes hot).

**With LFU cache (§7) — cache hit:**
- Return pre-computed membership from LFU cache. No network call.

### 7. Optional LFU Segment Membership Cache (List Segments Only)
- Enabled via `SdkConfig.lfu: Some(LfuConfig { capacity: usize, window: Duration })`.
- The SDK tracks evaluation frequency per `context_key` using an LFU counter.
- For contexts within the top-`capacity` by frequency within `window`:
  - Proactively fetches **all list segment memberships** for those contexts from the server
    via a batch endpoint: `POST /v1/environments/{env_id}/segments/list-check/batch`
    - Body: `{ contexts: [{type, key}], segment_keys: [SegmentKey] }`
    - Response: `{ results: [{ context_key, memberships: {[segment_key]: bool} }] }`
  - Membership results cached in `HashMap<(ContextKey, SegmentKey), bool>`.
  - Cache is refreshed on each definition poll cycle (for LFU-hot contexts).
  - On segment definition change (detected during definition sync), invalidate affected entries.
  - Polling for LFU batch refresh shares the same `poll_interval` as definition sync.

### 8. SDK Key Authentication (Server-Side)
- `x-sdk-key` gRPC metadata for definition sync.
- `x-sdk-key` header for REST list-check endpoints (same SDK key).
- Server resolves environment from key on every request.

## Non-Functional Requirements

1. **Performance**: Rule-based segments evaluated in-process (sub-ms). List segment
   LFU hits are also in-process. List API calls add ~1 network round-trip.
2. **Thread Safety**: `SdkClient` is `Send + Sync`.
3. **Graceful Degradation**: Definition cache serves stale data on poll failure.
   List-check API failures surface as `Result::Err` from `evaluate()`.
4. **Ergonomics**: `init()` + `evaluate()` is the primary API surface.

## Acceptance Criteria

1. `SdkClient::init()` blocks until first sync; errors on invalid SDK key.
2. `evaluate()` matches server-side evaluation for both rule-based and list-based segments.
3. Rule-based segment evaluation makes no network calls.
4. List-based segment (no LFU): exactly one API call to `/list-check` per evaluation.
5. List-based segment (LFU, warm): no API call — served from cache.
6. LFU proactive refresh: after `poll_interval`, memberships for hot contexts are
   updated without waiting for an `evaluate()` call.
7. Server down after init: rule-based evaluation continues; list-based returns error.

## Out of Scope

- Client-side SDKs (Android/iOS/React/Web).
- Server-Side Events / streaming — deferred.
- Event submission — deferred to Experimentation track.
- Persistent (disk-backed) cache.
- List-based segment entries synced to SDK (prohibitive for large lists).
