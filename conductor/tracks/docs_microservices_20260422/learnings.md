# Track Learnings: docs_microservices_20260422

Patterns, gotchas, and context discovered during implementation.

## Codebase Patterns (Inherited)

- **xtask build target:** xtask uses `cargo build -p <crate>` then invokes the debug binary directly. Binary path is `target/debug/<crate-name>`.
- **OpenAPI generation:** `utoipa` + `utoipa-axum` annotations on Axum 0.8 routes; use `{param}` path syntax (not `:param`). Security schemes: `sdk_key` (x-sdk-key header) and `bearer_jwt` (Authorization: Bearer).
- **Axum 0.8 Routing:** Use `{param}` syntax for path captures — `:param` style is deprecated.
- **Mermaid diagrams:** `mdbook-mermaid` is already installed and wired into `book.toml`. Use fenced ` ```mermaid ` blocks.
- **SUMMARY.md patching:** xtask's `patch_summary_grpc()` locates the `# gRPC / Protobuf Reference` heading and replaces until the next `#` heading. Same pattern can be used for other dynamic sections.
- **Proto files location:** `proto/` directory at workspace root. `collect_proto_files()` walks it recursively.
- **gRPC internal-only:** Internal service-to-service gRPC calls do not pass through the gateway auth layer — they are on an internal Docker network. Document this clearly in service pages.
- **OrganisationId not OrgId:** The org identifier type is `OrganisationId` — check `crates/stitchd-core/src/id.rs` for all ID type names.

---

<!-- Learnings from implementation will be appended below -->
