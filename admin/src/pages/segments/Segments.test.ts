import { describe, it, expect } from 'vitest'
import type { Segment } from './types'
import { isListSegment, getListCounts } from './helpers'

// Helper: parse tag input string (comma-separated) into array
function parseTagsInput(input: string): string[] {
  return input
    .split(',')
    .map((t) => t.trim())
    .filter((t) => t.length > 0)
}

// Helper: parse user list input (newline-separated) into array
function parseUserListInput(input: string): string[] {
  return input
    .split('\n')
    .map((u) => u.trim())
    .filter((u) => u.length > 0)
}

// Helper: filter segments by search term
function filterSegments(segments: Segment[], search: string): Segment[] {
  if (!search) return segments
  const q = search.toLowerCase()
  return segments.filter(
    (s) =>
      s.name.toLowerCase().includes(q) ||
      (s.description ?? '').toLowerCase().includes(q) ||
      s.tags.some((t) => t.toLowerCase().includes(q)),
  )
}

describe('parseTagsInput', () => {
  it('splits comma-separated tags', () => {
    expect(parseTagsInput('beta, internal, us-only')).toEqual(['beta', 'internal', 'us-only'])
  })

  it('filters empty entries', () => {
    expect(parseTagsInput('beta,  ,internal')).toEqual(['beta', 'internal'])
  })

  it('returns empty array for blank input', () => {
    expect(parseTagsInput('')).toEqual([])
    expect(parseTagsInput('  ,  ')).toEqual([])
  })

  it('handles single tag with no commas', () => {
    expect(parseTagsInput('beta')).toEqual(['beta'])
  })
})

describe('parseUserListInput', () => {
  it('splits newline-separated user keys', () => {
    expect(parseUserListInput('user1\nuser2\nuser3')).toEqual(['user1', 'user2', 'user3'])
  })

  it('trims whitespace from each line', () => {
    expect(parseUserListInput('  user1  \n  user2  ')).toEqual(['user1', 'user2'])
  })

  it('filters empty lines', () => {
    expect(parseUserListInput('user1\n\nuser2')).toEqual(['user1', 'user2'])
  })

  it('returns empty array for blank input', () => {
    expect(parseUserListInput('')).toEqual([])
    expect(parseUserListInput('\n\n')).toEqual([])
  })
})

describe('filterSegments', () => {
  const makeSegment = (partial: Partial<Segment> & { name: string }): Segment => ({
    id: 'seg-1',
    name: partial.name,
    description: partial.description,
    tags: partial.tags ?? [],
    condition_expr: undefined,
    user_list: [],
    include_count: partial.include_count ?? 0,
    exclude_count: partial.exclude_count ?? 0,
    condition_count: 0,
    version: 1,
    created_at: '2026-01-01T00:00:00Z',
    updated_at: '2026-01-01T00:00:00Z',
  })

  it('returns all segments when search is empty', () => {
    const segments = [makeSegment({ name: 'Alpha' }), makeSegment({ name: 'Beta' })]
    expect(filterSegments(segments, '')).toHaveLength(2)
  })

  it('filters by name (case-insensitive)', () => {
    const segments = [makeSegment({ name: 'Alpha Users' }), makeSegment({ name: 'Beta Testers' })]
    expect(filterSegments(segments, 'alpha')).toEqual([segments[0]])
  })

  it('filters by description', () => {
    const segments = [
      makeSegment({ name: 'Segment A', description: 'internal beta users' }),
      makeSegment({ name: 'Segment B', description: 'external customers' }),
    ]
    expect(filterSegments(segments, 'beta')).toEqual([segments[0]])
  })

  it('filters by tag', () => {
    const segments = [
      makeSegment({ name: 'Segment A', tags: ['beta', 'internal'] }),
      makeSegment({ name: 'Segment B', tags: ['production'] }),
    ]
    expect(filterSegments(segments, 'internal')).toEqual([segments[0]])
  })

  it('returns empty array when no match', () => {
    const segments = [makeSegment({ name: 'Alpha' }), makeSegment({ name: 'Beta' })]
    expect(filterSegments(segments, 'gamma')).toHaveLength(0)
  })
})

// ─── Helpers from ./helpers ───────────────────────────────────────────────────

function makeSegment2(partial: Partial<Segment> & { name: string }): Segment {
  return {
    id: 'seg-2',
    tags: [],
    user_list: [],
    include_count: 0,
    exclude_count: 0,
    condition_count: 0,
    version: 1,
    created_at: '2026-01-01T00:00:00Z',
    updated_at: '2026-01-01T00:00:00Z',
    ...partial,
  }
}

describe('isListSegment', () => {
  it('returns true when segment_type is list', () => {
    const seg = makeSegment2({ name: 'S', segment_type: 'list', context_type: 'user' })
    expect(isListSegment(seg)).toBe(true)
  })

  it('returns false when segment_type is rule', () => {
    const seg = makeSegment2({ name: 'S', segment_type: 'rule' })
    expect(isListSegment(seg)).toBe(false)
  })

  it('returns false when segment_type is absent', () => {
    const seg = makeSegment2({ name: 'S' })
    expect(isListSegment(seg)).toBe(false)
  })

  it('ignores user_list — deprecated, never set by server', () => {
    // user_list is always [] from the server; type detection must NOT use it
    const seg = makeSegment2({ name: 'S', segment_type: 'rule', user_list: ['u1', 'u2'] })
    expect(isListSegment(seg)).toBe(false)
  })
})

describe('getListCounts', () => {
  it('returns include_count and exclude_count from the segment', () => {
    const seg = makeSegment2({ name: 'S', segment_type: 'list', include_count: 42, exclude_count: 7 })
    expect(getListCounts(seg)).toEqual({ includeCount: 42, excludeCount: 7 })
  })

  it('returns zero counts for a newly created list segment', () => {
    const seg = makeSegment2({ name: 'S', segment_type: 'list', include_count: 0, exclude_count: 0 })
    expect(getListCounts(seg)).toEqual({ includeCount: 0, excludeCount: 0 })
  })

  it('works for rule segments too (counts are 0)', () => {
    const seg = makeSegment2({ name: 'S', segment_type: 'rule' })
    expect(getListCounts(seg)).toEqual({ includeCount: 0, excludeCount: 0 })
  })
})
