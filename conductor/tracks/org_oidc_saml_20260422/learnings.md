# Track Learnings: org_oidc_saml_20260422

Patterns, gotchas, and context discovered during implementation.

## Codebase Patterns (Inherited)

### From patterns.md
- Rust 2024 edition — `resolver = "3"` in workspace Cargo.toml
- `std::env::set_var` requires `unsafe {}` with `// SAFETY:` comment
- `macro_rules!` for UUID-based newtypes with `sqlx::Type(transparent)`
- `#[sqlx::test(migrations = "./migrations")]` for isolated DB integration tests
- New `sqlx::query!` macros need `cargo sqlx prepare` before offline CI compile
- Axum 0.8: use `{param}` path syntax (not `:param`)
- `IntoResponse` on custom `ApiError` enum for HTTP status mapping
- `tower::ServiceExt::oneshot` for handler unit tests without TCP server
- `-D warnings` in CI — all clippy lints are hard errors
- `PrometheusBuilder::new().install_recorder()` for metrics; pass `PrometheusHandle` as Axum State

### From auth_20260421
- The org identifier type is `OrganisationId` (not `OrgId`) — always check actual type names in `crates/stitchd-core/src/id.rs`
- Enum privilege ordering: define role variants low-privilege first so `#[derive(Ord)]` gives higher variants more privilege
- `#[sqlx(rename_all = "snake_case")]` on sqlx enum maps Rust PascalCase to snake_case DB CHECK values
- Rate limiting: `governor` + `tower_governor` with `SmartIpKeyExtractor` for x-forwarded-for → x-real-ip → peer

### Auth-Specific Context
- `auth_providers` table already exists in DB with CRUD operations in `PgAuthProviderRepository`
- `OidcProvider` in `crates/stitchd-core/src/auth/oidc.rs` — has `from_discovery()`, `authorization_url()`, `exchange_code()`
- SAML processing in `crates/stitchd-core/src/auth/saml.rs`
- AES-256-GCM encryption for secrets already implemented (used for TOTP) — reuse same pattern
- `JwtEngine::issue()` in `stitchd-core::auth::jwt` — reuse for issuing JWT after SSO login
- `client_secret` is stored encrypted in the `config` JSONB column of `auth_providers`

---

<!-- Learnings from implementation will be appended below -->
