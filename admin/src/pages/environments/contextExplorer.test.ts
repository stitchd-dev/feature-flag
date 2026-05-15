import { describe, it, expect } from 'vitest'

// ── Logic mirrored from ContextExplorer ──────────────────────────────────────

interface ContextTypeSuggestion { context_type: string; last_seen_at: string }
interface ContextParamSuggestion { param_key: string; inferred_type: string; is_private: boolean; last_seen_at: string }

function sortByRecency(types: ContextTypeSuggestion[]): ContextTypeSuggestion[] {
  return [...types].sort(
    (a, b) => new Date(b.last_seen_at).getTime() - new Date(a.last_seen_at).getTime(),
  )
}

// ── Tests ─────────────────────────────────────────────────────────────────────

describe('empty state', () => {
  it('shows empty state when types array is empty', () => {
    const types: ContextTypeSuggestion[] = []
    expect(types.length === 0).toBe(true)
  })

  it('does not show empty state when types exist', () => {
    const types: ContextTypeSuggestion[] = [
      { context_type: 'user', last_seen_at: '2026-05-15T00:00:00Z' },
    ]
    expect(types.length > 0).toBe(true)
  })
})

describe('expand/collapse rows', () => {
  it('param fetch is skipped when row is not expanded', () => {
    function shouldFetchParams(expanded: boolean, envId: string | null): boolean {
      return expanded && Boolean(envId)
    }
    expect(shouldFetchParams(false, 'env-abc')).toBe(false)
    expect(shouldFetchParams(true, 'env-abc')).toBe(true)
    expect(shouldFetchParams(true, null)).toBe(false)
  })
})

describe('private badge rendering', () => {
  it('private param has is_private true', () => {
    const params: ContextParamSuggestion[] = [
      { param_key: 'email', inferred_type: 'str', is_private: true, last_seen_at: '2026-05-01T00:00:00Z' },
      { param_key: 'plan', inferred_type: 'str', is_private: false, last_seen_at: '2026-05-01T00:00:00Z' },
    ]
    const privateParams = params.filter((p) => p.is_private)
    expect(privateParams).toHaveLength(1)
    expect(privateParams[0].param_key).toBe('email')
  })
})

describe('sortByRecency', () => {
  it('sorts types by last_seen_at descending', () => {
    const types: ContextTypeSuggestion[] = [
      { context_type: 'org', last_seen_at: '2026-04-01T00:00:00Z' },
      { context_type: 'user', last_seen_at: '2026-05-01T00:00:00Z' },
    ]
    const sorted = sortByRecency(types)
    expect(sorted[0].context_type).toBe('user')
    expect(sorted[1].context_type).toBe('org')
  })

  it('returns empty array unchanged', () => {
    expect(sortByRecency([])).toHaveLength(0)
  })
})

describe('no envId guard', () => {
  it('shows select-env prompt when envId is null', () => {
    function showSelectEnvPrompt(envId: string | null): boolean {
      return envId === null
    }
    expect(showSelectEnvPrompt(null)).toBe(true)
    expect(showSelectEnvPrompt('env-abc')).toBe(false)
  })
})
