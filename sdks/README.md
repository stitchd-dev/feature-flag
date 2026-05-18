# Stitchd SDKs

Home for all Stitchd Feature Flag SDK implementations.

## Layout

```
sdks/
├── spec/          Language-agnostic contract that all SDKs must conform to.
│   ├── docs/      Markdown behavioral spec (eval semantics, caching, polling, events, errors)
│   ├── proto/     Protobuf .proto files (SDK ↔ gateway gRPC surface)
│   ├── openapi/   OpenAPI 3.1 YAML (SDK ↔ gateway REST surface)
│   ├── schemas/   JSON Schema definitions for events, eval req/resp, config
│   └── fixtures/  Conformance test vectors (input → expected output) every SDK must pass
│
└── rust/          Server-side Rust SDK (crate name: stitchd-sdk-rust)
    ├── src/
    ├── tests/
    └── Cargo.toml
```

## Trust Model

All SDK ↔ Backend traffic flows **exclusively** through `stitchd-gateway`.
The gateway is the sole trust boundary: it validates the SDK key once, resolves
`(env_id, project_id, org_id)`, and forwards downstream with that context in
gRPC metadata / REST extensions. Backend microservices trust the gateway-supplied
context and do not re-validate the SDK key.

## Adding a New Language SDK

1. Read everything under `sdks/spec/docs/` — that is the behavioral contract.
2. Generate or hand-write client code from `sdks/spec/proto/` (gRPC) and `sdks/spec/openapi/` (REST).
3. Use the type definitions from `sdks/spec/schemas/` as your wire-level types.
4. Implement a conformance test runner that consumes `sdks/spec/fixtures/` and asserts the SDK matches every expected output.
5. Place the SDK under `sdks/<lang>/`.
