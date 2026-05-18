# stitchd-proto

Protobuf definitions and generated tonic stubs for all Stitchd gRPC services.

## Services

| Proto Package | Service | Hosted By |
|--------------|---------|-----------|
| `auth.v1` | `AuthService` | `stitchd-auth-service :50051` |
| `management.v1` | `ManagementService` | `stitchd-auth-service :50051` |
| `flags.v1` | `FlagService` | `stitchd-flag-service :50052` |
| `flags.v1` | `FlagSyncService` | `stitchd-flag-service :50052` |
| `segments.v1` | `SegmentationService` | `stitchd-segmentation-service :50053` |
| `events.v1` | `EventIngestionService` | `stitchd-analytics-service :50053` |
| `experiments.v1` | `ExperimentationService` | `stitchd-experimentation-service :50055` |
| `common.v1` | Shared message types (Context, etc.) | — |

## Code Generation

Stubs are generated at build time by `build.rs` using `tonic-build` and `prost-build` with the `protoc-bin-vendored` compiler. No external `protoc` installation is required.

Proto source files live in `src/` alongside the generated module tree.

## Usage

Every binary and library crate that needs to call or serve a gRPC service adds this crate as a dependency and imports from the relevant generated module:

```rust
use stitchd_proto::flags::v1::flag_service_server::FlagServiceServer;
use stitchd_proto::flags::v1::flag_service_client::FlagServiceClient;
```
