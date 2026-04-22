# Flag Evaluation Flow

The SDK evaluates flags **in-process** after syncing definitions from the server via gRPC.
The only network calls after startup are for list-based segment membership checks.

## Startup Sync

```mermaid
sequenceDiagram
    participant App
    participant SDK as stitchd-sdk
    participant GW as stitchd-gateway<br/>gRPC :50050
    participant FS as stitchd-flag-service<br/>gRPC :50052
    participant PG as PostgreSQL

    App->>SDK: SdkClient::init(config)
    SDK->>GW: gRPC SyncDefinitions (stream)<br/>metadata: x-sdk-key: sdk_live_...
    GW->>FS: ValidateSdkKey + proxy SyncDefinitions
    FS->>PG: load flag + segment definitions
    PG-->>FS: definitions
    FS-->>GW: FlagDefinitions snapshot (stream)
    GW-->>SDK: FlagDefinitions snapshot
    SDK->>SDK: cache in DefinitionCache (Arc<RwLock>)
    SDK-->>App: Arc<SdkClient> ready
```

After `init`, the SDK holds a complete copy of all flag definitions in memory. The gRPC
stream remains open and `flag-service` pushes incremental updates whenever a flag or
segment changes — no polling interval.

## Per-Request Evaluation

```mermaid
sequenceDiagram
    participant App
    participant SDK as stitchd-sdk
    participant GW as stitchd-gateway<br/>REST :8080
    participant SS as stitchd-segmentation-service

    App->>SDK: evaluate("flag-key", &ctx)
    SDK->>SDK: read DefinitionCache (lock-free read)
    SDK->>SDK: find flag by key

    alt Flag disabled or no rules match
        SDK-->>App: default variant
    else Rule-based segment match
        SDK->>SDK: evaluate ConditionExpr in-process
        SDK-->>App: matched variant
    else List-based segment
        SDK->>SDK: check LFU cache
        alt Cache hit
            SDK-->>App: variant (cache)
        else Cache miss
            SDK->>GW: REST GET /v1/environments/{env}/list-segment-check
            GW->>SS: gRPC CheckMembership
            SS-->>GW: MembershipResponse
            GW-->>SDK: membership boolean
            SDK->>SDK: populate LFU cache
            SDK-->>App: variant
        end
    end
```

## Rule Engine

Rules are evaluated as a `ConditionExpr` tree:

- **`And(Vec<ConditionExpr>)`** — all children must match
- **`Or(Vec<ConditionExpr>)`** — any child must match
- **`Not(Box<ConditionExpr>)`** — negation
- **`Leaf { field, operator, value }`** — compare a context attribute

Context attributes are looked up from the `EvaluationContext` passed to `evaluate()`.
The context contains typed `ParameterValue` entries (string, int, float, bool, list)
keyed by `(context_type, attribute_name)`.

## LFU Cache

List-segment membership results are cached using an LFU (Least Frequently Used) eviction
policy. Pre-warming a set of high-traffic contexts at startup avoids REST round-trips for
known users. Cache size is configured in `LfuConfig`.
