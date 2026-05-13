# Track Learnings: segments_ui_20260513

Patterns, gotchas, and context discovered during implementation.

## Codebase Patterns (Inherited)

- **Domain model change order:** When adding a field to a domain type, always follow the chain: `stitchd-core` structs → DB repo queries → flag/domain service → proto definition → proto mapping (`mapping.rs`) → gateway handler. Skipping steps causes compile errors deep in the chain. (from: flags_crud_20260512)
- **Admin vs SDK response shape:** Always define a separate `AdminFooJson` struct in the gateway for admin UI responses (full data: name, description, variants, rules, version, timestamps). The SDK-facing `FooJson` must stay minimal for performance. Never bloat the SDK response to satisfy UI needs. (from: flags_crud_20260512)
- **Proto condition payload:** `rule_payload` in `FlagRule` is `serde_json::to_vec(&ConditionExpr)` — a JSON-encoded condition tree stored as bytes. In the gateway admin API, deserialize with `serde_json::from_slice` and pass through as `serde_json::Value`; no custom schema decoding needed. (from: flags_crud_20260512)
- **RBAC permissions must be expanded from role — not left empty:** `rbac_context_from_jwt` defaults to `permissions: vec![]`. Role→permission expansion must be done explicitly in `crates/stitchd-auth-service/src/rbac.rs`. (from: env_sdk_rbac_20260429)
- **`require_non_system_org` middleware guards mgmt_routes:** Management routes in `router.rs` sit behind both JWT auth and `require_non_system_org`. (from: env_sdk_rbac_20260429)
- **Sidebar picker pattern:** All sidebar entity pickers share one visual pattern: trigger button using `.org-switcher` + `.org-avatar` + `.org-meta` + `.org-name` + `.org-chevron`; dropdown with "Switch to" label, `.sidebar-link` rows, checkmark on active item, outside-click-to-close via `useRef` + `mousedown` listener. (from: env_sdk_rbac_20260429)
- **Cargo must run from the worktree root:** Always `cd .worktrees/<track_id>/` before any Cargo command when working in a worktree. (from: env_sdk_rbac_20260429)
- **`verbatimModuleSyntax`:** Use `import type { Foo }` for type-only imports. (from: admin_ui_20260427)
- **SQLx offline compilation:** New queries need `cargo sqlx prepare` or use `sqlx::query_as` raw strings. (from: segmentation_20260412)

---

<!-- Learnings from implementation will be appended below -->
