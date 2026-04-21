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

mod tests;
