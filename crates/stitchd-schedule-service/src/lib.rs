//! `stitchd-schedule-service` — Scheduled-change lifecycle service.
//!
//! Applies one-shot and recurring (RRULE + IANA-tz, DST-aware) mutations to
//! flags, segments, and experiments at their scheduled time. A background tokio
//! interval loop (mirroring `stitchd-stats-service`) claims due changes from
//! PostgreSQL with `FOR UPDATE SKIP LOCKED` — restart-safe and idempotent — and
//! dispatches each to the owning service's canonical mutation RPC. Exposes the
//! gRPC `ScheduleService` plus a health/metrics HTTP endpoint.
//!
//! Phase 3 implements the scheduler core + the flag apply path; segment and
//! experiment apply paths arrive in Phase 5 via the [`apply::Applier`] seam.

pub mod apply;
pub mod config;
pub mod grpc;
pub mod mapping;
pub mod scheduler;
