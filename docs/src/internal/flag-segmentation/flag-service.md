# Flag Service (`stitchd-flag-service`)

## Responsibility

Stores, evaluates, and streams feature flag definitions. Responsibilities include:

- Flag CRUD (create, update, delete, archive)
- Flag evaluation (resolving the winning variant for a context)
- SDK definition sync (server-streaming flag definitions to SDK clients)
- SDK key validation (on behalf of the gateway for SDK routes)
- Per-flag hashing configuration management

## Port

| Transport | Default Port |
|-----------|-------------|
| gRPC | `50051` |

## Service: `FlagService`

**Package:** `stitchd.flags.v1`

### `GetFlag`

```
rpc GetFlag(GetFlagRequest) returns (FeatureFlag)
```

Fetch a single flag definition by key.

**`GetFlagRequest` fields:**

| Field | Type | Description |
|-------|------|-------------|
| `environment_id` | string | Environment scope |
| `flag_key` | string | Flag identifier |

### `ListFlags`

```
rpc ListFlags(ListFlagsRequest) returns (ListFlagsResponse)
```

List all flag definitions for an environment.

### `MutateFlag`

```
rpc MutateFlag(MutateFlagRequest) returns (MutateFlagResponse)
```

Create, update, delete, or archive a flag. Uses optimistic locking — include the current `version` for UPDATE/DELETE/ARCHIVE; the server returns `ABORTED` if the stored version differs.

**`MutateFlagRequest` fields:**

| Field | Type | Description |
|-------|------|-------------|
| `environment_id` | string | Environment scope |
| `kind` | `MutationKind` | `CREATE`, `UPDATE`, `DELETE`, or `ARCHIVE` |
| `flag` | `FeatureFlag` | Flag definition |
| `version` | uint64 | Optimistic-locking token (required for UPDATE/DELETE/ARCHIVE) |

### `GetFlagDefinitions`

```
rpc GetFlagDefinitions(GetFlagDefinitionsRequest) returns (stream FeatureFlag)
```

Server-streaming endpoint for SDK definition sync. Streams the full set of flag definitions for an environment, then keeps the connection open to stream incremental updates. Used internally; the gateway's `FlagSyncService` wraps this.

### `UpdateFlagHashing`

```
rpc UpdateFlagHashing(UpdateFlagHashingRequest) returns (UpdateFlagHashingResponse)
```

Replace the hashing configuration for a flag, controlling which context parameters drive percentage-rollout bucketing.

**`UpdateFlagHashingRequest` fields:**

| Field | Type | Description |
|-------|------|-------------|
| `environment_id` | string | Environment scope |
| `flag_key` | string | Flag to configure |
| `configs` | repeated `FlagHashingConfig` | Ordered list of context parameters used for hashing |

**`FlagHashingConfig` fields:**

| Field | Type | Description |
|-------|------|-------------|
| `parameter_key` | string | Attribute name (e.g., `user_id`) |
| `parameter_type` | string | Attribute type (e.g., `string`) |
| `order` | int32 | Priority order when multiple parameters are configured |

## Auth Requirements

Internal callers pass the RBAC context resolved by the gateway as gRPC metadata. The flag service does not re-validate credentials — it trusts the context injected by the gateway.
