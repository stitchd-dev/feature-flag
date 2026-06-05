# stitchd-schedule-service

<!-- cargo-rdme start -->

`stitchd-schedule-service` — Scheduled-change lifecycle service.

Applies one-shot and recurring (RRULE + IANA-tz, DST-aware) mutations to
flags, segments, and experiments at their scheduled time. A background tokio
interval loop (mirroring `stitchd-stats-service`) claims due changes from
PostgreSQL with `FOR UPDATE SKIP LOCKED` — restart-safe and idempotent — and
dispatches each to the owning service's canonical mutation RPC (flag →
flag-service `MutateFlag`, experiment → experimentation-service
`TransitionExperiment`, segment → segmentation-service `UpdateAdminSegment`).
It honors the whole-flag experiment lock and validates each transition at
fire time, recording every attempt in the `scheduled_change_runs` history.
Exposes the gRPC `ScheduleService` (create/list/get/cancel/pause/resume) plus
a health/metrics HTTP endpoint.

Entity-specific apply logic lives behind the `apply::Applier` seam; the
scheduler core is generic over an injected clock and applier for testing.

<!-- cargo-rdme end -->
