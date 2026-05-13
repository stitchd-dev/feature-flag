import { describe, it, expect } from 'vitest'
import type { Segment } from './types'

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
