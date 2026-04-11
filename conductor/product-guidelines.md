# Product Guidelines

## API Design

### Protocol Split
- **Admin APIs:** REST (JSON/HTTP), versioned from day one (e.g. `/v1/`)
- **SDK Communication:** gRPC (Protobuf) — flag sync, polling, and event ingestion

### REST API Principles
- All APIs versioned under `/v1/`, `/v2/` etc. from day one
- All list endpoints use cursor-based pagination
- Consistent error response envelope: `{ code, message, details }`
- Mutation endpoints accept idempotency keys
- All admin mutations produce an audit log entry

## Authentication & Authorization

### Human Users (Admin API)
- JWT / OAuth2 — stateless token-based auth; user identity is per email address
- Future scope: SAML support for enterprise SSO

### Self-Hosted Super Admin
- In self-hosted mode, a bootstrapped **Super Admin** account exists above all organisations
- Super Admin can: create organisations, provision the first set of users for each organisation
- Super Admin has no access to project-level resources (flags, segments etc.) — 
  purely an organisational provisioning role

### Three-Tier Hierarchy

**Tier 1 — Organisation**
- User accounts exist at organisation level (one account per email)
- Two built-in roles: `Owner` and `Member`
- Owner has irrevocable super-admin status across all projects within the organisation

**Tier 2 — Project**
- Users are granted access to a project by a Project Admin
- Built-in project roles: `Admin` and custom roles built from granular permissions
- Organisation Owner is automatically a Project Admin on all projects; 
  no other admin can modify the Owner's role
- Project Admins can promote any member to Admin
- Project Admins can create custom roles using the granular RBAC system
- Permissions within a project can be scoped to:
  - **Environment** (wildcard support, e.g. `*`, `prod-*`)
  - **Feature Flag** (wildcard support, e.g. `payments-*`)
  - **Segment** (wildcard support)
- Wildcard resolution: most-specific match wins

**Tier 3 — Environment**
- Each project contains one or more environments
- SDK Keys are scoped to a specific project + environment

### Client SDKs
- SDK Key per environment (scoped to project + environment)
- Minimum one active key enforced; supports key rotation (create + revoke)

## Observability

- **Logging:** Structured JSON logs; `privateParameters` from contexts must never 
  appear in any log entry
- **Metrics:** Prometheus metrics exposed for scraping
- **Tracing:** OpenTelemetry traces for all request paths
- Private context fields (`privateParameters`) excluded from all telemetry layers

## SDK Design Principles
- gRPC-first: typed Protobuf contracts for flag sync and event submission
- Client performs all rule evaluation locally after initial payload fetch
- Polling interval configurable; future: streaming via gRPC server-side streaming
- SDK key validated on every request; rejected on invalid/revoked key

## Data Privacy
- `privateParameters` in any context must be stripped before any logging, metrics 
  tagging, or audit trail capture
- Events only accepted for pre-registered event keys with known types — 
  unknown events rejected at ingestion boundary
