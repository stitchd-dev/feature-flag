# Gateway gRPC

The gateway exposes a single gRPC service for SDK clients that need real-time flag definition streaming: `FlagSyncService`.

## Service

| Service | Proto package | Gateway port |
|---------|--------------|-------------|
| `FlagSyncService` | `flags.v1` | `50050` |

This is a passthrough — the gateway forwards the streaming RPC directly to `stitchd-flag-service`.

## RPCs

### `SyncDefinitions`

```protobuf
rpc SyncDefinitions(SyncRequest) returns (stream SyncResponse);
```

Opens a server-streaming connection. The gateway pushes the full flag/segment definition set on connect, then streams incremental updates as definitions change.

**Auth:** Pass the SDK key as gRPC metadata:

```
x-sdk-key: sdk_live_abc123...
```

**`SyncRequest` fields:**

| Field | Type | Description |
|-------|------|-------------|
| `environment_id` | string | Environment to subscribe to |

**`SyncResponse` fields:**

| Field | Type | Description |
|-------|------|-------------|
| `flags` | repeated `FlagDefinition` | Current set of flag definitions |
| `segments` | repeated `SegmentDefinition` | Current set of segment definitions |
| `sequence_number` | int64 | Monotonically increasing sync counter |

## Connecting from the Rust SDK

The SDK automatically opens the streaming connection on `SdkClient::connect()`. You do not need to manage the gRPC channel directly.

```rust
let client = SdkClient::builder()
    .gateway("http://localhost:50050")
    .sdk_key("sdk_live_abc123")
    .build()
    .await?;
```

## Connecting from Other Languages

Use any gRPC client with the `flags/v1/flag_sync.proto` definition. Set the `x-sdk-key` metadata key on the channel.

```python
import grpc
from flags.v1 import flag_sync_pb2_grpc, flag_sync_pb2

channel = grpc.insecure_channel("localhost:50050")
stub = flag_sync_pb2_grpc.FlagSyncServiceStub(channel)
metadata = [("x-sdk-key", "sdk_live_abc123")]

for response in stub.SyncDefinitions(
    flag_sync_pb2.SyncRequest(environment_id="env-1"),
    metadata=metadata
):
    print(response)
```

See the [gRPC Protobuf Reference](../grpc/README.md) for the full proto schema.
