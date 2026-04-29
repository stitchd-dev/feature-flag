# Track Learnings: env_sdk_rbac_20260429

Patterns, gotchas, and context discovered during implementation.

## Codebase Patterns (Inherited)

### Rust / Backend
- **Axum 0.8 routing:** Use `{param}` path syntax (not `:param`). New PATCH/DELETE routes follow this.
- **mgmt_routes tree:** All new management routes go under `mgmt_routes` in `router.rs` — they get JWT + `require_non_system_org` middleware automatically.
- **RbacContext in requests:** The auth middleware injects `RbacContext` as a request extension. The permissions endpoint can extract it via `req.extensions().get::<RbacContext>()`.
- **`sqlx::query_as` for new tables:** Use raw `sqlx::query_as::<_, Row>(r"...")` instead of `sqlx::query!` macros for new repository code to avoid offline compilation failures.
- **SQLx offline mode:** New queries break compilation in offline mode until `cargo sqlx prepare` is run against a live DB.
- **Proto only has CREATE RPCs currently:** The management service proto (`proto/management/v1/management_service.proto`) has no List/Rename/Delete RPCs yet — all 8 must be added from scratch.

### Frontend
- **`import type` required:** `verbatimModuleSyntax: true` — always use `import type { Foo }` for type-only imports.
- **API base:** `api` axios instance in `admin/src/lib/api.ts`; proxied via Vite `/api → localhost:8080`.
- **OrgContext:** Provides `orgId`, `projectId`, `envId` — `projectId` is the key scoping hook for this track.
- **Session shape:** Currently `{ token, orgId, isSystem, userId }` — needs `roles` and `permissions` fields added.
- **react-refresh rule:** Files exporting both components and constants need `// eslint-disable-next-line react-refresh/only-export-components` or split into separate files.
- **TypeScript check command:** `node_modules/.bin/tsc --noEmit -p tsconfig.app.json` (run from `admin/` directory).

---

<!-- Learnings from implementation will be appended below -->
