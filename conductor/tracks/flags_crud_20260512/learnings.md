# Track Learnings: flags_crud_20260512

Patterns, gotchas, and context discovered during implementation.

## Codebase Patterns (Inherited)

### Backend (Rust)
- `FlagJson` in `stitchd-gateway/src/routes/flags.rs` is minimal (`key + enabled`
  only) — admin API needs a separate `AdminFlagJson` with full data.
- `create_variant` and `update_rules` gateway handlers are stubs (202 no-op) —
  both need real implementation this track.
- `rule_payload` bytes in `FlagRule` proto = `serde_json::to_vec(&condition)` —
  conditions are JSON-serialized `ConditionExpr`. Pass through as `serde_json::Value`
  in admin response; no custom decoding needed.
- `feature_flags` table has no `name` or `description` columns — migration required.
- `MutationKind::Archive` already exists in the proto — gateway just needs to wire it.
- `FlagRecord` is in `stitchd-core/src/flag.rs` — extend there first, then DB repo,
  then service, then proto, then gateway (dependency chain).
- Axum 0.8: use `{param}` path syntax (not `:param`).
- SQLx offline mode: new queries require `cargo sqlx prepare` after adding them;
  use `sqlx::query_as` for new tables to avoid offline-cache failures.

### Frontend (React/TypeScript)
- `verbatimModuleSyntax: true` — always use `import type { Foo }` for types/interfaces.
- Never run `npx tsc` — use `node_modules/.bin/tsc --noEmit -p tsconfig.app.json`.
- No test runner (vitest/jest) in admin/ — verification is tsc + lint + browser.
- RBAC gating pattern: `disabled` + `style={{ opacity: 0.35 }}` (not display:none)
  for missing permissions. `LockOverlay` for zero read access.
- `usePermissions()` seeds from JWT on mount, then fetches `/v1/auth/me/permissions`.
- Sidebar picker pattern: `.org-switcher` + `.org-avatar` + `.org-meta` classes.
- `useParams` fires on both `/org/:orgId` and `/superadmin/orgs/:orgId` — use
  `location.pathname.startsWith('/org/')` to disambiguate.
- EnvSwitcher stale-ID fallback: always fall back to `environments[0]` when
  stored envId doesn't match current project.

---

<!-- Learnings from implementation will be appended below -->
