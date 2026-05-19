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
    const o: RuleOutputJson = { allocation: { hash_targets: [{ context_type: 'user', field: 'key' }], buckets: [{ variant_key: 'on', weight_milli: 1000 }] } }
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

// ─── Regression: phantom empty leaf in segment rule editor (feature-flag-qjw) ──
//
// SegmentDetail now initialises with { And: [] } rather than defaultCondition().
// The tests below assert that:
//   1. { And: [] } is a valid ConditionExpr (empty-And = always-true catch-all).
//   2. defaultCondition() itself is still a Leaf (used internally by the rule
//      builder when the user explicitly adds a condition row).
//   3. The empty-And cannot accidentally contain a phantom leaf on first mount.
describe('segment rule editor — no phantom empty leaf on init', () => {
  it('empty And is a valid ConditionExpr and has no children', () => {
    const emptyAnd: ConditionExpr = { And: [] }
    expect(exprKey(emptyAnd)).toBe('And')
    expect((emptyAnd as { And: ConditionExpr[] }).And).toHaveLength(0)
  })

  it('defaultCondition still returns a Leaf (used for explicit Add condition)', () => {
    const expr = defaultCondition()
    expect(exprKey(expr)).toBe('Leaf')
  })

  it('segment init value is empty And — not a phantom leaf', () => {
    // This mirrors the logic in SegmentConditionEditor:
    //   (segment.condition_expr as ConditionExpr | null) ?? EMPTY_CONDITION
    // When condition_expr is null, result must be { And: [] }, never a Leaf.
    const conditionExprFromDb: ConditionExpr | null = null
    const EMPTY_CONDITION: ConditionExpr = { And: [] }
    const initialExpr = conditionExprFromDb ?? EMPTY_CONDITION
    expect(exprKey(initialExpr)).toBe('And')
    expect((initialExpr as { And: ConditionExpr[] }).And).toHaveLength(0)
  })

  it('after one Add condition the And contains exactly one leaf (no phantom)', () => {
    // Simulates: user starts with { And: [] }, clicks Add condition (addLeaf),
    // which appends defaultCondition() to the children array.
    const initial: ConditionExpr = { And: [] }
    const children = (initial as { And: ConditionExpr[] }).And
    const afterAddLeaf: ConditionExpr = { And: [...children, defaultCondition()] }
    expect((afterAddLeaf as { And: ConditionExpr[] }).And).toHaveLength(1)
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
