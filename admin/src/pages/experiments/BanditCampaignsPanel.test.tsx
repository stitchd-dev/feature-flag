/**
 * CampaignsTable — presentational render tests (Phase 3). Node env: renderToString.
 */
import { describe, it, expect } from 'vitest'
import { renderToString } from 'react-dom/server'
import { CampaignsTable, type FlagOption } from './BanditCampaignsPanel'
import type { BanditCampaign } from '../../lib/api/bandit'

const flags: FlagOption[] = [{ flag_id: 'fid-1', key: 'checkout-flow', name: 'Checkout' }]

const campaigns: BanditCampaign[] = [
  { id: 'c1', environment_id: 'e', flag_id: 'fid-1', name: 'Checkout opt', config: { max_iterations: 5, drift_threshold: 0.1, variant_discovery: 'winner_plus_new' }, status: 'active', iterations_spawned: 2, version: 1 },
  { id: 'c2', environment_id: 'e', flag_id: 'fid-1', name: 'Done campaign', config: { max_iterations: 3, drift_threshold: 0.2 }, status: 'completed', iterations_spawned: 3, version: 4 },
]

const noop = () => {}

describe('CampaignsTable', () => {
  it('renders a row per campaign with name, resolved flag key, status, iterations', () => {
    const html = renderToString(<CampaignsTable campaigns={campaigns} flags={flags} canManage={false} onStop={noop} />)
    expect(html).toMatch(/Checkout opt/)
    expect(html).toMatch(/checkout-flow/)
    expect(html).toMatch(/active/)
    expect(html).toMatch(/completed/)
  })

  it('summarises the config', () => {
    const html = renderToString(<CampaignsTable campaigns={campaigns} flags={flags} canManage={false} onStop={noop} />)
    expect(html).toMatch(/5 iters/)
    expect(html).toMatch(/drift 0\.1/)
  })

  it('shows Stop only for non-terminal campaigns when manageable', () => {
    const html = renderToString(<CampaignsTable campaigns={campaigns} flags={flags} canManage onStop={noop} />)
    // exactly one Stop button (active row), not the completed row
    const count = (html.match(/Stop/g) ?? []).length
    expect(count).toBe(1)
  })

  it('hides Stop entirely when the viewer cannot manage', () => {
    const html = renderToString(<CampaignsTable campaigns={campaigns} flags={flags} canManage={false} onStop={noop} />)
    expect(html).not.toMatch(/Stop/)
  })
})
