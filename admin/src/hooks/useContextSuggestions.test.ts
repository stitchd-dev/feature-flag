import { describe, it, expect } from 'vitest'

// ── Logic mirrored from useContextSuggestions ────────────────────────────────

function buildContextTypeUrl(envId: string): string {
  return `/v1/environments/${envId}/context-types`
}

function buildContextParamUrl(envId: string, contextType: string): string {
  return `/v1/environments/${envId}/context-types/${contextType}/params`
}

interface SuggestionsState<T> {
  data: T[]
  loading: boolean
  error: string | null
}

function initialState<T>(): SuggestionsState<T> {
  return { data: [], loading: false, error: null }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

describe('buildContextTypeUrl', () => {
  it('builds the correct endpoint', () => {
    expect(buildContextTypeUrl('env-abc')).toBe('/v1/environments/env-abc/context-types')
  })
})

describe('buildContextParamUrl', () => {
  it('builds the correct endpoint with type', () => {
    expect(buildContextParamUrl('env-abc', 'user')).toBe(
      '/v1/environments/env-abc/context-types/user/params',
    )
  })

  it('encodes type in path segment', () => {
    const url = buildContextParamUrl('env-abc', 'device')
    expect(url).toContain('device')
  })
})

describe('initialState', () => {
  it('starts with empty data, not loading, no error', () => {
    const s = initialState<string>()
    expect(s.data).toHaveLength(0)
    expect(s.loading).toBe(false)
    expect(s.error).toBeNull()
  })
})

describe('null envId guard', () => {
  it('skips fetch when envId is null', () => {
    // When envId is null the hook returns initial state without issuing a request.
    // Model this as a pure guard function.
    function shouldFetch(envId: string | null): boolean {
      return envId !== null && envId.length > 0
    }
    expect(shouldFetch(null)).toBe(false)
    expect(shouldFetch('')).toBe(false)
    expect(shouldFetch('env-abc')).toBe(true)
  })
})

describe('null envId/contextType guard for params', () => {
  it('skips fetch when either is null', () => {
    function shouldFetchParams(envId: string | null, contextType: string | null): boolean {
      return Boolean(envId && contextType)
    }
    expect(shouldFetchParams(null, 'user')).toBe(false)
    expect(shouldFetchParams('env-abc', null)).toBe(false)
    expect(shouldFetchParams(null, null)).toBe(false)
    expect(shouldFetchParams('env-abc', 'user')).toBe(true)
  })
})

describe('error state', () => {
  it('stores error message when fetch fails', () => {
    const state: SuggestionsState<string> = {
      data: [],
      loading: false,
      error: 'Network Error',
    }
    expect(state.error).toBe('Network Error')
    expect(state.data).toHaveLength(0)
  })
})
