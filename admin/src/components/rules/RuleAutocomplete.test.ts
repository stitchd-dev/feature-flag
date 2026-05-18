import { describe, it, expect } from 'vitest'

// ── Logic mirrored from ConditionClauseEditor autocomplete wiring ─────────────

interface ContextTypeSuggestion { context_type: string; last_seen_at: string }
interface ContextParamSuggestion { param_key: string; inferred_type: string; is_private: boolean; last_seen_at: string }
interface SuggestionItem { label: string; isPrivate?: boolean }

function buildTypeSuggestions(data: ContextTypeSuggestion[]): SuggestionItem[] {
  return data.map((t) => ({ label: t.context_type }))
}

function buildParamSuggestions(data: ContextParamSuggestion[]): SuggestionItem[] {
  return data.map((p) => ({ label: p.param_key, isPrivate: p.is_private }))
}

// Extracts context_type from a condition payload (mirrors extractContextType)
type Condition = Record<string, unknown>
function extractContextType(condition: Condition): string | null {
  const CONTEXT_OPS = ['Eq', 'Ne', 'Lt', 'Lte', 'Gt', 'Gte', 'Contains', 'StartsWith', 'EndsWith', 'SemverGte', 'SemverTilde', 'SemverCaret']
  const op = Object.keys(condition)[0]
  if (!CONTEXT_OPS.includes(op)) return null
  const payload = condition[op] as { context_type?: string }
  return payload?.context_type ?? null
}

// ── Tests ─────────────────────────────────────────────────────────────────────

describe('buildTypeSuggestions', () => {
  it('maps context type data to SuggestionItems', () => {
    const data: ContextTypeSuggestion[] = [
      { context_type: 'user', last_seen_at: '2026-05-01T00:00:00Z' },
      { context_type: 'device', last_seen_at: '2026-05-01T00:00:00Z' },
    ]
    const items = buildTypeSuggestions(data)
    expect(items).toHaveLength(2)
    expect(items[0].label).toBe('user')
    expect(items[1].label).toBe('device')
    expect(items[0].isPrivate).toBeUndefined()
  })

  it('returns empty when no data', () => {
    expect(buildTypeSuggestions([])).toHaveLength(0)
  })
})

describe('buildParamSuggestions', () => {
  it('preserves isPrivate flag from param data', () => {
    const data: ContextParamSuggestion[] = [
      { param_key: 'email', inferred_type: 'str', is_private: true, last_seen_at: '2026-05-01T00:00:00Z' },
      { param_key: 'plan', inferred_type: 'str', is_private: false, last_seen_at: '2026-05-01T00:00:00Z' },
    ]
    const items = buildParamSuggestions(data)
    expect(items).toHaveLength(2)
    expect(items[0].label).toBe('email')
    expect(items[0].isPrivate).toBe(true)
    expect(items[1].label).toBe('plan')
    expect(items[1].isPrivate).toBe(false)
  })
})

describe('extractContextType', () => {
  it('returns context_type from Eq condition', () => {
    const cond: Condition = { Eq: { context_type: 'user', param: 'plan', value: 'pro' } }
    expect(extractContextType(cond)).toBe('user')
  })

  it('returns null for InSegment condition (no context_type)', () => {
    const cond: Condition = { InSegment: 'seg-123' }
    expect(extractContextType(cond)).toBeNull()
  })

  it('returns null for FlagEvaluatedAs condition', () => {
    const cond: Condition = { FlagEvaluatedAs: { flag_id: 'f1', variant_id: 'v1' } }
    expect(extractContextType(cond)).toBeNull()
  })

  it('returns context_type from Contains condition', () => {
    const cond: Condition = { Contains: { context_type: 'device', param: 'ua', substr: 'Chrome' } }
    expect(extractContextType(cond)).toBe('device')
  })

  it('returns context_type from SemverGte condition', () => {
    const cond: Condition = { SemverGte: { context_type: 'app', param: 'version', value: '2.0.0' } }
    expect(extractContextType(cond)).toBe('app')
  })
})

describe('param suggestions follow context type', () => {
  it('param suggestions scope to the selected context type', () => {
    const userParams: ContextParamSuggestion[] = [
      { param_key: 'plan', inferred_type: 'str', is_private: false, last_seen_at: '2026-05-01T00:00:00Z' },
    ]
    const deviceParams: ContextParamSuggestion[] = [
      { param_key: 'platform', inferred_type: 'str', is_private: false, last_seen_at: '2026-05-01T00:00:00Z' },
    ]
    const userItems = buildParamSuggestions(userParams)
    const deviceItems = buildParamSuggestions(deviceParams)
    expect(userItems[0].label).toBe('plan')
    expect(deviceItems[0].label).toBe('platform')
  })
})
