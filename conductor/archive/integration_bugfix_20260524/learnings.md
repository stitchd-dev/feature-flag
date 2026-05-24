# Track Learnings: integration_bugfix_20260524

Patterns, gotchas, and context discovered during implementation.

## Codebase Patterns (Inherited)

### Stack Startup
- All six gRPC services + gateway are separate Cargo workspace binaries. Start each with its own `STITCHD_*` env vars (port, DB URL, gRPC peer addresses). See `tech-stack.md` for the env-var naming convention (`STITCHD_{SERVICE}_GRPC_PORT`, etc.).
- Admin UI dev server proxies `/api → http://localhost:8080` (strips `/api` prefix). Configured in `admin/vite.config.ts`.
- Docker-only databases: `docker compose up postgres clickhouse scylladb -d --wait`; run migrations before starting services.

### SDK Key Auth
- SDK key is passed as `x-sdk-key` header on both gRPC metadata and REST requests.
- Scoped to `(project_id, environment_id)` — a key from env A will be rejected for requests targeting env B.
- Min-1-active invariant enforced in `stitchd-auth-service`; revocation of last key returns an error.

### Cross-Context Hashing
- `hash_inputs: Vec<HashSelector>` on a percentage-rollout rule is the canonical selector list (post `flag_eval_unify_20260522`).
- Each selector is `ContextKey { context_type }` (hashes the `key` field) or `ContextParameter { context_type, parameter }` (hashes a named parameter).
- Selectors mix freely across context types — a single rule can hash on `user.key + user.params.tier + device.params.os`.
- Hash algorithm: Murmur3 → bucket 0–999. Same algorithm used in `stitchd-core::evaluation::evaluate_flag` AND the SDK; parity is required.

### Evaluate-Preview Parity
- `evaluate_preview` calls `stitchd-core::evaluation::evaluate_flag` with `TraceLevel::Full`.
- The Rust SDK's `evaluate()` calls the same function with the caller-requested trace level.
- If evaluate-preview and SDK return different variants for the same input, the bug is in how `hash_inputs` is being serialised / deserialised between REST/proto and the core function.

### TDD on Bug Fixes
- Per workflow.md: write a failing test reproducing the bug before fixing it. Run test to confirm it fails, then implement the fix.
- For backend: `cargo test -p <crate>` after each fix. For frontend: `npm run lint` + `tsc --noEmit`.

### sqlx Offline Cache
- After adding new `sqlx::query!` macros (especially in test code): regenerate with `SQLX_OFFLINE=false cargo sqlx prepare --workspace -- --tests`. The `-- --tests` flag is required to catch queries in `#[cfg(test)]` and `tests/` directories.

---

<!-- Learnings from implementation will be appended below -->
