# Implementation Plan: Bandit Campaign Management

Track: `bandit_campaign_ui_20260610` · Beads epic: TBD · Branch: `track/bandit_campaign_ui_20260610`

Methodology: TDD. Backend gate: `cargo test -p stitchd-gateway`. Frontend gate:
`tsc -b`, `lint`, vitest (`CI=true`), `build`.

## Phase 1: Gateway create + stop

- [ ] Task 1.1 (TDD): Gateway unit tests (stub state) — `POST .../bandit-campaigns`
      and `POST .../bandit-campaigns/{id}/stop` return 200/502 against the stub
      experimentation client; create maps `{flag_id,name,config}` → request.
      <!-- files: crates/stitchd-gateway/src/routes/experiments.rs -->
- [ ] Task 1.2 (Green): Implement `create_bandit_campaign` + `stop_bandit_campaign`
      handlers (+ `CreateBanditCampaignBody`), register routes in `router.rs`, add
      `#[utoipa::path]` + register in `openapi.rs` paths(). `cargo test -p
      stitchd-gateway` green (incl. openapi contract).
      <!-- files: crates/stitchd-gateway/src/routes/experiments.rs, crates/stitchd-gateway/src/router.rs, crates/stitchd-gateway/src/openapi.rs -->
- [ ] Task: Conductor - User Manual Verification 'Phase 1' (Protocol in workflow.md)

## Phase 2: Admin client

- [ ] Task 2.1 (TDD): Failing tests — `createBanditCampaign` POSTs
      `{flag_id,name,config}` to the campaigns URL; `stopBanditCampaign` POSTs to
      `.../{id}/stop`; both return the campaign.
      <!-- files: admin/src/lib/api/bandit.campaigns.test.ts -->
- [ ] Task 2.2 (Green): Implement the two client fns + `BanditCampaignConfigInput`
      / `CreateBanditCampaignBody` types in `bandit.ts`.
      <!-- files: admin/src/lib/api/bandit.ts -->
- [ ] Task: Conductor - User Manual Verification 'Phase 2' (Protocol in workflow.md)

## Phase 3: UI campaign panel

- [ ] Task 3.1 (TDD): Pure config-builder test (`buildCampaignConfig`) +
      presentational render tests for the campaigns table + create modal validation.
      <!-- files: admin/src/pages/experiments/banditCampaignHelpers.test.ts, admin/src/pages/experiments/BanditCampaignsPanel.test.tsx -->
- [ ] Task 3.2 (Green): Implement `banditCampaignHelpers.ts` (config builder +
      Yup schema + status badge), `BanditCampaignsPanel.tsx` (list + create modal +
      stop confirm, org_admin-gated, loading/empty/error), and mount it on
      `ExperimentsList`.
      <!-- files: admin/src/pages/experiments/banditCampaignHelpers.ts, admin/src/pages/experiments/BanditCampaignsPanel.tsx, admin/src/pages/experiments/ExperimentsList.tsx -->
- [ ] Task: Conductor - User Manual Verification 'Phase 3' (Protocol in workflow.md)

## Phase 4: Verify

- [ ] Task 4.1: `cargo test -p stitchd-gateway` green; admin `tsc`/`lint`/vitest/
      `build` green. Update learnings; file any follow-ups (pause/resume/edit).
- [ ] Task: Conductor - User Manual Verification 'Phase 4' (Protocol in workflow.md)
