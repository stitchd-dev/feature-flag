/**
 * Unit tests for the prerequisites-editor pure helpers
 * (flag_lifecycle_20260604, Phase 8.3). Environment is `node`.
 */
import { describe, it, expect } from 'vitest'
import { detectLocalCycle, toSetBody, isCycleMessage } from './prerequisiteHelpers'
import type { PrereqRow as Row } from './prerequisiteHelpers'

describe('detectLocalCycle', () => {
  it('flags a self-edge', () => {
    const rows: Row[] = [{ prerequisite_flag_key: 'a', required_variant_key: 'on' }]
    expect(detectLocalCycle('a', rows, {})).toBe('a → a')
  })

  it('flags a direct 2-cycle when the prerequisite already depends on this flag', () => {
    const rows: Row[] = [{ prerequisite_flag_key: 'b', required_variant_key: 'on' }]
    // b already lists a as a prerequisite.
    expect(detectLocalCycle('a', rows, { b: ['a'] })).toBe('a → b → a')
  })

  it('returns null for a valid (locally acyclic) edge', () => {
    const rows: Row[] = [{ prerequisite_flag_key: 'b', required_variant_key: 'on' }]
    expect(detectLocalCycle('a', rows, { b: ['c'] })).toBeNull()
  })

  it('ignores empty rows', () => {
    const rows: Row[] = [{ prerequisite_flag_key: '', required_variant_key: '' }]
    expect(detectLocalCycle('a', rows, {})).toBeNull()
  })
})

describe('toSetBody', () => {
  it('builds the PUT body and drops incomplete rows', () => {
    const rows: Row[] = [
      { prerequisite_flag_key: 'b', required_variant_key: 'on' },
      { prerequisite_flag_key: '', required_variant_key: '' },
      { prerequisite_flag_key: 'c', required_variant_key: '' },
    ]
    const body = toSetBody(rows, 'off', 3)
    expect(body).toEqual({
      prerequisites: [{ prerequisite_flag_key: 'b', required_variant_key: 'on' }],
      fallback_variant_key: 'off',
      version: 3,
    })
  })

  it('allows an empty fallback (off/disabled variant)', () => {
    expect(toSetBody([], '', 1).fallback_variant_key).toBe('')
  })
})

describe('isCycleMessage', () => {
  it('recognizes the flag-service cycle message', () => {
    expect(isCycleMessage('prerequisite cycle detected: a -> b -> a')).toBe(true)
  })
  it('rejects unrelated messages', () => {
    expect(isCycleMessage('flag locked by experiment')).toBe(false)
  })
})
