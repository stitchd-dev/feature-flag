# Track Learnings: sdk_rewrite_20260516

Patterns, gotchas, and context discovered during implementation.

## Codebase Patterns (Inherited)

Relevant patterns from `conductor/patterns.md` for this track:

- **In-process cache primitive:** Use `moka::future::Cache<K, V>` with `time_to_live` for in-process caching. Call `.get_or_try_insert_with(key, loader)` — concurrent callers for the same key coalesce to a single loader invocation. Invalidate explicitly with `.invalidate(&key).await`. (from db_optim_20260516)
- **Pagination total without second query:** Use `COUNT(*) OVER()` window function — useful for paginated reads in SDK admin endpoints if needed. (from db_optim_20260516)
- **`serde_urlencoded` + `#[serde(flatten)]` + `u32`:** Axum's `Query<T>` extractor uses `serde_urlencoded` which passes string values; `u32` inside flattened structs needs the custom `de_u32_from_str` visitor. (from db_optim_20260516)
- **Vendored protoc:** `protoc-bin-vendored` is the project's build-time protoc source. New `.proto` files in `sdks/spec/proto/` must be wired into `crates/stitchd-proto/build.rs`. (from scaffold_20260411)
- **Local `DATABASE_URL` for `#[sqlx::test]`:** Use `postgresql://stitchd:stitchd@localhost:5432/stitchd` (TCP). Socket-auth URLs fail. (from scheduled_stats_20260423)
- **Cargo must run from the worktree root:** Always `cd .worktrees/<track_id>/` or pass `-C <worktree_path>` before any Cargo command when working in a worktree. (from env_sdk_rbac_20260429)
- **Domain model change order:** `stitchd-core` structs → DB repo queries → flag/domain service → proto definition → proto mapping (`mapping.rs`) → gateway handler. Skipping steps causes deep compile errors. (from flags_crud_20260512)
- **Admin vs SDK response shape:** Keep the SDK-facing response (used by polling) minimal for performance — do not bloat to satisfy UI needs. The SDK's `SyncDefinitions` should only include fields needed for evaluation. (from flags_crud_20260512)

## Key Architectural Decisions (this track)

- **Gateway is the sole SDK trust boundary.** Backend services do NOT validate SDK keys; they trust `x-env-id` propagated via gRPC metadata from the gateway.
- **All transport poll-based** — no streaming for this track. `SyncDefinitions` is unary, called on the SDK's polling interval.
- **LRU is lazy-on-miss with hybrid background refresh.** Entries created only when `evaluate()` hits an unseen `(context_type, key)`; background task refreshes only entries already resident.
- **List-segment refresh is filtered.** Background refresh task only requests memberships for segments referenced by current flag definitions (avoid polling unused segments).
- **`sdks/spec/` is the cross-language foundation.** All future SDKs (JS/Python/Go) must conform to the contracts here — proto for gRPC, OpenAPI for REST, JSON Schema for types, fixtures for behavioral conformance.

---

<!-- Learnings from implementation will be appended below -->
