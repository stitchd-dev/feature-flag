# Plan: Fix audit capture layering

## Phase 1: Re-layer audit middleware (inner to auth)
- [x] Task 1.1: Change `audit_middleware` to `State<Arc<GatewayState>>` + read
      `state.audit_pool`; build `PgAuditWriter` inline (or write directly).
- [x] Task 1.2: Layer it inner-to-auth on resource/mgmt/superadmin groups in
      `build_router`; remove the outer layer + writer Arc in `main.rs`.
- [x] Task 1.3: Update tests (the existing #[sqlx::test] injects RbacContext via a
      preceding layer — adapt to the new State signature); add a no-RbacContext
      skip test. cargo test -p stitchd-gateway green; clippy/fmt; sqlx-check.
- [x] Task: Conductor - User Manual Verification 'Phase 1' (Protocol in workflow.md)

## Phase 2: Live verification
- [x] Task 2.1: Rebuild + restart gateway against the live stack; run the smoke
      (login → create/toggle → GET audit) and confirm org-scoped rows with
      actor_id + org_id. Update learnings + file the service-unification follow-up.
- [x] Task: Conductor - User Manual Verification 'Phase 2' (Protocol in workflow.md)
