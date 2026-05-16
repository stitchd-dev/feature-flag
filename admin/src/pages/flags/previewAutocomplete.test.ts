import { describe, it, expect } from 'vitest'

// ── Logic mirrored from PreviewTab FormBuilder autocomplete ───────────────────

interface ContextTypeSuggestion { context_type: string; last_seen_at: string }
interface ContextParamSuggestion { param_key: string; inferred_type: string; is_private: boolean; last_seen_at: string }
interface SuggestionItem { label: string; isPrivate?: boolean }

function buildTypeSuggestions(data: ContextTypeSuggestion[]): SuggestionItem[] {
  return data.map((t) => ({ label: t.context_type }))
}

function buildParamSuggestions(data: ContextParamSuggestion[]): SuggestionItem[] {
  return data.map((p) => ({ label: p.param_key, isPrivate: p.is_private }))
}

// ── Tests ─────────────────────────────────────────────────────────────────────

describe('preview tab _type autocomplete', () => {
  it('shows known context types as suggestions', () => {
    const typeData: ContextTypeSuggestion[] = [
      { context_type: 'user', last_seen_at: '2026-05-01T00:00:00Z' },
      { context_type: 'org', last_seen_at: '2026-05-01T00:00:00Z' },
    ]
    const suggestions = buildTypeSuggestions(typeData)
    expect(suggestions.map((s) => s.label)).toContain('user')
    expect(suggestions.map((s) => s.label)).toContain('org')
  })

  it('returns empty when no types registered', () => {
    expect(buildTypeSuggestions([])).toHaveLength(0)
  })
})

describe('preview tab param key autocomplete', () => {
  it('shows known param keys for the selected type', () => {
    const paramData: ContextParamSuggestion[] = [
      { param_key: 'plan', inferred_type: 'str', is_private: false, last_seen_at: '2026-05-01T00:00:00Z' },
      { param_key: 'email', inferred_type: 'str', is_private: true, last_seen_at: '2026-05-01T00:00:00Z' },
    ]
    const suggestions = buildParamSuggestions(paramData)
    expect(suggestions).toHaveLength(2)
    expect(suggestions[1].isPrivate).toBe(true)
  })

  it('no suggestions when _type is empty (null envId guard)', () => {
    function shouldFetch(contextType: string | null): boolean {
      return Boolean(contextType && contextType.length > 0)
    }
    expect(shouldFetch(null)).toBe(false)
    expect(shouldFetch('')).toBe(false)
    expect(shouldFetch('user')).toBe(true)
  })
})
