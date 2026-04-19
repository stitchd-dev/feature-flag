# Track Learnings: mdbook_docs_20260418

Patterns, gotchas, and context discovered during implementation.

## Codebase Patterns (Inherited)

- **Vendored protoc:** `protoc-bin-vendored` is already a build dependency in `stitchd-proto`. The xtask can reuse this to invoke `protoc-gen-doc` without requiring a system install.
- **Axum 0.8 Routing:** Use `{param}` syntax for path captures. New OpenAPI routes should follow this pattern.
- **Workspace resolver:** `resolver = "3"` — new `xtask` crate must not override this.

---

<!-- Learnings from implementation will be appended below -->

## [2026-04-18 00:00] - Phase 2 Task 2: Expose /api-docs/openapi.json endpoint

- **Implemented:** Created `StitchdApiDoc` struct in `api/openapi.rs` using `#[derive(utoipa::OpenApi)]` aggregating all paths and schemas; wired `GET /api-docs/openapi.json` into Axum router.
- **Files changed:** `crates/stitchd-server/src/api/openapi.rs` (new), `src/api/mod.rs`, `src/lib.rs`, `crates/stitchd-core/src/rule_engine/types.rs`
- **Commit:** d16db8d
- **Learnings:**
  - Patterns: utoipa OpenApi aggregator belongs in `api/openapi.rs`; expose raw JSON via `utoipa::OpenApi::openapi()` — no extra dep needed
  - Gotchas: `ConditionExpr` is self-referential (`Vec<ConditionExpr>`, `Box<ConditionExpr>`) — causes stack overflow in utoipa schema generation unless `#[schema(no_recursion)]` is added to the type in stitchd-core
  - Context: The openapi.json handler does NOT need `AppState` — it's fully static and serves the same response every time
---
