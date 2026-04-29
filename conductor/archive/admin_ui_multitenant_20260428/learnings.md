# Track Learnings: admin_ui_multitenant_20260428

Patterns, gotchas, and context discovered during implementation.

## Codebase Patterns (Inherited from admin_ui_20260427)

- Gateway runs on port 8080; all API routes prefixed `/v1/`
- Auth JWT issued via `POST /v1/auth/login`; `org_id` in login request scopes the token
- `RbacContext.is_system = true` means system-org (superadmin); `false` means regular org user
- `RbacContext.tenant_id` holds the org ID the token is scoped to
- Admin routes (`/v1/admin/*`) require `is_system=true`; management routes require `is_system=false`
- Resource routes (flags, segments, experiments) use `project_id` or `env_id` in path — not `org_id`
- OIDC flow: provider-scoped authorize at `/v1/auth/oidc/{provider_id}/authorize`
- React Router v7 is in use (package.json shows `react-router-dom ^7.14.2`)
- Vite config proxies `/api` → gateway in dev mode

## Key Architectural Decisions

- JWT is decoded client-side (base64 payload only) to extract `is_system` — no crypto verify needed in the browser
- `org_id` comes from the login response body, not from JWT decode (LoginResponse includes it explicitly)
- Org context (`projectId`, `envId`) must be set post-login — no API endpoint to auto-discover these; stored in localStorage

---

<!-- Learnings from implementation will be appended below -->
