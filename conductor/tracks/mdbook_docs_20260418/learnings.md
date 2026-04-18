# Track Learnings: mdbook_docs_20260418

Patterns, gotchas, and context discovered during implementation.

## Codebase Patterns (Inherited)

- **Vendored protoc:** `protoc-bin-vendored` is already a build dependency in `stitchd-proto`. The xtask can reuse this to invoke `protoc-gen-doc` without requiring a system install.
- **Axum 0.8 Routing:** Use `{param}` syntax for path captures. New OpenAPI routes should follow this pattern.
- **Workspace resolver:** `resolver = "3"` — new `xtask` crate must not override this.

---

<!-- Learnings from implementation will be appended below -->
