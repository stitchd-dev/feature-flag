# Rust SDK

The Stitchd Rust SDK (`stitchd-sdk-rust`) embeds **server-side, in-process
flag evaluation** into your backend service. It pulls flag and segment
definitions from `stitchd-gateway` over gRPC, evaluates flags locally
against your own evaluation context, and ships evaluation events back to
the gateway in fire-and-forget batches. There is no per-evaluation
network hop — once the SDK has bootstrapped, `evaluate(...)` is a hot
in-process call.

## What it does

- **In-process evaluation** — the rule engine (`stitchd-core::rule_engine`)
  runs against a local snapshot of flag definitions. No outbound call on
  the evaluate hot path.
- **gRPC definition sync** — a background task calls
  `SdkService::SyncDefinitions` every `definition_poll_interval` (default
  30 s) and atomically swaps the local snapshot on each successful poll.
- **List-segment membership cache** — list-based segments resolve through
  a bounded LRU keyed by `(context_type, context_key)`. On miss the SDK
  calls `POST /v1/sdk/segments/list:batch` synchronously; subsequent hits
  are zero-network. A second background task proactively refreshes
  resident entries every `list_segment_refresh_interval` (default 60 s).
- **Asynchronous event delivery** — every evaluation enqueues a
  `FlagEvaluationEvent` onto a bounded queue. A background flush task
  drains it to `POST /v1/sdk/events:batch` every `event_flush_interval`
  (default 5 s) or when the batch hits `event_batch_size` (default 100).
  Queue overflow drops the oldest event — `evaluate()` never blocks.
- **Track events** — `SdkClient::track(...)` enqueues caller-supplied
  business events onto a separate retrying buffer that ships to
  `POST /v1/events/track`. Unknown / mismatched event keys are skipped
  with a warning.
- **Authentication** — every request carries the `x-sdk-key` header.
  The key is environment-scoped; rotate it through the admin UI.

## Install

The SDK is currently consumed as a path dependency in this workspace.
External crate publishing is on the roadmap; until then point Cargo at
the source tree.

```toml
[dependencies]
stitchd-sdk-rust = { path = "sdks/rust" }
stitchd-core     = { path = "crates/stitchd-core" } # for Context / ParameterValue
tokio            = { version = "1", features = ["full"] }
```

The minimum required Rust version is set in the workspace
`Cargo.toml::workspace.package.rust-version`. The SDK has no
`default-features` toggles — the `test-util` feature exposes an
in-memory `SdkClient` constructor used by conformance and integration
tests.

## Quickstart

The canonical runnable example lives in the crate's module-level
rustdoc, and is auto-extracted into [`./quickstart.md`](./quickstart.md)
by `cargo xtask docs` so the Markdown copy never drifts from compiled
source. Read `./quickstart.md` for the latest snippet, or jump straight
to [`sdks/rust/examples/live_verify.rs`](https://github.com/) for an
end-to-end runnable example that talks to a real gateway.

A condensed view:

```rust,ignore
use stitchd_sdk_rust::{EvalRequest, SdkClient, SdkConfig};
use stitchd_core::context::Context;

let config = SdkConfig::new("http://localhost:8081", std::env::var("STITCHD_SDK_KEY")?);
let client = SdkClient::init(config).await?;

let results = client
    .evaluate(&[EvalRequest {
        flag_key: "checkout-flow".to_string(),
        context: Context::new("user", "alice"),
    }])
    .await;

println!("variant = {}", results[0].variant_key);
client.shutdown(std::time::Duration::from_secs(5)).await?;
```

Each `EvalResult` carries `flag_key`, `variant_key`, `variant_value`
(`serde_json::Value`), and an `outcome` discriminating
`Matched { rule_index }`, `DefaultRule`, `Disabled`, and `FlagNotFound`.
Use `evaluate_with_reasoning(...)` to additionally receive the matched
rule index + name for debugging or audit.

## API reference

Generated rustdoc is published alongside this book at
[`../rustdoc/index.html`](../rustdoc/index.html).

The public surface is intentionally small:

| Item | Purpose |
|---|---|
| `SdkClient` | The entry point. `init` / `evaluate` / `evaluate_with_reasoning` / `track` / `flush` / `shutdown`. |
| `SdkConfig` | Constructed via `SdkConfig::new(gateway_url, sdk_key)`. All other fields default per the spec. |
| `EvalRequest` / `EvalResult` / `EvalOutcome` | Request/response types for `evaluate`. |
| `EvalResultWithReasoning` / `ReasoningTrace` | Returned by `evaluate_with_reasoning`. |
| `SdkError` / `TrackError` | Error taxonomy mirroring `sdks/spec/docs/06-errors.md`. |
| `BufferedEvent` / `TypedValue` / `FlushReport` | Track-event buffer types used by `track` + `flush`. |

Lower-level building blocks (`DefinitionSnapshot`, `MembershipCache`,
`EventQueue`, `PollTask`, `RefreshTask`, `FlushTask`, etc.) are exported
for advanced integration / testing but are **not** the recommended way
to consume the SDK — prefer `SdkClient`.

## Spec compliance

This crate is the reference implementation of the language-agnostic SDK
contract in [`sdks/spec/`](https://github.com/). That directory is the
single source of truth for evaluation semantics, caching rules, polling
lifecycle, event delivery, and error policy — every Stitchd SDK
implementation (Rust, JavaScript, Python, Go, …) must conform to it.
When adding a new language SDK, you do **not** invent new behaviour —
you implement what is specified there.

Behavioural conformance is enforced by shared test fixtures under
`sdks/spec/fixtures/evaluation/`; the Rust SDK runs them through the
`conformance` integration test (enabled by the `test-util` feature):

```bash
cargo test --features test-util -p stitchd-sdk-rust --test conformance
```

## Further reading

- [Quickstart](./quickstart.md) — runnable snippet, regenerated from
  `sdks/rust/src/lib.rs` by `cargo xtask docs`.
- [Spec — overview](../../../sdks/spec/docs/01-overview.md)
- [Spec — evaluation semantics](../../../sdks/spec/docs/02-evaluation-semantics.md)
- [Spec — caching](../../../sdks/spec/docs/03-caching.md)
- [Spec — polling lifecycle](../../../sdks/spec/docs/04-polling.md)
- [Spec — events](../../../sdks/spec/docs/05-events.md)
- [Spec — errors & retry policy](../../../sdks/spec/docs/06-errors.md)
