# SDK Spec — Language-Agnostic Contract

This directory is the **single source of truth** for the SDK ↔ gateway contract.
Every Stitchd SDK implementation (Rust, JavaScript, Python, Go, …) must conform
to everything here. When you add a new language SDK, you do not invent new
behaviour — you implement what is specified here.

## Contents

| Path | Purpose | Authoritative for |
|---|---|---|
| `docs/` | Markdown behavioral spec | Eval semantics, caching rules, polling lifecycle, event delivery, error/retry policy |
| `proto/` | Protobuf `.proto` files | gRPC wire format (currently: `SdkService.SyncDefinitions`, `IngestSdkEvalLog`; `SegmentationSdkService.BatchCheckListMembership`) |
| `openapi/` | OpenAPI 3.1 YAML | REST wire format (`POST /v1/sdk/segments/list:batch`, `POST /v1/sdk/events:batch`) |
| `schemas/` | JSON Schema | Cross-language type definitions for events, eval req/resp, reasoning trace, config |
| `fixtures/` | Test vectors | Behavioral conformance — every SDK must produce the expected output for each input scenario |

## Versioning

This contract is currently `v1` (implied — not yet explicitly stamped). When
breaking changes become necessary, introduce a new versioned directory
(`proto/v2/`, etc.) and run both contracts in parallel during migration.

## Conformance

An implementation is "spec-compliant" when:

1. Its gRPC client compiles against `proto/` without modification.
2. Its REST client matches the `openapi/` operation IDs, paths, request bodies, and response shapes.
3. Its event-emission produces JSON conforming to `schemas/flag_evaluation_event.schema.json`.
4. Its configuration accepts the fields described in `schemas/sdk_config.schema.json`.
5. Its evaluation engine passes 100% of fixtures in `fixtures/`.

## Out of Scope (Yet)

- Server-streaming definition sync (SSE / WebSocket / gRPC server-stream) — all
  sync is currently poll-based via unary RPCs.
- Client-side SDKs (browser / mobile) — a separate trust model applies; not
  covered by this spec.
