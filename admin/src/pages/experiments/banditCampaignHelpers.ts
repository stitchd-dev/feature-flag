/**
 * Pure helpers + validation for the bandit campaign management panel.
 *
 * BanditCampaignConfig (stitchd-core bandit/types.rs): max_iterations (>=1),
 * drift_threshold (0,1), variant_discovery, optional budget_cap.max_total_units.
 */
import * as Yup from 'yup'
import type { BanditCampaignConfigInput, VariantDiscoveryPolicy } from '../../lib/api/bandit'

export interface CampaignFormValues {
  flag_id: string
  name: string
  max_iterations: number
  drift_threshold: number
  variant_discovery: VariantDiscoveryPolicy
  /** Optional budget cap; blank string = uncapped. */
  max_total_units: string
}

export const VARIANT_DISCOVERY_OPTIONS: { value: VariantDiscoveryPolicy; label: string; desc: string }[] = [
  { value: 'winner_plus_new', label: 'Winner + new', desc: 'Carry the winner forward as control plus any newly-registered variants' },
  { value: 'winner_only', label: 'Winner only', desc: 'Carry only the winner forward (no auto-discovery)' },
]

export const campaignConfigSchema = Yup.object({
  flag_id: Yup.string().required('Select a flag'),
  name: Yup.string().trim().min(1, 'Name is required').max(120).required('Name is required'),
  max_iterations: Yup.number().integer('Must be a whole number').min(1, 'At least 1 iteration').required('Required'),
  drift_threshold: Yup.number().moreThan(0, 'Must be > 0').lessThan(1, 'Must be < 1').required('Required'),
  variant_discovery: Yup.string().oneOf(['winner_plus_new', 'winner_only']).required(),
  max_total_units: Yup.string().test('uint', 'Must be a positive whole number', (v) => {
    if (!v || v.trim() === '') return true
    const n = Number(v)
    return Number.isInteger(n) && n >= 1
  }),
})

/** Build the BanditCampaignConfig payload from validated form values. */
export function buildCampaignConfig(v: CampaignFormValues): BanditCampaignConfigInput {
  const config: BanditCampaignConfigInput = {
    max_iterations: Number(v.max_iterations),
    drift_threshold: Number(v.drift_threshold),
    variant_discovery: v.variant_discovery,
  }
  const cap = v.max_total_units?.trim()
  if (cap) config.budget_cap = { max_total_units: Number(cap) }
  return config
}

export function campaignStatusBadge(status: string): { label: string; className: string } {
  switch (status) {
    case 'active':
      return { label: 'active', className: 'badge success' }
    case 'paused':
      return { label: 'paused', className: 'badge warning' }
    case 'completed':
      return { label: 'completed', className: 'badge' }
    case 'cancelled':
      return { label: 'cancelled', className: 'badge' }
    default:
      return { label: status, className: 'badge' }
  }
}

/** Completed / cancelled campaigns are terminal — no Stop action. */
export function isTerminalCampaign(status: string): boolean {
  return status === 'completed' || status === 'cancelled'
}

/** A short human summary of a campaign's config JSON for the list row. */
export function campaignConfigSummary(config: unknown): string {
  if (!config || typeof config !== 'object') return '—'
  const c = config as Record<string, unknown>
  const parts: string[] = []
  if (typeof c.max_iterations === 'number') parts.push(`${c.max_iterations} iters`)
  if (typeof c.drift_threshold === 'number') parts.push(`drift ${c.drift_threshold}`)
  if (typeof c.variant_discovery === 'string') parts.push(String(c.variant_discovery).replace(/_/g, ' '))
  return parts.length > 0 ? parts.join(' · ') : '—'
}
