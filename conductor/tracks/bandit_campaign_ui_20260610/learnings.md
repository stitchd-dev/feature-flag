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
