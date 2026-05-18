//! Protobuf-generated types and tonic service stubs for Stitchd gRPC APIs.
//!
//! Generated code is included via [`include_proto!`] macros.
//! Do not edit generated files directly — modify the `.proto` sources instead.

// Generated code does not conform to our lint rules.
#![allow(warnings, missing_docs, clippy::all, clippy::pedantic, clippy::nursery)]

/// Common shared types (Context, ParameterValue).
pub mod common {
    pub mod v1 {
        tonic::include_proto!("stitchd.common.v1");
    }
}

/// Feature flag sync service types and client/server stubs.
pub mod flags {
    pub mod v1 {
        tonic::include_proto!("stitchd.flags.v1");
    }
}

/// Segment types.
pub mod segments {
    pub mod v1 {
        tonic::include_proto!("stitchd.segments.v1");
    }
}

/// Event ingestion service types and client/server stubs.
pub mod events {
    pub mod v1 {
        tonic::include_proto!("stitchd.events.v1");
    }
}

/// Auth service — credential validation and RBAC context.
pub mod auth {
    pub mod v1 {
        tonic::include_proto!("stitchd.auth.v1");
    }
}

/// Experiment management service.
pub mod experiments {
    pub mod v1 {
        tonic::include_proto!("stitchd.experiments.v1");
    }
}

/// Platform management service (org/project/env/sdk-key/user bootstrap).
pub mod management {
    pub mod v1 {
        tonic::include_proto!("stitchd.management.v1");
    }
}

/// Stats recompute job service.
pub mod stats {
    pub mod v1 {
        tonic::include_proto!("stitchd.stats.v1");
    }
}

/// SDK contracts — both the gateway-hosted `SdkService` (SDK-facing) and the
/// backend RPCs the gateway calls (`FlagSdkBackendService`,
/// `SegmentationSdkBackendService`). Both files are in proto package
/// `stitchd.sdk.v1` so they generate into one Rust module here.
pub mod sdk {
    pub mod v1 {
        tonic::include_proto!("stitchd.sdk.v1");
    }
}

/// Analytics service — event ingestion, context registry, eval stats, context intelligence.
pub mod analytics {
    pub mod v1 {
        tonic::include_proto!("stitchd.analytics.v1");
    }
}

mod tests;
