//! ClickHouse event ingestion for experiments and metrics.
//!
//! Only pre-registered events with known types are accepted at the ingestion boundary.
#![deny(warnings, missing_docs, clippy::all)]
#![warn(clippy::pedantic, clippy::nursery)]

// TODO: event types and ingestion service
// TODO: ClickHouse client setup

#[cfg(test)]
mod tests {
    #[test]
    fn it_compiles() {}
}
