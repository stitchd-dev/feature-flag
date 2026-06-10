# Track Learnings: bandit_campaign_ui_20260610

## Context / audit
- Deviation: experimentation-service proto has CreateBanditCampaign/GetBanditCampaign/
  ListBanditCampaigns/StopBanditCampaign, but the GATEWAY only exposes the two GET
  routes (router.rs ~300-305); no create/stop handlers. Admin client + UI are
  read-only/absent — listBanditCampaigns is unused; no campaign UI exists.
- CreateBanditCampaignRequest = {environment_id, flag_id, name, config(JSON string)}.
  StopBanditCampaignRequest = {environment_id, campaign_id}.
- BanditCampaignConfig (core bandit/types.rs): {max_iterations:u32>=1,
  drift_threshold:f64 in (0,1), variant_discovery:"winner_plus_new"|"winner_only"
  (default winner_plus_new), budget_cap?:{max_total_units?:i64}}.
- BanditCampaign status: active|paused|completed|cancelled (Stop → cancelled).
- Gateway openapi.rs paths(...) registry + #[utoipa::path] generate the contract;
  no committed openapi.json to diff — adding annotated routes stays self-consistent.
- Gateway compiles incrementally in ~10s here; unit tests use stub state (no DB).

## State machine
- Only Stop exists (no Pause/Resume/Update RPC) → UI offers Stop on non-terminal
  campaigns only; terminal = completed|cancelled.

<!-- impl notes below -->

## Impl (2026-06-10)
- Gateway: create_bandit_campaign (POST, config serde_json::Value → to_string()) +
  stop_bandit_campaign (POST .../stop), registered in router.rs + openapi.rs
  (paths + CreateBanditCampaignBody schema). Mirrors list/get. 3 stub unit tests +
  openapi contract green; clippy -D warnings + fmt clean. (Pre-existing 3
  idempotency::pg_store_* tests fail without DATABASE_URL — unrelated; pass in CI.)
- Client: createBanditCampaign/stopBanditCampaign + BanditCampaignConfigInput.
- UI: BanditCampaignsPanel (list + create modal + stop confirm, org_admin-gated)
  mounted on ExperimentsList (reuses its flagOptions). banditCampaignHelpers
  (config builder + Yup schema + status badge + terminal check). Lint +1 warning
  (load-in-effect, matches Environments.tsx pattern).
- Follow-ups (no RPC): pause/resume campaign, edit campaign config.

## Verification note
Gateway: cargo test -p stitchd-gateway (stub, no DB) + openapi contract green;
clippy/fmt clean. Admin: tsc clean, lint 0 errors, vitest 1069 (16 new), build.
Live E2E needs full stack + a flag to spawn iterations on.
