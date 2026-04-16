# Product Guidelines

## API Design & Protocols
*   **Protocol Split:** 
    *   **Admin APIs:** REST (JSON/HTTP), strictly versioned (e.g., `/v1/`).
    *   **SDK Communication:** gRPC (Protobuf) for flag sync, polling, and event ingestion.
*   **REST Principles:**
    *   Cursor-based pagination for all list endpoints.
    *   Consistent error envelope: `{ code, message, details }`.
    *   Idempotency keys required for mutation endpoints.
    *   Automatic audit log generation for every admin mutation.

## Authentication & Authorization
*   **Human Users:** JWT/OAuth2 stateless token-based authentication (identity per email).
*   **Self-Hosted Super Admin:** 
    *   Bootstrapped role above all organizations.
    *   Can create organizations and provision initial users.
    *   *No access* to project-level resources (flags, segments, etc.).
*   **Three-Tier RBAC Hierarchy:**
    1.  **Organization:** Roles: `Owner` (irrevocable project-wide admin) and `Member`.
    2.  **Project:** Managed by Project Admins. Supports custom roles with granular permissions scoped to Environments, Flags, or Segments (with wildcard support; most-specific match wins).
    3.  **Environment:** Project contains multiple environments; SDK Keys are scoped here.
*   **SDK Keys:** Minimum one active key required; supports rotation (create + revoke).

## SDK Design Principles
*   **gRPC-First:** Typed Protobuf contracts for all SDK-to-Server communication.
*   **Local Evaluation:** Clients perform all rule evaluation locally after the initial payload fetch.
*   **Efficiency:** Configurable polling intervals with a future path to gRPC server-side streaming.
*   **Strict Ingestion:** SDK keys validated on every request.

## Observability & Data Privacy
*   **Logging:** Structured JSON logs. **CRITICAL:** `privateParameters` must never appear in any log entry, metric tag, or trace.
*   **Telemetry:** Prometheus metrics and OpenTelemetry traces for all request paths.
*   **Data Integrity:** Events are only accepted for pre-registered keys with known types; unknown events are rejected at the ingestion boundary.

## Product Principles
*   **Technical & Precise:** Documentation and error messages must be accurate and actionable (e.g., "Invalid context property type: expected 'int', received 'string'").
*   **Correctness over Speed:** Data integrity (optimistic locking) and statistical rigor (CUPED/Bayesian models) are the primary mandates.
*   **Auditability by Default:** Every change must be traceable. No "ghost" modifications.
