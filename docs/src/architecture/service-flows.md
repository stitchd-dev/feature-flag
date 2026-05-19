# Service Coordination Flows

Key request flows across the microservice mesh. All inter-service calls use gRPC.

## Flag Evaluation

An SDK client evaluates a feature flag. The gateway authenticates the SDK key, then asks the flag service to evaluate the flag for the given context. The flag service may call the segmentation service to resolve rule-based targeting.

```mermaid
sequenceDiagram
    participant SDK as SDK Client
    participant GW as stitchd-gateway<br/>(REST :8080)
    participant FS as stitchd-flag-service<br/>(gRPC :50051)
    participant SS as stitchd-segmentation-service<br/>(gRPC :50053)

    SDK->>GW: POST /v1/environments/{environment_id}/evaluate<br/>x-sdk-key: sdk_live_...
    GW->>FS: ValidateSdkKey(environment_id, sdk_key)
    FS-->>GW: Ok(environment)

    GW->>FS: EvaluateFlag(flag_key, context_type, context_key, attributes)

    alt flag has segment rule
        FS->>SS: CheckMembership(segment_key, context_type, context_key)
        SS-->>FS: MembershipResponse(is_member)
    end

    FS-->>GW: EvaluateResponse(variant_key, is_enabled)
    GW-->>SDK: 200 { variant_key, is_enabled }
```

## Event Ingestion

An SDK client records a metric event. The gateway authenticates the SDK key, then forwards the event to the analytics service for storage and downstream processing.

```mermaid
sequenceDiagram
    participant SDK as SDK Client
    participant GW as stitchd-gateway<br/>(REST :8080)
    participant FS as stitchd-flag-service<br/>(gRPC :50051)
    participant ANS as stitchd-analytics-service<br/>(gRPC :50053)

    SDK->>GW: POST /v1/environments/{environment_id}/events<br/>x-sdk-key: sdk_live_...
    GW->>FS: ValidateSdkKey(environment_id, sdk_key)
    FS-->>GW: Ok(environment)

    GW->>ANS: IngestEvent(events: [{ metric_key, context_type, context_key, value }])
    ANS-->>GW: IngestResponse(accepted_count, rejected_keys)

    GW-->>SDK: 200 { accepted_count, rejected_keys }
```

## Definition Sync

An SDK client opens a long-lived gRPC streaming connection to receive the full flag/segment definition set and incremental updates. The gateway passes the stream through to the flag service.

```mermaid
sequenceDiagram
    participant SDK as SDK Client
    participant GW as stitchd-gateway<br/>(gRPC :50050)
    participant FS as stitchd-flag-service<br/>(gRPC :50051)

    SDK->>GW: FlagSyncService.SyncDefinitions(SyncRequest)<br/>metadata: x-sdk-key: sdk_live_...
    GW->>FS: ValidateSdkKey(environment_id, sdk_key)
    FS-->>GW: Ok(environment)

    GW->>FS: FlagSyncService.SyncDefinitions(SyncRequest)

    Note over FS: Stream open — full snapshot first
    FS-->>GW: SyncResponse(flags[], segments[], sequence_number=1)
    GW-->>SDK: SyncResponse(flags[], segments[], sequence_number=1)

    Note over FS: Incremental update on mutation
    FS-->>GW: SyncResponse(flags[updated], segments[], sequence_number=2)
    GW-->>SDK: SyncResponse(flags[updated], segments[], sequence_number=2)

    Note over SDK: Connection held open indefinitely
```

## Human Auth

An admin user or the Admin UI logs in and obtains a JWT. Subsequent management requests carry the JWT which the gateway validates before proxying.

```mermaid
sequenceDiagram
    participant UI as Admin UI / Operator
    participant GW as stitchd-gateway<br/>(REST :8080)
    participant AS as stitchd-auth-service<br/>(gRPC :50052)

    UI->>GW: POST /v1/auth/login<br/>{ email, password }
    GW->>AS: Login(email, password)
    AS-->>GW: LoginResponse(token, expires_at)
    GW-->>UI: 200 { token, expires_at }

    Note over UI: Subsequent management request
    UI->>GW: GET /v1/environments/{env}/flags<br/>Authorization: Bearer eyJhbGci...
    GW->>AS: ValidateToken(token)
    AS-->>GW: TokenClaims(user_id, org_id, roles)

    GW->>GW: Authorise: roles include required permission?

    alt authorised
        GW->>GW: Proxy to stitchd-flag-service
        GW-->>UI: 200 { flags: [...] }
    else forbidden
        GW-->>UI: 403 { error: "insufficient permissions" }
    end
```
