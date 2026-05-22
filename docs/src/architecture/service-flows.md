# Service Coordination Flows

This page collects sequence diagrams for the most common cross-service
request paths. All inter-service calls use gRPC over a tonic 0.14 channel
held in `GatewayState`. The gateway is the **single trust boundary** — it
authenticates the caller, then forwards the resolved `environment_id` (and
for JWT routes, the `RbacContext`) to backend services as gRPC metadata.

Service port reference:

| Service | gRPC port | Default address env var |
|---|---|---|
| `stitchd-gateway` (SDK gRPC) | `:50050` | — (in-process) |
| `stitchd-auth-service` | `:50051` | `STITCHD_AUTH_SERVICE_ADDR` |
| `stitchd-flag-service` | `:50052` | `STITCHD_FLAG_SERVICE_ADDR` |
| `stitchd-segmentation-service` | `:50053` | `STITCHD_SEGMENTATION_SERVICE_ADDR` |
| `stitchd-analytics-service` | `:50054` | `STITCHD_ANALYTICS_SERVICE_ADDR` |
| `stitchd-experimentation-service` | `:50055` | `STITCHD_EXPERIMENTATION_SERVICE_ADDR` |
| `stitchd-stats-service` | `:50056` | `STITCHD_STATS_SERVICE_ADDR` |

## SDK Definition Sync

An SDK polls the gateway every `definition_poll_interval` (default 30s)
for the full flag + segment + event-definition snapshot. The first sync
happens synchronously inside `SdkClient::init` and the background task
takes over from there.

```mermaid
sequenceDiagram
    participant SDK as stitchd-sdk-rust
    participant GW as stitchd-gateway<br/>:50050 (SDK gRPC)
    participant AUTH as auth-service<br/>:50051
    participant FLAG as flag-service<br/>:50052
    participant PG as PostgreSQL

    SDK->>GW: SdkService.SyncDefinitions{}<br/>metadata: x-sdk-key
    GW->>AUTH: AuthService.ValidateCredential(sdk_key)
    Note over AUTH: SdkKeyCache (moka, 60s TTL)<br/>hash → SdkKey lookup
    AUTH->>PG: SELECT … FROM sdk_keys WHERE key_hash = $1 AND is_active
    PG-->>AUTH: row
    AUTH-->>GW: RbacContext { environment_id, organisation_id, subject=sdk_key_id }

    GW->>FLAG: FlagSdkBackendService.SyncDefinitions{}<br/>metadata: x-env-id
    FLAG->>PG: list_flags(env_id) + list_segments(env_id) + list_event_defs(env_id)
    PG-->>FLAG: rows
    FLAG-->>GW: SyncDefinitionsResponse {<br/>  flags, rule_segments,<br/>  list_segments (meta only),<br/>  event_definitions,<br/>  environment_id, server_timestamp_ms<br/>}
    GW-->>SDK: SyncDefinitionsResponse (passed through verbatim)
```

The gateway's SDK gRPC port (`:50050`) hosts `SdkService` —
`build_grpc_server` in `crates/stitchd-gateway/src/grpc_server.rs` wires
the two `SdkService` RPCs (`SyncDefinitions`, `IngestSdkEvalLog`) to
`FlagSdkBackendService` on the flag-service. SDKs never see backend
services directly.

## Per-Eval List-Segment Membership

When a flag evaluation references a list-based segment and the LRU
doesn't have the relevant entry, the SDK issues a batched REST call to
resolve membership:

```mermaid
sequenceDiagram
    participant SDK as stitchd-sdk-rust
    participant GW as stitchd-gateway<br/>:8080 (REST)
    participant AUTH as auth-service
    participant SEG as segmentation-service<br/>:50053
    participant SCY as ScyllaDB

    SDK->>GW: POST /v1/sdk/segments/list:batch<br/>x-sdk-key, { queries: [...] }
    GW->>GW: sdk_auth_middleware<br/>(SdkKeyCache hit → SdkContext)
    GW->>SEG: SegmentationSdkBackendService<br/>.BatchCheckListMembership<br/>x-env-id metadata
    loop one CQL query per partition tuple
        SEG->>SCY: SELECT … FROM segment_list_entries<br/>WHERE (segment_id, context_type, generation, entry_key) = (?,?,?,?)
        Note over SEG,SCY: active generation read first from<br/>segment_list_generations
    end
    SCY-->>SEG: rows
    SEG-->>GW: BatchCheckListMembershipResponse<br/>(one MembershipResult per query)
    GW-->>SDK: 200 { results: [...] }
```

The response is keyed the same way as the request — N queries in, N
results out, same order. The SDK populates its LRU and the next
evaluation against any segment in the response is a cache hit.

## SDK Event Flush — Evaluation Events

The SDK emits one `FlagEvaluationEvent` per `evaluate()` call into a
bounded queue. The background `FlushTask` drains it every
`event_flush_interval` (default 5s) or when `event_batch_size` (default
100) events accumulate.

```mermaid
sequenceDiagram
    participant SDK as stitchd-sdk-rust
    participant GW as stitchd-gateway<br/>:8080
    participant FLAG as flag-service<br/>:50052
    participant CH as ClickHouse<br/>flag_evaluation_log

    SDK->>GW: POST /v1/sdk/events:batch<br/>x-sdk-key, { events: [...] }
    GW->>GW: sdk_auth_middleware → SdkContext
    GW->>GW: stamp environment_id on every event<br/>(SDK-supplied env_id is ignored)
    GW->>FLAG: FlagSdkBackendService.IngestSdkEvalLog<br/>x-env-id metadata
    FLAG->>FLAG: convert SdkEvalBatch → Vec<EvalLogRow>
    FLAG->>FLAG: eval_log_writer::EvalLogSender (MPSC)
    FLAG-)CH: batched INSERT (async)
    FLAG-->>GW: IngestSdkEvalLogResponse {}
    GW-->>SDK: 202 Accepted
```

Each `FlagEvaluationEvent` becomes one row in `flag_evaluation_log` with
`targeting_on`, `matched_rule_id`, `variant_key`, `context_type`,
`context_key`, `evaluated_at`. This is the substrate for the
[experiment attribution pipeline](../experimentation/attribution.md) —
the `experiment_assignments_mv` watches inserts here and routes them to
`experiment_assignments`.

## SDK Track Event Flush — Metric Events

Distinct from evaluation events: `client.track(event_key, ...)` enqueues
on the `EventBuffer` and flushes to a different endpoint that goes
through analytics-service into `events_v2`.

```mermaid
sequenceDiagram
    participant App
    participant SDK as stitchd-sdk-rust<br/>EventBuffer
    participant GW as stitchd-gateway<br/>:8080
    participant ANL as analytics-service<br/>:50054
    participant CH as ClickHouse<br/>events_v2

    App->>SDK: client.track("purchase", {user: alice, ...}, 49.95)
    SDK->>SDK: validate against cached event_definitions
    SDK->>SDK: enqueue (flushes on size/interval)

    SDK->>GW: POST /v1/events/track<br/>x-sdk-key, { events: [...] }
    GW->>GW: sdk_auth_middleware → SdkContext
    GW->>GW: event_quota_middleware (per-env token bucket)
    GW->>ANL: AnalyticsService.TrackEvents<br/>x-env-id metadata
    ANL->>ANL: EventDefinitionCache (60s TTL)<br/>per-event validate
    ANL-)CH: INSERT INTO events_v2 (batched)
    ANL-->>GW: TrackEventsResponse { accepted_count, rejected[] }
    GW-->>SDK: 202 Accepted
```

`event_quota_middleware` enforces a per-env-id token bucket (default 1000
events/sec, configurable via `STITCHD_EVENT_QUOTA_PER_SEC`). The 5 MiB
body limit is applied only to this route via a per-route `DefaultBodyLimit`
layer.

## Server-Side Flag Evaluate Preview

Used by the admin UI's "Test" panel. Runs the same rule engine as the
SDK, but on `flag-service` against a user-supplied mock context, and
returns a full rule trace.

```mermaid
sequenceDiagram
    participant UI as Admin UI
    participant GW as stitchd-gateway<br/>:8080
    participant AUTH as auth-service
    participant FLAG as flag-service

    UI->>GW: POST /v1/projects/{p}/flags/{f}/evaluate-preview<br/>Authorization: Bearer …
    GW->>GW: auth_middleware (JWT verify)
    GW->>AUTH: ValidateCredential(bearer_token)
    AUTH-->>GW: RbacContext { user_id, org_id, roles, permissions }
    GW->>GW: require_permission("flag:read", project_scope)

    GW->>FLAG: FlagService.EvaluatePreview(<br/>  flag_key, mock_context, env_id)
    FLAG->>FLAG: stitchd_core::rules::evaluate(<br/>  flag, context, in-memory cache)
    Note over FLAG: NO ClickHouse write —<br/>preview is read-only
    FLAG-->>GW: EvaluatePreviewResponse {<br/>  variant_key, rule_trace, rollout_debug }
    GW-->>UI: 200 { variant_key, rule_trace, rollout_debug }
```

The preview never writes to the eval log — admin curiosity must not
contaminate experiment exposures.

## Human Login + Token Refresh

```mermaid
sequenceDiagram
    participant UI as Admin UI
    participant GW as stitchd-gateway<br/>:8080
    participant AUTH as auth-service<br/>:50051
    participant PG as PostgreSQL

    UI->>GW: POST /v1/auth/login<br/>{ email, password, org_id? }
    GW->>AUTH: AuthService.LoginWithPassword(email, password, org_id)
    AUTH->>PG: SELECT FROM users WHERE email = $1
    PG-->>AUTH: row (password_hash + token_secret + totp_enabled)
    AUTH->>AUTH: argon2id verify_password
    alt MFA enabled
        AUTH-->>GW: 401 mfa_required (challenge_token)
        GW-->>UI: 401 { mfa_required, challenge_token }
        Note over UI: User submits TOTP code …
    else MFA not enabled / completed
        AUTH->>AUTH: issue access_token (JWT HS256) + refresh_token
        AUTH->>PG: INSERT refresh_tokens (token_hash, user_id, org_id, expires_at)
        AUTH-->>GW: LoginResponse { access_token, refresh_token, user_id, org_id }
        GW-->>UI: 200 { access_token, refresh_token, user_id, org_id, expires_in }
    end

    Note over UI: Subsequent management request
    UI->>GW: GET /v1/projects/{p}/flags<br/>Authorization: Bearer eyJhbGci…
    GW->>AUTH: ValidateCredential(bearer_token)
    AUTH-->>GW: RbacContext { subject=user_id, org_id, roles, permissions }
    GW->>GW: require_permission("flag:read", project_scope)
    GW->>FLAG: FlagService.ListFlags(project_id)
    FLAG-->>GW: ListFlagsResponse
    GW-->>UI: 200 { items, total, page, per_page }

    Note over UI: 5 min later — access token nearing expiry
    UI->>GW: POST /v1/auth/refresh { refresh_token }
    GW->>AUTH: RefreshToken(refresh_token)
    AUTH->>PG: SELECT FROM refresh_tokens WHERE token_hash = $1
    AUTH->>PG: revoke old (rotation) + INSERT new refresh_token row
    AUTH-->>GW: RefreshTokenResponse { access_token, refresh_token, expires_in }
    GW-->>UI: 200 { access_token, refresh_token, expires_in }
```

JWT validation flows through `auth_middleware` on every authenticated
request. The `RbacContext` injected into request extensions carries
`is_system` so the gateway's `require_system_org` / `require_non_system_org`
guards can short-circuit unauthorised routes before any backend gRPC call.

## OIDC / SAML Login

The `/v1/auth/oidc/*` and `/v1/auth/saml/*` route trees follow the
standard browser-mediated redirect dance. Both terminate in the same
JWT issuance that password login uses, so post-login behaviour is
identical.

```mermaid
sequenceDiagram
    participant UI as Admin UI
    participant GW as stitchd-gateway
    participant AUTH as auth-service<br/>(OidcLoginService)
    participant IDP as External IdP

    UI->>GW: POST /v1/orgs/{org}/auth/oidc/authorize
    GW->>AUTH: OidcAuthorize(org_id, redirect_uri)
    Note over AUTH: build PKCE challenge,<br/>store in OidcStateStore (300s TTL)
    AUTH-->>GW: { authorize_url, state }
    GW-->>UI: 200 { authorize_url, state }
    UI->>IDP: redirect to authorize_url

    Note over IDP: user authenticates
    IDP->>GW: GET /v1/auth/oidc/{provider_id}/callback?code=…&state=…
    GW->>AUTH: OidcCallback(code, state)
    AUTH->>IDP: exchange code → id_token (PKCE-protected)
    IDP-->>AUTH: id_token (signed JWT)
    AUTH->>AUTH: verify signature; extract claims
    AUTH->>AUTH: upsert user; assign org_membership
    AUTH->>AUTH: issue access_token + refresh_token
    AUTH-->>GW: LoginResponse
    GW-->>UI: redirect with tokens (URL-encoded fragment)
```

SAML follows the same pattern with `SamlLoginService` and `SamlRelayStore`
(600s TTL): `POST .../sso` returns a SAML AuthnRequest XML payload to
post to the IdP, the IdP POSTs the assertion back to the ACS at
`POST /v1/auth/saml/{provider_id}/callback`, and `LiveSamlExchanger`
validates the assertion (`quick-xml` + `flate2`) before issuing JWTs.

## Experiment Compute (Scheduled & On-Demand)

`stitchd-stats-service` runs two loops:

```mermaid
sequenceDiagram
    participant ST as stats-service<br/>scheduler<br/>(60-min tokio::time::interval)
    participant EXP as experimentation-service<br/>:50055
    participant ANL as analytics-service<br/>:50054
    participant CH as ClickHouse
    participant PG as PostgreSQL

    Note over ST: scheduler_interval ticker fires
    ST->>EXP: ExperimentationService.ListRunningExperiments<br/>(server-streaming)
    EXP->>PG: SELECT FROM experiments WHERE status IN ('running','paused')
    EXP-->>ST: stream<RunningExperiment>
    loop per experiment
        ST->>ST: dispatch_metric_query(<br/>  metric_def, exp_id, iter_id, env_id, …)
        ST->>CH: SELECT … FROM events_v2 JOIN experiment_assignments
        CH-->>ST: per-context-type per-variant rows
        ST->>ST: Frequentist + Bayesian + CUPED + SRM + Guardrails
        ST->>ANL: AnalyticsService.WriteExperimentResults<br/>(repeated ExperimentResult)
        ANL->>CH: INSERT INTO experiment_results
        ANL-->>ST: ok
        ST->>PG: UPDATE stats_schedule.last_computed_at
    end
```

The on-demand path is reachable via `POST /v1/.../experiments/{id}/recompute`:

```mermaid
sequenceDiagram
    participant UI as Admin UI
    participant GW as stitchd-gateway
    participant ST as stats-service<br/>StatsService.TriggerRecompute

    UI->>GW: POST /v1/environments/{env}/experiments/{id}/recompute
    GW->>ST: StatsService.TriggerRecompute(experiment_id)
    ST->>ST: spawn job (tokio::spawn)<br/>insert stats_jobs row, status=pending
    ST-->>GW: TriggerRecomputeResponse { job_id, status: "pending" }
    GW-->>UI: 202 { job_id }

    Note over UI: poll for status
    UI->>GW: GET /v1/environments/{env}/experiments/{id}/recompute/{job_id}
    GW->>ST: StatsService.GetJobStatus(job_id)
    ST-->>GW: GetJobStatusResponse { status, started_at_ms, completed_at_ms, error }
    GW-->>UI: 200 { ... }
```

`TriggerRecompute` is also called fire-and-forget by analytics-service
when a metric or event definition changes — see [Metrics § Event-Driven
Recompute](./metrics.md#event-driven-recompute).

## Iteration Start / Stop (CH Dictionary Reload)

When an experiment iteration starts or stops, the
`experiment_iterations_active` ClickHouse dictionary must reflect the
change quickly enough for `experiment_assignments_mv` to route new evals
correctly. The natural dictionary TTL is 30–60s; the experimentation
service additionally fires an explicit reload:

```mermaid
sequenceDiagram
    participant UI as Admin UI
    participant GW as stitchd-gateway
    participant EXP as experimentation-service
    participant PG as PostgreSQL
    participant CH as ClickHouse

    UI->>GW: POST /v1/environments/{env}/experiments/{id}/transitions { to: "running" }
    GW->>EXP: ExperimentationService.TransitionExperiment(...)
    EXP->>PG: UPDATE experiments SET status='running' WHERE id = $1
    EXP->>PG: UPDATE experiment_iterations SET started_at = now() WHERE …
    PG-->>EXP: ok

    Note over EXP: fire-and-forget — TTL caps staleness if this fails
    EXP-)CH: SYSTEM RELOAD DICTIONARY experiment_iterations_active
    EXP-->>GW: Experiment (updated)
    GW-->>UI: 200 (updated experiment)

    Note over CH: subsequent flag_evaluation_log inserts<br/>flow through experiment_assignments_mv<br/>using the refreshed dictionary
```
