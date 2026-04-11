//! Rust client SDK for Stitchd.
//!
//! Initialises with a set of contexts, fetches flag rules from the server via gRPC,
//! evaluates all rules client-side, and polls for updates at a configurable interval.
#![deny(warnings, missing_docs, clippy::all)]
#![warn(clippy::pedantic, clippy::nursery)]

// TODO: SDK client (init, flag evaluation, polling loop)
// TODO: event submission

#[cfg(test)]
mod tests {
    #[test]
    fn it_compiles() {}
}
