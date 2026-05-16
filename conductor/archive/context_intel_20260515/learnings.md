# Track Learnings: context_intel_20260515

Patterns, gotchas, and context discovered during implementation.

## Codebase Patterns (Inherited)

- **Admin vs SDK response shape:** Always define a separate `AdminFooJson` struct in the gateway for admin UI responses. The SDK-facing `FooJson` must stay minimal. (from: flags_crud_20260512)
- **Proto condition payload:** `rule_payload` in `FlagRule` is `serde_json::to_vec(&ConditionExpr)` — a JSON-encoded condition tree stored as bytes. Deserialize with `serde_json::from_slice`. (from: flags_crud_20260512)
- **Domain model change order:** `stitchd-core` structs → DB repo → flag service → proto → mapping.rs → gateway handler. (from: flags_crud_20260512)
- **`verbatimModuleSyntax`:** Always use `import type { Foo }` for type-only imports in the admin UI. (from: admin_ui_20260427)
- **RBAC UI gating:** Use `disabled` + `style={{ opacity: 0.35 }}` for actions lacking permission. (from: env_sdk_rbac_20260429)
- **Cargo must run from the worktree root:** Always `cd .worktrees/context_intel_20260515/` before Cargo commands. (from: env_sdk_rbac_20260429)
- **`sqlx::query_as` for new tables:** New repository modules should use raw `sqlx::query_as::<_, Row>(r"...")` strings instead of `sqlx::query!` macros to avoid offline compilation failures. (from: scheduled_stats_20260423)
- **Local `DATABASE_URL` for `#[sqlx::test]`:** Use `postgresql://stitchd:stitchd@localhost:5432/stitchd` (TCP). Socket-auth URLs fail because `#[sqlx::test]` always connects over TCP. (from: scheduled_stats_20260423)
- **ClickHouse fire-and-forget pattern:** Use `tokio::spawn` for ClickHouse writes from the hot evaluation path. Log errors via `tracing::error!` but never propagate them to the caller. (from: events_20260419)
- **Vite dev proxy:** Admin UI proxies `/api → http://localhost:8080`. Use `VITE_API_BASE_URL` in `.env` for production builds. (from: admin_ui_20260427)

---

<!-- Learnings from implementation will be appended below -->
