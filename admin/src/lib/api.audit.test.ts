/**
 * Unit tests for the audit-log API wrapper (audit_log_20260611).
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

const { listAuditLog } = await import('./api')

beforeEach(() => {
  spyClient.get.mockReset()
})

describe('listAuditLog', () => {
  it('GETs the org-scoped audit URL and returns the page', async () => {
    const page = { items: [{ id: 'a1', resource_type: 'flag', action: 'flag.update', created_at: '2026-06-11T00:00:00Z' }], next_cursor: null }
    spyClient.get.mockResolvedValueOnce({ data: page })
    const out = await listAuditLog('org-1')
    expect(spyClient.get.mock.calls[0][0]).toContain('/v1/orgs/org-1/audit')
    expect(out).toEqual(page)
  })

  it('forwards cursor / limit / resource_type / action filters', async () => {
    spyClient.get.mockResolvedValueOnce({ data: { items: [], next_cursor: null } })
    await listAuditLog('org-1', { cursor: 'abc', limit: 25, resource_type: 'flag', action: 'flag.update' })
    const url = spyClient.get.mock.calls[0][0] as string
    expect(url).toContain('cursor=abc')
    expect(url).toContain('limit=25')
    expect(url).toContain('resource_type=flag')
    expect(url).toContain('action=flag.update')
  })

  it('omits empty filters', async () => {
    spyClient.get.mockResolvedValueOnce({ data: { items: [], next_cursor: null } })
    await listAuditLog('org-1', { resource_type: '' })
    const url = spyClient.get.mock.calls[0][0] as string
    expect(url).not.toContain('resource_type=')
  })
})
