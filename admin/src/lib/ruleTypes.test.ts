import { describe, it, expect } from 'vitest'
import {
  conditionKey,
  exprKey,
  isVariantOutput,
  allocationSum,
  defaultCondition,
  defaultOutput,
  localId,
  type Condition,
  type ConditionExpr,
  type RuleOutputJson,
  type AllocationBucket,
} from './ruleTypes'

describe('conditionKey', () => {
  it('returns the variant key of a Condition', () => {
    const c: Condition = { Eq: { context_type: 'user', param: 'id', value: '123' } }
    expect(conditionKey(c)).toBe('Eq')
  })

  it('handles InSegment (string payload)', () => {
    const c: Condition = { InSegment: 'beta-users' }
    expect(conditionKey(c)).toBe('InSegment')
  })

  it('handles FlagEvaluatedAs', () => {
    const c: Condition = { FlagEvaluatedAs: { flag_id: 'f1', variant_id: 'on' } }
    expect(conditionKey(c)).toBe('FlagEvaluatedAs')
  })
})

describe('exprKey', () => {
  it('returns Leaf for leaf expressions', () => {
    const e: ConditionExpr = { Leaf: { Eq: { context_type: 'user', param: 'plan', value: 'pro' } } }
    expect(exprKey(e)).toBe('Leaf')
  })

  it('returns And for And expressions', () => {
    const e: ConditionExpr = { And: [] }
    expect(exprKey(e)).toBe('And')
  })

  it('returns Or for Or expressions', () => {
    const e: ConditionExpr = { Or: [] }
    expect(exprKey(e)).toBe('Or')
  })

  it('returns Not for Not expressions', () => {
    const e: ConditionExpr = { Not: { And: [] } }
    expect(exprKey(e)).toBe('Not')
  })
})

describe('isVariantOutput', () => {
  it('returns true for variant_key outputs', () => {
    const o: RuleOutputJson = { variant_key: 'on' }
    expect(isVariantOutput(o)).toBe(true)
  })

  it('returns false for allocation outputs', () => {
    const o: RuleOutputJson = { allocation: [{ variant_key: 'on', weight_milli: 1000 }] }
    expect(isVariantOutput(o)).toBe(false)
  })
})

describe('allocationSum', () => {
  it('returns 0 for empty buckets', () => {
    expect(allocationSum([])).toBe(0)
  })

  it('sums weight_milli values', () => {
    const buckets: AllocationBucket[] = [
      { variant_key: 'on', weight_milli: 700 },
      { variant_key: 'off', weight_milli: 300 },
    ]
    expect(allocationSum(buckets)).toBe(1000)
  })

  it('detects invalid totals', () => {
    const buckets: AllocationBucket[] = [
      { variant_key: 'a', weight_milli: 500 },
      { variant_key: 'b', weight_milli: 400 },
    ]
    expect(allocationSum(buckets)).toBe(900)
    expect(allocationSum(buckets)).not.toBe(1000)
  })
})

describe('defaultCondition', () => {
  it('returns a Leaf Eq condition', () => {
    const expr = defaultCondition()
    expect(exprKey(expr)).toBe('Leaf')
    const leaf = (expr as { Leaf: Condition }).Leaf
    expect(conditionKey(leaf)).toBe('Eq')
  })

  it('uses user context_type by default', () => {
    const expr = defaultCondition()
    const leaf = (expr as { Leaf: { Eq: { context_type: string; param: string; value: string } } }).Leaf
    expect(leaf.Eq.context_type).toBe('user')
  })
})

describe('defaultOutput', () => {
  it('returns a variant_key output', () => {
    const o = defaultOutput('on')
    expect(isVariantOutput(o)).toBe(true)
    expect((o as { variant_key: string }).variant_key).toBe('on')
  })
})

describe('localId', () => {
  it('returns a non-empty string', () => {
    expect(localId().length).toBeGreaterThan(0)
  })

  it('generates unique IDs', () => {
    const ids = Array.from({ length: 100 }, localId)
    expect(new Set(ids).size).toBe(100)
  })
})
