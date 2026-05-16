# 01 — Overview

This document and the rest of `sdks/spec/docs/` describe SDK **behaviour** —
what every SDK must do, regardless of implementation language. Nothing here is
specific to Rust, JavaScript, Python, Go, or any other language. When in doubt,
prefer abstract terms (e.g. "concurrent-safe handle") over language-specific
constructs (e.g. "`Arc<T>`", "`Promise<T>`").

## What an SDK Does

A Stitchd SDK is a library that an application embeds to evaluate feature flags
**in-process**. It does not call out to a remote evaluation service per flag —
it pulls flag definitions and segment definitions from the Stitchd gateway and
applies them locally to a `Context` provided by the application.

The SDK is responsible for:

1. **Definition sync** — keeping a local snapshot of all flag definitions,
   rule-based segment definitions, and list-based segment metadata current via
   periodic polling.
2. **List-segment membership cache** — maintaining a bounded LRU of
   `(context_type, key) → {segment_id: bool}` to avoid a network round-trip for
   every list-segment check.
3. **Evaluation** — applying the canonical rule engine (specified in
   `02-evaluation-semantics.md`) to a flag definition plus caller-supplied
   context and returning a variant.
4. **Event emission** — publishing one `FlagEvaluationEvent` per evaluated flag
   for downstream analytics (delivered via batched flush; see `05-events.md`).

## Gateway is the Trust Boundary

All SDK ↔ Backend traffic flows **exclusively** through `stitchd-gateway`.

- The SDK presents its `x-sdk-key` HTTP header (or gRPC metadata key with the
  same name) on every request.
- The gateway validates the key, resolves
  `(environment_id, project_id, organization_id)`, and injects this resolved
  context into requests forwarded downstream.
- Backend microservices trust the gateway-supplied environment id and
  **never re-validate** the SDK key.
- A revoked SDK key is rejected at the gateway on the next request after its
  in-memory cache expiry (default 60s). The SDK has no way to learn of
  revocation other than receiving a `401 Unauthorized` (REST) or
  `Unauthenticated` (gRPC) response — see `06-errors.md` for how to react.

## Wire Protocols

| Concern | Protocol | Endpoint |
|---|---|---|
| Definition sync (polling) | gRPC (unary) | gateway `<grpc_port>` / `sdk.v1.SdkService/SyncDefinitions` |
| Batch list-segment membership | REST | `POST /v1/sdk/segments/list:batch` |
| On-demand single-context list-segment fetch | REST (same endpoint, 1-element batch) | `POST /v1/sdk/segments/list:batch` |
| Event ingestion | REST | `POST /v1/sdk/events:batch` |

Streaming (server-streaming gRPC, SSE, WebSocket push) is **out of scope** for
this revision. All sync is poll-based. Streaming is a planned future enhancement
and will be specified as a separate optional transport.

## Concurrency Model

A single SDK instance is intended to serve **many concurrent evaluation requests**
from the embedding application (web server, worker pool, batch job). Implementations:

- MUST provide a single instance that can be shared across application
  request handlers safely.
- MUST allow read access (`evaluate`, `evaluate_with_reasoning`) to proceed
  without blocking on background sync tasks. The definition snapshot SHOULD use
  a lock-free read pattern (e.g. atomic pointer swap, copy-on-write,
  read-write lock with infrequent writes).
- MUST NOT spawn one background task per `evaluate()` call. The polling, LRU
  refresh, and event-flush tasks are launched **once**, at `init` time, and
  drained at `shutdown` time.

## Lifecycle

```
                    init(config)
                        │
                        ▼
              ┌─── perform first definition sync (blocking) ──── if fail: init returns error
              │           │
              │           ▼
              │     spawn background tasks:
              │       1. definition poll loop
              │       2. lru refresh loop
              │       3. event flush loop
              │
              ▼
        evaluate() / evaluate_with_reasoning()      ◄── application uses the SDK
              │
              ▼
            shutdown()
              │
              ▼
       drain event buffer → stop background tasks → release resources
```

The first definition sync is **blocking**. `init` MUST NOT return successfully
until the snapshot contains real data. This guarantees that any subsequent
`evaluate()` call has flag definitions to work with — there is no "warming up"
window during which evaluations silently return defaults.

## Non-Goals

- The SDK is **server-side only**. Client-side (browser / mobile) SDKs use a
  different trust model and are not covered by this spec.
- The SDK does **not** mutate flag/segment definitions. It is read-only with
  respect to the configuration store.
- The SDK does **not** ingest user-defined application events (only its own
  per-evaluation `FlagEvaluationEvent`).
