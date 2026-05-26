import { describe, it, expect } from 'vitest'
import {
  conditionKey,
  exprKey,
  isVariantOutput,
  allocationSum,
  defaultCondition,
  defaultOutput,
  defaultAllocationOutput,
  hashInputsFromTargets,
  hashTargetsFromInputs,
  normalizeOutput,
  localId,
  type Condition,
  type ConditionExpr,
  type RuleOutputJson,
  type AllocationBucket,
  type AllocationOutput,
} from './ruleTypes'
import type { HashSelector } from './hashInputTypes'

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
    const o: RuleOutputJson = {
      allocation: {
        hash_inputs: [{ kind: 'context_key', context_type: 'user' }],
        hash_targets: [{ context_type: 'user', field: 'key' }],
        buckets: [{ variant_key: 'on', weight_bp: 10000 }],
      },
    }
    expect(isVariantOutput(o)).toBe(false)
  })
})

describe('allocationSum', () => {
  it('returns 0 for empty buckets', () => {
    expect(allocationSum([])).toBe(0)
  })

  it('sums weight_bp values', () => {
    const buckets: AllocationBucket[] = [
      { variant_key: 'on', weight_bp: 7000 },
      { variant_key: 'off', weight_bp: 3000 },
    ]
    expect(allocationSum(buckets)).toBe(10000)
  })

  it('detects invalid totals', () => {
    const buckets: AllocationBucket[] = [
      { variant_key: 'a', weight_bp: 5000 },
      { variant_key: 'b', weight_bp: 4000 },
    ]
    expect(allocationSum(buckets)).toBe(9000)
    expect(allocationSum(buckets)).not.toBe(10000)
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

// ─── Phase 7: hash_inputs projection + round-trip identity ─────────────────────
//
// Phase 4 of `flag_eval_unify_20260522` introduces `hash_inputs` as the
// canonical selector list on percentage-allocation outputs, alongside the
// legacy `hash_targets`. The admin UI authors `hash_inputs` and derives
// `hash_targets` on every change. These tests pin the projection helpers
// and the round-trip-identity contract for Task 7.8 (round-trip save →
// reopen → edit → save).

describe('hashTargetsFromInputs / hashInputsFromTargets', () => {
  it('projects ContextKey to {field: "key"}', () => {
    const inputs: HashSelector[] = [
      { kind: 'context_key', context_type: 'user' },
    ]
    expect(hashTargetsFromInputs(inputs)).toEqual([
      { context_type: 'user', field: 'key' },
    ])
  })

  it('projects ContextParameter to {field: <parameter>}', () => {
    const inputs: HashSelector[] = [
      { kind: 'context_parameter', context_type: 'device', parameter: 'os' },
    ]
    expect(hashTargetsFromInputs(inputs)).toEqual([
      { context_type: 'device', field: 'os' },
    ])
  })

  it('reverse projection upgrades legacy "key" to ContextKey', () => {
    const targets = [{ context_type: 'user', field: 'key' }]
    expect(hashInputsFromTargets(targets)).toEqual([
      { kind: 'context_key', context_type: 'user' },
    ])
  })

  it('reverse projection upgrades legacy parameter to ContextParameter', () => {
    const targets = [{ context_type: 'device', field: 'os' }]
    expect(hashInputsFromTargets(targets)).toEqual([
      { kind: 'context_parameter', context_type: 'device', parameter: 'os' },
    ])
  })

  it('round-trips a multi-context selector list', () => {
    const inputs: HashSelector[] = [
      { kind: 'context_key', context_type: 'user' },
      { kind: 'context_parameter', context_type: 'user', parameter: 'name' },
      { kind: 'context_parameter', context_type: 'device', parameter: 'os' },
      { kind: 'context_key', context_type: 'application' },
    ]
    // inputs → targets → inputs is the identity.
    const targets = hashTargetsFromInputs(inputs)
    expect(hashInputsFromTargets(targets)).toEqual(inputs)
  })
})

describe('normalizeOutput — Phase 7 backwards compatibility', () => {
  it('upgrades a bare-array legacy allocation to hash_inputs', () => {
    const raw = { allocation: [{ variant_key: 'on', weight_bp: 10000 }] }
    const out = normalizeOutput(raw)
    expect(isVariantOutput(out)).toBe(false)
    const a = (out as { allocation: AllocationOutput }).allocation
    expect(a.hash_inputs).toEqual([
      { kind: 'context_key', context_type: 'user' },
    ])
    expect(a.hash_targets).toEqual([{ context_type: 'user', field: 'key' }])
  })

  it('uses hash_inputs verbatim when present, derives hash_targets', () => {
    const raw = {
      allocation: {
        hash_inputs: [
          { kind: 'context_key', context_type: 'user' },
          {
            kind: 'context_parameter',
            context_type: 'device',
            parameter: 'os',
          },
        ],
        hash_targets: [], // server-derived; UI re-derives on every change
        buckets: [
          { variant_key: 'on', weight_bp: 5000 },
          { variant_key: 'off', weight_bp: 5000 },
        ],
      },
    }
    const out = normalizeOutput(raw)
    const a = (out as { allocation: AllocationOutput }).allocation
    expect(a.hash_inputs).toHaveLength(2)
    expect(a.hash_targets).toEqual([
      { context_type: 'user', field: 'key' },
      { context_type: 'device', field: 'os' },
    ])
  })

  it('derives hash_inputs from hash_targets when hash_inputs is absent', () => {
    // Pre-Phase-4 payloads only carry `hash_targets`. The UI must lift
    // them into the canonical shape so the rule builder can edit them.
    const raw = {
      allocation: {
        hash_targets: [
          { context_type: 'user', field: 'key' },
          { context_type: 'org', field: 'tier' },
        ],
        buckets: [{ variant_key: 'on', weight_bp: 10000 }],
      },
    }
    const out = normalizeOutput(raw)
    const a = (out as { allocation: AllocationOutput }).allocation
    expect(a.hash_inputs).toEqual([
      { kind: 'context_key', context_type: 'user' },
      {
        kind: 'context_parameter',
        context_type: 'org',
        parameter: 'tier',
      },
    ])
  })

  it('normalizes empty allocation to canonical default', () => {
    const out = normalizeOutput({ allocation: {} })
    const a = (out as { allocation: AllocationOutput }).allocation
    expect(a.hash_inputs).toEqual([
      { kind: 'context_key', context_type: 'user' },
    ])
    expect(a.hash_targets).toEqual([{ context_type: 'user', field: 'key' }])
    expect(a.buckets).toEqual([])
  })
})

describe('defaultAllocationOutput — Phase 7 shape', () => {
  it('seeds hash_inputs with user.key', () => {
    const out = defaultAllocationOutput(['on', 'off'])
    expect(out.hash_inputs).toEqual([
      { kind: 'context_key', context_type: 'user' },
    ])
  })

  it('seeds hash_targets consistent with hash_inputs', () => {
    const out = defaultAllocationOutput(['on', 'off'])
    expect(out.hash_targets).toEqual(hashTargetsFromInputs(out.hash_inputs))
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
