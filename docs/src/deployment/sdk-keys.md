# SDK Keys

SDK keys authenticate the Stitchd Rust SDK — and any future SDK that follows
the contract in `sdks/spec/` — against the gateway's SDK-facing routes. Each
key is scoped to a **single environment** and is the only credential SDK
clients ever see.

## Lifecycle Summary

| Stage      | Where                                                                 | Mechanism                                                                  |
|------------|-----------------------------------------------------------------------|----------------------------------------------------------------------------|
| Issue      | Admin REST API on the gateway (JWT-authenticated)                     | `POST /v1/management/environments/{environment_id}/sdk-keys`               |
| Store      | PostgreSQL `sdk_keys` table                                           | SHA-256 hash only — the raw key is never persisted server-side.            |
| Carry      | SDK ↔ gateway, every request                                          | `x-sdk-key` HTTP header (REST) or gRPC metadata key (SDK sync stream).     |
| Validate   | `stitchd-gateway::middleware::sdk_auth_middleware` → `auth-service`   | TTL-cached lookup; injects `SdkContext` into the request extensions.       |
| Revoke     | Admin REST API                                                        | `DELETE /v1/management/environments/{environment_id}/sdk-keys/{sdk_key_id}` |

## Scope & Identity

A key resolves to exactly one
`(organisation_id, project_id, environment_id)` tuple. There is no
project- or org-level key: anything that needs cross-environment access uses
a JWT-authenticated human or service principal on the admin surface, not an
SDK key.

The Auth Service's `ValidateCredential` RPC returns an `RbacContext` from
which the gateway constructs an
[`SdkContext`](https://github.com/stitchd-dev/feature-flag/blob/main/crates/stitchd-gateway/src/middleware/sdk_auth.rs)
with three fields — `environment_id`, `organisation_id`, and `sdk_key_id`.
These flow as `x-env-id` / `x-org-id` gRPC metadata to backend services,
which never see the raw SDK key.

## Key Format

By convention SDK keys carry a `sdk_live_*` or `sdk_test_*` prefix when
displayed in the admin UI and in documentation examples (the
[architecture flows](../architecture/service-flows.md) use this convention).
The on-the-wire value is treated as an opaque token by the gateway — the
auth service hashes it with SHA-256 and looks the hash up in
`sdk_keys.key_hash`. The prefix is informational; revocation status is the
sole correctness signal.

The `sdk_keys` table has these columns
([`crates/stitchd-db/migrations/20260411000002_sdk_keys.sql`](https://github.com/stitchd-dev/feature-flag/blob/main/crates/stitchd-db/migrations/20260411000002_sdk_keys.sql)):

| Column            | Type          | Notes                                                  |
|-------------------|---------------|--------------------------------------------------------|
| `id`              | `UUID`        | Surrogate ID; returned as `sdk_key_id` in API output.  |
| `environment_id`  | `UUID`        | Foreign key into `environments`.                       |
| `key_hash`        | `TEXT`        | SHA-256 of the raw key. Indexed by `idx_sdk_key_hash`.  |
| `is_active`       | `BOOLEAN`     | `false` after revocation.                              |
| `created_at`      | `TIMESTAMPTZ` |                                                        |
| `revoked_at`      | `TIMESTAMPTZ` | `NULL` until revoked.                                  |

## Issuing a Key

Create a key against an environment via the JWT-authenticated admin API:

```bash
curl -X POST "http://localhost:8080/v1/management/environments/${ENV_ID}/sdk-keys" \
  -H "Authorization: Bearer ${ADMIN_JWT}"
```

Response (`HTTP 201`):

```json
{
  "sdk_key_id": "11111111-1111-1111-1111-111111111111",
  "raw_key": "sdk_live_..."
}
```

The `raw_key` is the **only time** the plaintext value leaves the server.
Capture it immediately — there is no recovery path; lose it and you must
rotate.

The endpoint lives on the gateway's `mgmt_routes` router (see
[`crates/stitchd-gateway/src/router.rs`](https://github.com/stitchd-dev/feature-flag/blob/main/crates/stitchd-gateway/src/router.rs)),
which is JWT-authenticated via `auth_middleware` and gated by a
`require_non_system_org` check.

## Using a Key from the SDK

The Rust SDK takes the key in `SdkConfig`:

```rust
use stitchd_sdk_rust::{SdkClient, SdkConfig};

let config = SdkConfig::new(
    "http://localhost:50050",   // gateway gRPC sync endpoint
    "http://localhost:8080",    // gateway REST (list-segment + events)
    "sdk_live_...",             // the raw_key returned by create_sdk_key
);
let client = SdkClient::init(config).await?;
```

The SDK sets `x-sdk-key` on every gRPC sync request (metadata) and every
REST request (header). The gateway's
`sdk_auth_middleware` enforces presence on `/v1/sdk/*` and `/v1/events/track`;
the gRPC sync server enforces it in `stitchd_gateway::grpc_server` for the
streaming `SyncDefinitions` and unary `IngestSdkEvalLog` RPCs.

Bearer tokens are explicitly **rejected** on SDK routes — even a valid admin
JWT yields `401 missing_sdk_key`. This is defence in depth: the SDK
telemetry surface should never receive human-user credentials by accident.

## Listing Keys

```bash
curl "http://localhost:8080/v1/management/environments/${ENV_ID}/sdk-keys" \
  -H "Authorization: Bearer ${ADMIN_JWT}"
```

Returns key IDs, active status, and creation/revocation timestamps. The
plaintext is **never** returned post-creation.

## Rotation (Zero-Downtime)

The auth service treats every active key for an environment as
independently valid, so the safe rotation pattern is additive-then-revoke:

1. **Create** a new key with `POST /v1/management/environments/.../sdk-keys`.
2. **Deploy** the new key to your application(s) — update the `SdkConfig`
   passed to `SdkClient::init`.
3. **Verify** the new key is receiving traffic. The auth service emits
   per-key validation counters; you can also confirm by tailing
   `flag_evaluation_log` rows that arrived after the deploy.
4. **Revoke** the old key:
   ```bash
   curl -X DELETE \
     "http://localhost:8080/v1/management/environments/${ENV_ID}/sdk-keys/${OLD_KEY_ID}" \
     -H "Authorization: Bearer ${ADMIN_JWT}"
   ```

There is no grace period — the moment `is_active` flips to `false`,
subsequent validations return `invalid_sdk_key`. Cached validations in the
auth service's `SdkKeyCache` are bounded by its TTL (sub-second by
default), so revocation propagates to running gateways without an explicit
flush.

## Validation Cache

`stitchd-auth-service` keeps a per-key validation cache to keep the SDK
auth hot path well below 1 ms. Cache misses do a single
`SELECT ... WHERE key_hash = ? AND is_active` against the composite
`idx_sdk_keys_key_hash_active` index added in
`20260516000001_idx_sdk_key_hash.sql`. The cache stores
`(env_id, organisation_id, sdk_key_id)`, never the raw key.

The TTL is the only "stickiness" between a revoke and the moment the
gateway starts rejecting — typical bound is well under one second. There
is no manual cache invalidation API; rotation correctness does not depend
on one.

## Environment Variables

SDK key handling itself has no env vars — keys flow as request data, not
configuration. The surrounding plumbing is shared with the rest of the
auth stack (`STITCHD_JWT_SECRET`, the data-store URLs). See
[`./env-vars.md`](./env-vars.md) for the full reference.
