//! gRPC handler implementations for `SegmentationService`.

pub mod crud_tests;
pub mod evaluation_tests;
pub mod list_membership_tests;
pub mod sdk_backend;
pub mod service;
pub mod update_tests;

pub use sdk_backend::SegmentationSdkBackendServiceImpl;
pub use service::SegmentationServiceImpl;
