/**
 * Unit tests for the dependency-graph + delete-block pure helpers
 * (flag_lifecycle_20260604, Phase 8.4). Environment is `node`.
 */
import { describe, it, expect } from 'vitest'
import type { DependencyEdge } from '../../lib/types'
import {
  parseDependencyExists,
  edgeKindLabel,
  edgeLabel,
  graphCounts,
} from './dependencyHelpers'

describe('parseDependencyExists', () => {
  it('parses a structured 409 dependency_exists body', () => {
    const err = {
      response: {
        status: 409,
        data: {
          error: 'dependency_exists',
          dependents: ['flag-a', 'flag-b'],
          message: 'still referenced',
        },
      },
    }
    expect(parseDependencyExists(err)).toEqual({
      error: 'dependency_exists',
      dependents: ['flag-a', 'flag-b'],
      message: 'still referenced',
    })
  })

  it('defaults the message and coerces dependents to strings', () => {
    const err = {
      response: { status: 409, data: { error: 'dependency_exists', dependents: [1, 2] } },
    }
    const out = parseDependencyExists(err)
    expect(out?.dependents).toEqual(['1', '2'])
    expect(out?.message).toMatch(/reference/i)
  })

  it('returns null for a non-409', () => {
    expect(parseDependencyExists({ response: { status: 400, data: { error: 'x' } } })).toBeNull()
  })

  it('returns null for a 409 that is not a dependency block', () => {
    expect(
      parseDependencyExists({ response: { status: 409, data: { error: 'flag_locked' } } }),
    ).toBeNull()
  })

  it('returns null for a non-axios error', () => {
    expect(parseDependencyExists(new Error('boom'))).toBeNull()
  })
})

describe('edgeKindLabel', () => {
  it('humanizes known kinds', () => {
    expect(edgeKindLabel('prerequisite_flag')).toBe('prerequisite flag')
    expect(edgeKindLabel('segment_ref')).toBe('segment reference')
    expect(edgeKindLabel('dependent_flag')).toBe('dependent flag')
  })
  it('falls back to underscore-stripped text', () => {
    expect(edgeKindLabel('some_other_kind')).toBe('some other kind')
  })
})

describe('edgeLabel', () => {
  const edge = (over: Partial<DependencyEdge>): DependencyEdge => ({
    entity_kind: 'flag',
    id: 'id',
    key: '',
    kind: 'prerequisite_flag',
    ...over,
  })
  it('prefers the key', () => {
    expect(edgeLabel(edge({ key: 'my-flag' }))).toBe('my-flag')
  })
  it('truncates a long id when no key', () => {
    expect(edgeLabel(edge({ key: '', id: 'aaaaaaaa-bbbb-cccc' }))).toBe('aaaaaaaa…')
  })
  it('shows a short id verbatim', () => {
    expect(edgeLabel(edge({ key: '', id: 'short' }))).toBe('short')
  })
})

describe('graphCounts', () => {
  it('counts upstream + downstream', () => {
    const up: DependencyEdge[] = [
      { entity_kind: 'flag', id: 'a', key: 'a', kind: 'prerequisite_flag' },
    ]
    expect(graphCounts(up, [])).toEqual({ upstream: 1, downstream: 0 })
  })
})
