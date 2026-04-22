# Auth Service (`stitchd-auth-service`)

## Responsibility

Handles all identity and access-control concerns for the platform:

- Human-user login (email + password → JWT)
- Token validation for incoming gateway requests
- SDK key validation (proxied through from the gateway)
- RBAC context computation (org, environment, roles, permissions)
- Organisation and user management (create org, create user, seed admin)

## Port

| Transport | Default Port |
|-----------|-------------|
| gRPC | `50052` |

## Service: `AuthService`

**Package:** `stitchd.auth.v1`

### `ValidateCredential`

```
rpc ValidateCredential(CredentialRequest) returns (RbacContext)
```

Validates a credential (JWT bearer token or SDK key) and returns the resolved RBAC context. Called by the gateway on every authenticated request.

Returns `UNAUTHENTICATED` if the credential is invalid or expired.

**`CredentialRequest` fields:**

| Field | Type | Description |
|-------|------|-------------|
| `bearer_token` | string (oneof) | A signed JWT issued by the human-auth flow |
| `sdk_key` | string (oneof) | A raw SDK key presented via `x-sdk-key` header |

**`RbacContext` fields:**

| Field | Type | Description |
|-------|------|-------------|
| `tenant_id` | string | Organisation ID |
| `environment_id` | string | Environment scope (empty for admin tokens) |
| `roles` | repeated string | Assigned roles |
| `permissions` | repeated string | Resolved permission set |
| `subject` | string | `user_id` for JWT; `sdk_key_id` for SDK keys |
| `is_system` | bool | `true` for users in the platform System org — grants admin route access |

### `LoginWithPassword`

```
rpc LoginWithPassword(LoginRequest) returns (LoginResponse)
```

Authenticates an email + password credential and issues a JWT access token and refresh token.

**`LoginRequest` fields:**

| Field | Type | Description |
|-------|------|-------------|
| `email` | string | User email address |
| `password` | string | User password |
| `org_id` | string | Optional org scope; defaults to user's first org |

**`LoginResponse` fields:**

| Field | Type | Description |
|-------|------|-------------|
| `access_token` | string | Signed JWT |
| `refresh_token` | string | Refresh token for obtaining new access tokens |
| `expires_in` | int64 | Seconds until the access token expires |
| `user_id` | string | Resolved user ID |
| `org_id` | string | Resolved organisation ID |

## Auth Requirements

All calls to `AuthService` originate from `stitchd-gateway`. Internal services never call `AuthService` directly — the gateway resolves RBAC context and injects it as gRPC metadata for downstream services.
