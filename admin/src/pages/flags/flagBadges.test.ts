/**
 * Unit tests for the flags-list lifecycle badge logic
 * (flag_lifecycle_20260604, Phase 8.5). Pure logic only (environment `node`).
 */
import { describe, it, expect } from 'vitest'
import type { AdminFlagResponse } from '../../lib/types'
import { buildPrerequisiteOfSet, hasPrerequisites } from './flagBadges'

function flag(over: Partial<AdminFlagResponse>): AdminFlagResponse {
  return {
    flag_id: 'id',
    key: 'k',
    name: 'n',
    description: '',
    flag_type: 'bool',
    enabled: true,
    status: 'enabled',
    version: 1,
    variants: [],
    rules: [],
    default_variant_key: null,
    created_at: null,
    updated_at: null,
    ...over,
  }
}

describe('hasPrerequisites', () => {
  it('is true when the flag has prerequisite rows', () => {
    expect(
      hasPrerequisites(
        flag({ prerequisites: [{ prerequisite_flag_key: 'a', required_variant_key: 'on' }] }),
      ),
    ).toBe(true)
  })
  it('is false when absent or empty', () => {
    expect(hasPrerequisites(flag({}))).toBe(false)
    expect(hasPrerequisites(flag({ prerequisites: [] }))).toBe(false)
  })
})

describe('buildPrerequisiteOfSet', () => {
  it('collects every key referenced as a prerequisite', () => {
    const flags = [
      flag({ key: 'parent', prerequisites: [{ prerequisite_flag_key: 'child', required_variant_key: 'on' }] }),
      flag({ key: 'child' }),
      flag({ key: 'lonely' }),
    ]
    const set = buildPrerequisiteOfSet(flags)
    expect(set.has('child')).toBe(true)
    expect(set.has('parent')).toBe(false)
    expect(set.has('lonely')).toBe(false)
  })

  it('ignores empty prerequisite keys', () => {
    const set = buildPrerequisiteOfSet([
      flag({ key: 'p', prerequisites: [{ prerequisite_flag_key: '', required_variant_key: '' }] }),
    ])
    expect(set.size).toBe(0)
  })
})
