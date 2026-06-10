/**
 * Unit tests for the bandit campaign create/stop client wrappers
 * (bandit_campaign_ui_20260610, Phase 2). Mocks axios.create before importing.
 */
import { describe, it, expect, vi, beforeEach } from 'vitest'

interface SpyClient {
  get: ReturnType<typeof vi.fn>
  post: ReturnType<typeof vi.fn>
  put: ReturnType<typeof vi.fn>
  patch: ReturnType<typeof vi.fn>
  delete: ReturnType<typeof vi.fn>
  interceptors: { request: { use: () => void }; response: { use: () => void } }
  defaults: { baseURL: string }
}

const spyClient: SpyClient = {
  get: vi.fn(),
  post: vi.fn(),
  put: vi.fn(),
  patch: vi.fn(),
  delete: vi.fn(),
  interceptors: { request: { use: () => undefined }, response: { use: () => undefined } },
  defaults: { baseURL: '/api' },
}

vi.mock('axios', () => ({ default: { create: () => spyClient, post: vi.fn() } }))

const { createBanditCampaign, stopBanditCampaign } = await import('./bandit')

beforeEach(() => {
  spyClient.get.mockReset()
  spyClient.post.mockReset()
})

describe('createBanditCampaign', () => {
  it('POSTs { flag_id, name, config } to the env campaigns URL', async () => {
    spyClient.post.mockResolvedValueOnce({ data: { id: 'c1', status: 'active' } })
    const out = await createBanditCampaign('env-1', {
      flag_id: 'flag-1',
      name: 'Checkout opt',
      config: { max_iterations: 5, drift_threshold: 0.1, variant_discovery: 'winner_plus_new' },
    })
    expect(spyClient.post.mock.calls[0][0]).toBe('/v1/environments/env-1/bandit-campaigns')
    expect(spyClient.post.mock.calls[0][1]).toEqual({
      flag_id: 'flag-1',
      name: 'Checkout opt',
      config: { max_iterations: 5, drift_threshold: 0.1, variant_discovery: 'winner_plus_new' },
    })
    expect(out.id).toBe('c1')
  })

  it('encodes the environment id', async () => {
    spyClient.post.mockResolvedValueOnce({ data: { id: 'c1' } })
    await createBanditCampaign('env/1', { flag_id: 'f', name: 'n', config: { max_iterations: 1, drift_threshold: 0.2 } })
    expect(spyClient.post.mock.calls[0][0]).toContain('env%2F1')
  })
})

describe('stopBanditCampaign', () => {
  it('POSTs to the campaign /stop URL', async () => {
    spyClient.post.mockResolvedValueOnce({ data: { id: 'c1', status: 'cancelled' } })
    const out = await stopBanditCampaign('env-1', 'c1')
    expect(spyClient.post.mock.calls[0][0]).toBe('/v1/environments/env-1/bandit-campaigns/c1/stop')
    expect(out.status).toBe('cancelled')
  })

  it('propagates gateway errors', async () => {
    spyClient.post.mockRejectedValueOnce({ response: { status: 502 } })
    await expect(stopBanditCampaign('env-1', 'c1')).rejects.toMatchObject({ response: { status: 502 } })
  })
})
