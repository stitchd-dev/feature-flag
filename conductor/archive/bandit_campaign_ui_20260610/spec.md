# Spec: Bandit Campaign Management (gateway slice + UI)

**Track ID:** `bandit_campaign_ui_20260610`
**Type:** Feature (capability gap; gateway behind the experimentation-service)
**Beads:** `feature-flag-j38` (wisp `feature-flag-wisp-99m`)

## Overview

The experimentation-service exposes the full bandit-campaign lifecycle —
`CreateBanditCampaign`, `GetBanditCampaign`, `ListBanditCampaigns`,
`StopBanditCampaign` (proto `experimentation_service.proto`). But:

- The **gateway only exposes the two GET routes** (`list_bandit_campaigns`,
  `get_bandit_campaign`); there are **no create/stop handlers** — a
  gateway-behind-service deviation.
- The **admin client** (`admin/src/lib/api/bandit.ts`) only has
  `listBanditCampaigns`/`getBanditCampaign` (read-only).
- **No UI renders campaigns at all** — `listBanditCampaigns` is unused; there is
  no way to create, view, or stop an autonomous optimization campaign from the
  console.

This track closes the gap end-to-end: expose create/stop on the gateway, add the
client methods, and add a campaign-management UI surface (env-scoped) so an
operator can create, list, and stop bandit campaigns.

`BanditCampaignConfig` (stitchd-core `bandit/types.rs`) is JSON-carried:
`{ max_iterations: u32 (≥1), drift_threshold: f64 (0,1), variant_discovery:
"winner_plus_new"|"winner_only", budget_cap?: { max_total_units?: i64 } }`.
`CreateBanditCampaignRequest` = `{ environment_id, flag_id, name, config (JSON
string) }`. `StopBanditCampaignRequest` = `{ environment_id, campaign_id }`.

## Functional Requirements

### FR1 — Gateway: expose create + stop
- `POST /v1/environments/{environment_id}/bandit-campaigns` — body
  `{ flag_id, name, config: <object> }` → `CreateBanditCampaign` (config
  serialised to a JSON string). Returns `BanditCampaignJson`.
- `POST /v1/environments/{environment_id}/bandit-campaigns/{campaign_id}/stop`
  → `StopBanditCampaign`. Returns `BanditCampaignJson`.
- Add `#[utoipa::path]` annotations and register both in `openapi.rs` `paths(...)`.
- Mirror the existing list/get handler conventions (`GatewayError::from`,
  `campaign_to_json`). Gateway unit tests (stub state) cover the new routes
  (status `200`/`502` smoke + request mapping where practical).

### FR2 — Admin client: create + stop
- `createBanditCampaign(envId, { flag_id, name, config })` → POST, returns the
  `BanditCampaign`. Typed `BanditCampaignConfig` input.
- `stopBanditCampaign(envId, campaignId)` → POST `/stop`, returns the campaign.
- Unit tests (mock axios) asserting URL/method/body + config passthrough.

### FR3 — UI: campaign management surface
- A `BanditCampaignsPanel` on the Experiments page (env-scoped, below the
  experiments table) listing campaigns: name, bound flag, status badge,
  iterations spawned, config summary (max iterations / drift / discovery).
- **New campaign** (modal, org_admin-gated): flag picker (from the env's flags),
  name, `max_iterations` (≥1), `drift_threshold` (0–1), `variant_discovery`
  select, optional `max_total_units` budget cap. Builds a valid
  `BanditCampaignConfig` and calls `createBanditCampaign`; refresh + surface
  errors.
- **Stop** action per non-terminal campaign (confirm dialog, org_admin-gated) →
  `stopBanditCampaign`; refresh.
- Loading / empty / error states. Empty state explains what a campaign is.

## Non-Functional Requirements

- **TDD:** gateway unit tests; admin client unit tests; pure config-builder +
  presentational render tests (node-env conventions).
- **Backend gate:** `cargo test -p stitchd-gateway` (stub-based, no DB) green;
  the generated OpenAPI stays self-consistent (contract test passes).
- **Frontend gate:** `tsc`, `lint` (0 errors), vitest, `build`.
- Match existing gateway + admin conventions; no new deps.

## Acceptance Criteria

1. Gateway serves create + stop; both appear in the generated OpenAPI; gateway
   tests green.
2. Admin client `createBanditCampaign`/`stopBanditCampaign` POST correctly.
3. The Experiments page renders a campaigns panel; an operator can create a
   campaign, see it listed, and stop it (UI gated on org_admin).
4. No fabricated data; loading/empty/error states present.
5. `cargo test -p stitchd-gateway` green; admin tsc/lint/vitest/build green.

## Out of Scope

- Pausing/resuming campaigns (no Pause/Resume RPC — only Stop exists).
- Editing a campaign's config after creation (no Update RPC).
- Deep per-campaign drill-down beyond what `GetBanditCampaign` returns.
- A dedicated sidebar nav entry / route (panel lives on the Experiments page).
