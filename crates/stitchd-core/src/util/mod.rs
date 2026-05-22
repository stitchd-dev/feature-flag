//! Cross-cutting utilities shared between the workspace's microservices.
//!
//! These helpers live in `stitchd-core` because they are pure-Rust, have no
//! I/O of their own (callers supply the I/O closure), and several downstream
//! `*-service` crates need them. Keep this module narrowly scoped — anything
//! domain-specific belongs in its own `stitchd-core` submodule.

pub mod grpc_connect;
