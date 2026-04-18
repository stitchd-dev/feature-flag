# SDK Keys

SDK keys authenticate the Rust SDK against the server. Each key is scoped to a specific project and environment.

## Creating an SDK Key

Use the Admin REST API to create an SDK key for an environment:

```bash
curl -X POST http://localhost:8080/api/v1/environments/{env_id}/sdk-keys \
  -H "Authorization: Bearer <admin_token>" \
  -H "Content-Type: application/json"
```

## Key Rotation

At least one active SDK key per environment is enforced. To rotate:

1. Create a new key
2. Update your SDK client configuration
3. Revoke the old key once traffic has migrated
