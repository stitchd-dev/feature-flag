/**
 * Unit tests for the auth-provider API wrappers in `admin/src/lib/api.ts`
 * (members_roles_20260610, Phase 4).
 *
 * These pin the wire shapes against the gateway `auth_providers` routes:
 * responses use `id` + `enabled`, the SAML-metadata endpoint returns `{ xml }`,
 * and create/update bodies are tagged by `provider_type`.
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

const {
  listAuthProviders,
  createAuthProvider,
  updateAuthProvider,
  deleteAuthProvider,
  getSamlSpMetadata,
} = await import('./api')

beforeEach(() => {
  spyClient.get.mockReset()
  spyClient.post.mockReset()
  spyClient.put.mockReset()
  spyClient.patch.mockReset()
  spyClient.delete.mockReset()
})

describe('auth-provider API wrappers', () => {
  it('listAuthProviders returns the bare array with id + enabled fields', async () => {
    const providers = [
      { id: 'p1', org_id: 'o1', provider_type: 'oidc', display_name: 'Okta', enabled: true, acs_url: null, oidc: { issuer_url: 'https://okta', client_id: 'c' }, saml: null, created_at: '', updated_at: '' },
    ]
    spyClient.get.mockResolvedValueOnce({ data: providers })
    const out = await listAuthProviders('o1')
    expect(spyClient.get.mock.calls[0][0]).toBe('/v1/orgs/o1/auth-providers')
    expect(out[0].id).toBe('p1')
    expect(out[0].enabled).toBe(true)
  })

  it('createAuthProvider POSTs the provider_type-tagged body', async () => {
    spyClient.post.mockResolvedValueOnce({ data: { id: 'p2' } })
    await createAuthProvider('o1', {
      provider_type: 'oidc',
      display_name: 'Okta',
      config: { issuer_url: 'https://okta', client_id: 'c', client_secret: 's', scopes: ['openid'] },
    })
    expect(spyClient.post.mock.calls[0][0]).toBe('/v1/orgs/o1/auth-providers')
    expect(spyClient.post.mock.calls[0][1].provider_type).toBe('oidc')
    expect(spyClient.post.mock.calls[0][1].config.client_secret).toBe('s')
  })

  it('updateAuthProvider PUTs to the provider URL', async () => {
    spyClient.put.mockResolvedValueOnce({ data: { id: 'p2' } })
    await updateAuthProvider('o1', 'p2', { enabled: false })
    expect(spyClient.put.mock.calls[0][0]).toBe('/v1/orgs/o1/auth-providers/p2')
    expect(spyClient.put.mock.calls[0][1]).toEqual({ enabled: false })
  })

  it('deleteAuthProvider DELETEs the provider URL', async () => {
    spyClient.delete.mockResolvedValueOnce({ data: undefined })
    await deleteAuthProvider('o1', 'p2')
    expect(spyClient.delete.mock.calls[0][0]).toBe('/v1/orgs/o1/auth-providers/p2')
  })

  it('getSamlSpMetadata unwraps the { xml } envelope into a string', async () => {
    spyClient.get.mockResolvedValueOnce({ data: { xml: '<EntityDescriptor/>' } })
    const xml = await getSamlSpMetadata('o1', 'p2')
    expect(spyClient.get.mock.calls[0][0]).toBe('/v1/orgs/o1/auth-providers/p2/saml/metadata')
    expect(xml).toBe('<EntityDescriptor/>')
  })
})
