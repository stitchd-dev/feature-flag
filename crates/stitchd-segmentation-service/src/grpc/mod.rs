//! gRPC handler implementations for `SegmentationService`.

pub mod crud_tests;
pub mod evaluation_tests;
pub mod service;

pub use service::SegmentationServiceImpl;
