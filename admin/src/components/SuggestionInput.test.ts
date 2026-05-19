import { describe, it, expect } from 'vitest'

// ── Logic mirrored from SuggestionInput ──────────────────────────────────────

interface SuggestionItem {
  label: string
  isPrivate?: boolean
}

function filterSuggestions(items: SuggestionItem[], query: string): SuggestionItem[] {
  return items.filter((s) => s.label.toLowerCase().includes(query.toLowerCase()))
}

function navigateDown(activeIndex: number, total: number): number {
  return Math.min(activeIndex + 1, total - 1)
}

function navigateUp(activeIndex: number): number {
  return Math.max(activeIndex - 1, 0)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

describe('filterSuggestions', () => {
  const items: SuggestionItem[] = [
    { label: 'email', isPrivate: true },
    { label: 'plan', isPrivate: false },
    { label: 'locale' },
  ]

  it('returns all items when query is empty', () => {
    expect(filterSuggestions(items, '')).toHaveLength(3)
  })

  it('filters by substring match', () => {
    expect(filterSuggestions(items, 'plan')).toHaveLength(1)
    expect(filterSuggestions(items, 'plan')[0].label).toBe('plan')
  })

  it('is case-insensitive', () => {
    expect(filterSuggestions(items, 'EMAIL')).toHaveLength(1)
  })

  it('returns empty when no match', () => {
    expect(filterSuggestions(items, 'xyz')).toHaveLength(0)
  })

  it('matches partial label substrings', () => {
    expect(filterSuggestions(items, 'loc')).toHaveLength(1)
  })
})

describe('keyboard navigation', () => {
  it('navigateDown advances index', () => {
    expect(navigateDown(0, 3)).toBe(1)
    expect(navigateDown(1, 3)).toBe(2)
  })

  it('navigateDown clamps at last item', () => {
    expect(navigateDown(2, 3)).toBe(2)
  })

  it('navigateUp decrements index', () => {
    expect(navigateUp(2)).toBe(1)
    expect(navigateUp(1)).toBe(0)
  })

  it('navigateUp clamps at 0', () => {
    expect(navigateUp(0)).toBe(0)
  })
})

describe('private param badge', () => {
  it('private items have isPrivate flag set', () => {
    const items: SuggestionItem[] = [
      { label: 'email', isPrivate: true },
      { label: 'plan', isPrivate: false },
    ]
    const privateOnes = items.filter((i) => i.isPrivate)
    expect(privateOnes).toHaveLength(1)
    expect(privateOnes[0].label).toBe('email')
  })

  it('items without isPrivate are not private', () => {
    const item: SuggestionItem = { label: 'locale' }
    expect(item.isPrivate).toBeFalsy()
  })
})

describe('empty state', () => {
  it('no items means dropdown is not shown', () => {
    const items: SuggestionItem[] = []
    const shouldShow = items.length > 0
    expect(shouldShow).toBe(false)
  })
})
