# stitchd-sdk (Rust)

Server-side Rust SDK for Stitchd Feature Flag.

This crate is the reference implementation of the language-agnostic SDK
contract specified under [`sdks/spec/`](../spec/). Other language SDKs
(JavaScript, Python, Go, …) live under sibling directories
(`sdks/js/`, `sdks/python/`, etc.) and conform to the same spec.

## Status

Currently scaffolded but largely empty. The full public API lands across
[`sdk_rewrite_20260516` Phase 5](../../conductor/tracks/sdk_rewrite_20260516/plan.md).
This README will grow into a real quickstart + reference once the API is built.

## Behavioural Contract

For the canonical algorithm, caching rules, polling lifecycle, and event
delivery semantics, read:

- [`sdks/spec/docs/01-overview.md`](../spec/docs/01-overview.md)
- [`sdks/spec/docs/02-evaluation-semantics.md`](../spec/docs/02-evaluation-semantics.md)
- [`sdks/spec/docs/03-caching.md`](../spec/docs/03-caching.md)
- [`sdks/spec/docs/04-polling.md`](../spec/docs/04-polling.md)
- [`sdks/spec/docs/05-events.md`](../spec/docs/05-events.md)
- [`sdks/spec/docs/06-errors.md`](../spec/docs/06-errors.md)

## Conformance

The conformance test runner consumes
[`sdks/spec/fixtures/`](../spec/fixtures/). To run (once implemented in
Phase 6):

```bash
cargo test -p stitchd-sdk --test conformance
```

## Internal Implementation Notes

- Definition snapshot held in `ArcSwap<DefinitionSnapshot>` for lock-free reads
- List-segment membership LRU via `moka::sync::Cache`
- Three background tasks: definition poll, LRU refresh, event flush (all
  poll-based; no streaming in this revision)
- gRPC client (definition sync) via `tonic`; REST client (batch + events) via `reqwest`
