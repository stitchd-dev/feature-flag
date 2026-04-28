# Codebase Patterns

Reusable patterns discovered during development. Read this before starting new work.

## Code Conventions

- Rust 2024 edition requires `resolver = "3"` in workspace `Cargo.toml` (not `"2"`). (from: scaffold_20260411, 2026-04-11)
- `std::env::set_var` is **unsafe** in Rust 2024 — always wrap in `unsafe {}` with a `// SAFETY:` comment, even in build scripts. (from: scaffold_20260411, 2026-04-11)
- `rustfmt.toml` options like `imports_granularity`, `group_imports`, `wrap_comments`, `normalize_comments` are **nightly-only** — they silently no-op on stable without error. Strip them from stable configs. (from: scaffold_20260411, 2026-04-11)

## Architecture

- **Prometheus metrics:** Use `PrometheusBuilder::new().install_recorder()` to get a `PrometheusHandle`. Pass it as Axum `State` and call `handle.render()` in the `/metrics` route handler. (from: scaffold_20260411, 2026-04-11)
- **Graceful shutdown:** Use `tokio::select!` over `ctrl_c()` + `SIGTERM` (gated `#[cfg(unix)]`) as the shutdown signal. Pass to `axum::serve(...).with_graceful_shutdown(...)`. (from: scaffold_20260411, 2026-04-11)
- **Vendored protoc:** Add `protoc-bin-vendored` as a build dependency in `stitchd-proto` and set `PROTOC` env var in `build.rs`. Eliminates system `protoc` requirement for all contributors and CI. (from: scaffold_20260411, 2026-04-11)
- **API Error Mapping:** Implement `IntoResponse` for a custom `ApiError` enum that maps internal errors (Repository, Validation, etc.) to HTTP status codes. (from: segmentation_20260412, 2026-04-12)
- **Axum 0.8 Routing:** Use `{param}` syntax for path captures (e.g., `/users/{id}`) as Axum 0.8 deprecated the `:param` style. (from: fix_errors_20260412, 2026-04-12)

## Rust 2024 Edition Patterns

- When iterating a `HashMap` and only needing keys or values, use `.keys()` / `.values()` — `for (k, _) in map` triggers `clippy::for_kv_map` as a warning-level error with `-D warnings`. (from: rule_engine_20260412, 2026-04-12)
- In filter closures over iterator references (e.g. `.filter(|(_, d)| ...)`), the value `d` is `&&T` — dereference with `**d` or use `.filter(|&(_, d)| ...)`. Pattern-binding `&d` inside a non-reference outer pattern fails in Rust 2024. (from: rule_engine_20260412, 2026-04-12)
- **ID Newtypes:** Use `macro_rules!` to define repetitive UUID-based newtypes with `sqlx::Type(transparent)` to minimize boilerplate. (from: domain_20260411, 2026-04-11)
- **Type Dispatch:** `matches!` macro with tuple patterns is the cleanest way to express multi-arm type dispatch (e.g., `matches!((val, type), (Variant::Int(_), Type::Int))`). (from: domain_20260411, 2026-04-11)
- **Wildcard Matching:** Use `strip_suffix('*')` for simple prefix-based wildcard matching without the overhead of regex. (from: domain_20260411, 2026-04-11)

## Auth Patterns

- **Enum privilege ordering:** Define role enum variants low-privilege first so `#[derive(Ord)]` gives higher-numbered variants more privilege. E.g., `OrgMember=0, OrgAdmin=1` → `OrgAdmin > OrgMember` without custom `PartialOrd`. (from: auth_20260421, 2026-04-21)
- **sqlx enum DB mapping:** `#[sqlx(rename_all = "snake_case")]` on an enum with `sqlx::Type` maps Rust `PascalCase` variants to `snake_case` in the DB CHECK constraint values automatically. (from: auth_20260421, 2026-04-21)
- **ID type name:** The org identifier type is `OrganisationId` (not `OrgId`) — always check actual type names in `crates/stitchd-core/src/id.rs` before using. (from: auth_20260421, 2026-04-21)
- **Rate limiting pattern:** `governor` + `tower_governor` with a `SmartIpKeyExtractor` that reads `x-forwarded-for` → `x-real-ip` → peer address in that order to correctly key per-client behind a reverse proxy. (from: auth_20260421, 2026-04-21)

## Gotchas

- `clickhouse` crate v0.13 has no `derive` feature — use `uuid`, `time`, `lz4` features instead. (from: scaffold_20260411, 2026-04-11)
- **SQLx Offline Compilation:** `sqlx::query!` macros require a live DB or up-to-date `.sqlx` cache. New queries will break compilation in offline mode until `cargo sqlx prepare` is executed. (from: segmentation_20260412, 2026-04-12)
- **Database Extension Dependencies:** Call functions from extensions (like `pg_partman`) using plain `sqlx::query` to avoid macro-based compilation errors when extensions aren't available in the local build environment. (from: segmentation_20260412, 2026-04-12)
- **OpenTelemetry version alignment:** `tracing-opentelemetry 0.29` requires `opentelemetry ^0.28`. `opentelemetry-otlp 0.27` requires `opentelemetry ^0.27`. These produce **incompatible types** — pin all OTel crates to the same minor version. (from: scaffold_20260411, 2026-04-11)
- `opentelemetry_sdk 0.28` made `Resource::new()` private. Use `Resource::builder()` — confirm exact builder API before wiring OTLP in the observability track. (from: scaffold_20260411, 2026-04-11)
- **Recursive Types:** Recursive enums or structs (like expression trees) must use `Box<T>` for recursive variants to avoid infinite-size type errors. (from: rule_engine_20260412, 2026-04-12)
- **SQLx CLI Installation:** Use `--no-default-features --features rustls,postgres` when installing `sqlx-cli` to avoid unnecessary system dependencies like OpenSSL. (from: domain_20260411, 2026-04-11)
- **Integer Truncation:** Rust 2024 lints against implicit/risky conversions; `as i32` on `usize` should be avoided or handled with `try_into()` if overflow is possible. (from: fix_errors_20260412, 2026-04-12)

## Testing

- Axum router integration tests: use `tower::ServiceExt::oneshot` to send a single request through the router without starting a real TCP server. Add `tower` as a `[dev-dependencies]` entry. (from: scaffold_20260411, 2026-04-11)
- **Isolated DB Testing:** `#[sqlx::test(migrations = "./migrations")]` is the idiomatic way to run fast, isolated database tests with automatic migration handling in SQLx 0.8. (from: domain_20260411, 2026-04-11)
- **`cargo sqlx prepare` skips `#[cfg(test)]`:** Test-only queries are NOT captured by `cargo sqlx prepare` (which runs `cargo check`, not `cargo test`). Always compile tests against a live `DATABASE_URL`, never `SQLX_OFFLINE=true`. (from: scheduled_stats_20260423, archived 2026-04-23)
- **`cargo sqlx prepare` deletes cached test queries:** Re-running `cargo sqlx prepare` may remove previously-cached test-only query entries. Verify test compilation with a live DB after every `prepare` run. (from: scheduled_stats_20260423, archived 2026-04-23)
- **`sqlx::query_as` for new tables:** New repository modules should use `sqlx::query_as::<_, Row>(r"...")` raw strings instead of `sqlx::query!` macros to avoid offline compilation failures when the `.sqlx` cache hasn't been populated for a new table yet. (from: scheduled_stats_20260423, archived 2026-04-23)
- **Local `DATABASE_URL` for `#[sqlx::test]`:** Use `postgresql://stitchd:stitchd@localhost:5432/stitchd` (TCP). Socket-auth URLs (e.g. `postgresql://vishal@localhost/stitchd`) fail because `#[sqlx::test]` always connects over TCP. (from: scheduled_stats_20260423, archived 2026-04-23)
- **Test env-var isolation:** Config tests and sqlx tests share the same binary. Use `--test-threads=1` and an `EnvGuard` RAII wrapper to prevent env-var contamination across test cases. (from: scheduled_stats_20260423, archived 2026-04-23)

---
Last refreshed: 2026-04-23

## Frontend (Admin UI) Patterns

- **`verbatimModuleSyntax`:** Vite + TypeScript projects with `verbatimModuleSyntax: true` require `import type { Foo }` for any type-only import — a plain `import { Foo }` triggers TS1484. Always use the `type` keyword for types, interfaces, and enums not used as values. (from: admin_ui_20260427, archived 2026-04-28)
- **Gateway API shapes are minimal:** The gateway's JSON responses contain only the fields needed by the SDK evaluation path (e.g., `FlagJson` only has `key` + `enabled`). Any admin UI displaying richer data (owner, sparklines, segments, variants) must use mock data or a dedicated admin API. (from: admin_ui_20260427, archived 2026-04-28)
- **react-refresh ESLint rule:** Files that export both components and non-component values (maps, constants, type aliases) trigger `react-refresh/only-export-components`. Fix by either moving the non-component export to a separate file, or adding `// eslint-disable-next-line react-refresh/only-export-components` on that export line. (from: admin_ui_20260427, archived 2026-04-28)
- **TypeScript CLI in Vite projects:** Never run `npx tsc` — it resolves to a stray `tsc` package (2.0.x). Always use `node_modules/.bin/tsc --noEmit -p tsconfig.app.json` with the full absolute path to the admin directory as CWD. (from: admin_ui_20260427, archived 2026-04-28)
- **Vite dev proxy for gateway:** Admin UI uses `vite.config.ts` server proxy: `/api → http://localhost:8080` with `changeOrigin: true` and path rewrite stripping the `/api` prefix. Set `VITE_API_BASE_URL` in `.env` for production builds pointing directly at the gateway. (from: admin_ui_20260427, archived 2026-04-28)
