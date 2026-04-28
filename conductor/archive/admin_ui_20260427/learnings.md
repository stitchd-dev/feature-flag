# Track Learnings: admin_ui_20260427

Patterns, gotchas, and context discovered during implementation.

## Codebase Patterns (Inherited)

- Rust 2024 edition conventions (see conductor/patterns.md) — not directly applicable to this frontend track
- Gateway uses Axum 0.8 with `{param}` path syntax — relevant when reading OpenAPI routes
- All gateway endpoints live behind `stitchd-gateway` on port 8080
- Auth JWT is issued by `stitchd-auth-service` via the gateway (`POST /auth/login`)
- OIDC flow: `/auth/oidc/authorize` triggers PKCE redirect; callback at `/auth/oidc/callback`
- SAML flow: `/auth/saml/initiate` triggers IdP redirect

---

<!-- Learnings from implementation will be appended below -->
