# SDK Keys

SDK keys authenticate the Rust SDK against `stitchd-server`. Each key is scoped to a
single `project + environment` pair — an SDK key cannot access data from another environment.

## Key Format

Keys are prefixed with `sk_live_` followed by a random token, e.g.:

```
sk_live_a3f8b2c9d1e4...
```

## Creating an SDK Key

Use the Admin REST API to create a key for a specific environment:

```bash
curl -X POST "http://localhost:8080/api/v1/environments/{env_id}/sdk-keys" \
  -H "Authorization: Bearer <admin_token>" \
  -H "Content-Type: application/json" \
  -d '{"name": "production-server"}'
```

The response includes the full key value — **store it immediately**; the server does not
return the plaintext again after creation.

## Using the Key in the SDK

Pass the key in `SdkConfig`:

```rust
let config = SdkConfig::new(
    "http://localhost:9090",   // gRPC endpoint
    "http://localhost:8080",   // REST endpoint
    "sk_live_...",             // SDK key
);
```

## Key Rotation

At least one active SDK key per environment is enforced at the API level — you cannot
delete the last key. The safe rotation procedure is:

1. **Create** a new key via the API
2. **Deploy** the new key to your application (update `SdkConfig`)
3. **Verify** the new key is active and receiving traffic
4. **Revoke** the old key via `DELETE /api/v1/sdk-keys/{key_id}`

This zero-downtime rotation ensures no evaluation gaps during key rollover.

## Listing Keys

```bash
curl "http://localhost:8080/api/v1/environments/{env_id}/sdk-keys" \
  -H "Authorization: Bearer <admin_token>"
```

Returns key IDs, names, creation timestamps, and active status — never the plaintext
secret after initial creation.
