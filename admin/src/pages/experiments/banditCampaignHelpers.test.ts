/**
 * Pure tests for bandit campaign helpers (bandit_campaign_ui_20260610, Phase 3).
 */
import { describe, it, expect } from 'vitest'
import {
  buildCampaignConfig,
  campaignConfigSchema,
  campaignStatusBadge,
  isTerminalCampaign,
} from './banditCampaignHelpers'

describe('buildCampaignConfig', () => {
  it('builds the minimal config without a budget cap', () => {
    const cfg = buildCampaignConfig({
      flag_id: 'f', name: 'n', max_iterations: 5, drift_threshold: 0.1,
      variant_discovery: 'winner_plus_new', max_total_units: '',
    })
    expect(cfg).toEqual({ max_iterations: 5, drift_threshold: 0.1, variant_discovery: 'winner_plus_new' })
    expect('budget_cap' in cfg).toBe(false)
  })

  it('includes budget_cap when max_total_units is provided', () => {
    const cfg = buildCampaignConfig({
      flag_id: 'f', name: 'n', max_iterations: 3, drift_threshold: 0.2,
      variant_discovery: 'winner_only', max_total_units: '100000',
    })
    expect(cfg.budget_cap).toEqual({ max_total_units: 100000 })
    expect(cfg.variant_discovery).toBe('winner_only')
  })
})

describe('campaignConfigSchema', () => {
  const base = { flag_id: 'f', name: 'Camp', max_iterations: 5, drift_threshold: 0.1, variant_discovery: 'winner_plus_new', max_total_units: '' }
  it('accepts a valid form', async () => {
    await expect(campaignConfigSchema.validate(base)).resolves.toBeTruthy()
  })
  it('requires a flag', async () => {
    await expect(campaignConfigSchema.validate({ ...base, flag_id: '' })).rejects.toThrow()
  })
  it('requires max_iterations >= 1', async () => {
    await expect(campaignConfigSchema.validate({ ...base, max_iterations: 0 })).rejects.toThrow()
  })
  it('requires drift_threshold strictly between 0 and 1', async () => {
    await expect(campaignConfigSchema.validate({ ...base, drift_threshold: 0 })).rejects.toThrow()
    await expect(campaignConfigSchema.validate({ ...base, drift_threshold: 1 })).rejects.toThrow()
    await expect(campaignConfigSchema.validate({ ...base, drift_threshold: 0.5 })).resolves.toBeTruthy()
  })
})

describe('campaignStatusBadge', () => {
  it('maps known statuses', () => {
    expect(campaignStatusBadge('active').className).toContain('success')
    expect(campaignStatusBadge('cancelled').label).toBe('cancelled')
  })
})

describe('isTerminalCampaign', () => {
  it('treats completed/cancelled as terminal', () => {
    expect(isTerminalCampaign('completed')).toBe(true)
    expect(isTerminalCampaign('cancelled')).toBe(true)
    expect(isTerminalCampaign('active')).toBe(false)
    expect(isTerminalCampaign('paused')).toBe(false)
  })
})
