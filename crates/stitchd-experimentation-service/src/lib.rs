//! Stitchd Experimentation Service — gRPC service for experiment lifecycle management.
//!
//! Implements the `ExperimentationService` proto contract:
//! - `CreateExperiment` — creates an experiment; calls Flag Service when activating
//! - `GetExperiment` — fetch a single experiment by ID
//! - `ListExperiments` — list all experiments for an environment
//! - `GetResults` — fetch pre-computed statistical results

pub mod analytics_client;
pub mod dict_refresh;
pub mod exposure_reader;
pub mod flag_client;
pub mod service;
