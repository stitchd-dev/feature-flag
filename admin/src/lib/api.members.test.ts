/**
 * Unit tests for the org-member management API wrappers in
 * `admin/src/lib/api.ts` (members_roles_20260610, Phase 1).
 *
 * These wrap the ORG-SCOPED management routes
 * (`/v1/management/orgs/{org_id}/users`), NOT the superadmin routes — the
 * Members page runs in a non-system org context.
 *
 * Mirrors `api.lifecycle.test.ts`: `axios.create` is mocked before importing
 * `api.ts` so the wrappers record calls into a shared spy client.
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

const { listOrgMembers, createOrgMember, removeOrgMember } = await import('./api')

beforeEach(() => {
  spyClient.get.mockReset()
  spyClient.post.mockReset()
  spyClient.put.mockReset()
  spyClient.patch.mockReset()
  spyClient.delete.mockReset()
})

describe('org member API wrappers', () => {
  it('listOrgMembers GETs the management org-users URL and returns the page', async () => {
    const page = {
      items: [
        {
          user_id: 'u1',
          email: 'a@example.com',
          display_name: 'Alice',
          role: 'org_admin',
          created_at: '2026-06-01T00:00:00Z',
        },
      ],
      next_cursor: null,
    }
    spyClient.get.mockResolvedValueOnce({ data: page })
    const out = await listOrgMembers('org-1')
    expect(spyClient.get.mock.calls[0][0]).toContain('/v1/management/orgs/org-1/users')
    expect(out).toEqual(page)
  })

  it('listOrgMembers forwards cursor + limit query params', async () => {
    spyClient.get.mockResolvedValueOnce({ data: { items: [], next_cursor: null } })
    await listOrgMembers('org-1', { cursor: 'abc', limit: 25 })
    const url = spyClient.get.mock.calls[0][0] as string
    expect(url).toContain('cursor=abc')
    expect(url).toContain('limit=25')
  })

  it('listOrgMembers does NOT hit the superadmin route', async () => {
    spyClient.get.mockResolvedValueOnce({ data: { items: [], next_cursor: null } })
    await listOrgMembers('org-1')
    expect(spyClient.get.mock.calls[0][0]).not.toContain('/superadmin/')
  })

  it('createOrgMember POSTs the create body to the management URL', async () => {
    const created = { user_id: 'u2', email: 'b@example.com', display_name: 'Bob' }
    spyClient.post.mockResolvedValueOnce({ data: created })
    const body = {
      email: 'b@example.com',
      display_name: 'Bob',
      password: 'hunter2pass',
      org_role: 'org_member' as const,
    }
    const out = await createOrgMember('org-1', body)
    expect(spyClient.post.mock.calls[0][0]).toBe('/v1/management/orgs/org-1/users')
    expect(spyClient.post.mock.calls[0][1]).toEqual(body)
    expect(out).toEqual(created)
  })

  it('removeOrgMember DELETEs the management user URL', async () => {
    spyClient.delete.mockResolvedValueOnce({ data: undefined })
    await removeOrgMember('org-1', 'u3')
    expect(spyClient.delete.mock.calls[0][0]).toBe('/v1/management/orgs/org-1/users/u3')
  })
})
