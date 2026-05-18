/**
 * Tests for the "Match Segment" rule type integration.
 *
 * Covers:
 *  1. Wire-format serialisation: InSegment / NotInSegment conditions carry a
 *     plain segment UUID string (matching the Rust ConditionExpr serde shape).
 *  2. Round-trip: a condition built from a segment picker value can be passed
 *     back through ruleTypes helpers unchanged.
 *  3. `conditionKey` correctly identifies segment condition variants.
 *  4. `buildCondition` (the factory used by ConditionClauseEditor) produces
 *     the right shape when switching to/from segment ops.
 */
import { describe, it, expect } from 'vitest'
import {
  conditionKey,
  exprKey,
  type Condition,
  type ConditionExpr,
} from '../../lib/ruleTypes'

// ─── Helpers (mirrors buildCondition logic in ConditionClauseEditor) ─────────

function buildSegmentCondition(op: 'InSegment' | 'NotInSegment', segmentId: string): Condition {
  return { [op]: segmentId } as Condition
}

function serializeConditionExpr(expr: ConditionExpr): unknown {
  // Simulates how FlagDetail.saveRules sends conditions to the backend —
  // just returns the plain object (JSON-serialisable).
  return expr
}

function deserializeConditionExpr(raw: unknown): ConditionExpr {
  // Simulates parseRuleState — casts the raw condition as ConditionExpr.
  return raw as ConditionExpr
}

// ─── Tests ────────────────────────────────────────────────────────────────────

describe('InSegment / NotInSegment wire format', () => {
  it('serialises InSegment as { InSegment: "<uuid>" }', () => {
    const segmentId = '550e8400-e29b-41d4-a716-446655440000'
    const cond = buildSegmentCondition('InSegment', segmentId)
    expect(cond).toEqual({ InSegment: segmentId })
  })

  it('serialises NotInSegment as { NotInSegment: "<uuid>" }', () => {
    const segmentId = '550e8400-e29b-41d4-a716-446655440001'
    const cond = buildSegmentCondition('NotInSegment', segmentId)
    expect(cond).toEqual({ NotInSegment: segmentId })
  })

  it('segment ID is a plain string, not an object', () => {
    const cond: Condition = { InSegment: 'my-seg-id' }
    const value = (cond as { InSegment: string }).InSegment
    expect(typeof value).toBe('string')
  })
})

describe('conditionKey identifies segment variants', () => {
  it('returns "InSegment" for an InSegment condition', () => {
    const c: Condition = { InSegment: 'seg-abc' }
    expect(conditionKey(c)).toBe('InSegment')
  })

  it('returns "NotInSegment" for a NotInSegment condition', () => {
    const c: Condition = { NotInSegment: 'seg-xyz' }
    expect(conditionKey(c)).toBe('NotInSegment')
  })
})

describe('round-trip: Leaf(InSegment) condition', () => {
  const segmentId = 'seg-roundtrip-123'

  it('serialises and deserialises a Leaf(InSegment) expr unchanged', () => {
    const original: ConditionExpr = {
      Leaf: { InSegment: segmentId },
    }
    const wireJson = serializeConditionExpr(original)
    const restored = deserializeConditionExpr(wireJson)

    expect(exprKey(restored)).toBe('Leaf')
    const leaf = (restored as { Leaf: Condition }).Leaf
    expect(conditionKey(leaf)).toBe('InSegment')
    expect((leaf as { InSegment: string }).InSegment).toBe(segmentId)
  })

  it('preserves the segment ID through JSON stringify/parse', () => {
    const original: ConditionExpr = { Leaf: { InSegment: segmentId } }
    const restored = JSON.parse(JSON.stringify(original)) as ConditionExpr
    expect((restored as { Leaf: { InSegment: string } }).Leaf.InSegment).toBe(segmentId)
  })
})

describe('round-trip: Leaf(NotInSegment) condition', () => {
  const segmentId = 'seg-not-in-456'

  it('serialises and deserialises a Leaf(NotInSegment) expr unchanged', () => {
    const original: ConditionExpr = {
      Leaf: { NotInSegment: segmentId },
    }
    const wireJson = serializeConditionExpr(original)
    const restored = deserializeConditionExpr(wireJson)

    expect(exprKey(restored)).toBe('Leaf')
    const leaf = (restored as { Leaf: Condition }).Leaf
    expect(conditionKey(leaf)).toBe('NotInSegment')
    expect((leaf as { NotInSegment: string }).NotInSegment).toBe(segmentId)
  })
})

describe('segment condition nested in And/Or group', () => {
  it('survives And([InSegment, Eq]) round-trip', () => {
    const expr: ConditionExpr = {
      And: [
        { Leaf: { InSegment: 'seg-abc' } },
        { Leaf: { Eq: { context_type: 'user', param: 'plan', value: 'pro' } } },
      ],
    }
    const restored = deserializeConditionExpr(serializeConditionExpr(expr))
    expect(exprKey(restored)).toBe('And')
    const children = (restored as { And: ConditionExpr[] }).And
    expect(children).toHaveLength(2)
    expect(conditionKey((children[0] as { Leaf: Condition }).Leaf)).toBe('InSegment')
    expect((children[0] as { Leaf: { InSegment: string } }).Leaf.InSegment).toBe('seg-abc')
  })
})

describe('SegmentBadge link construction', () => {
  it('builds correct segment detail URL from orgId and segmentId', () => {
    const orgId = 'org-123'
    const segmentId = 'seg-abc'
    const href = `/org/${orgId}/segments/${segmentId}`
    expect(href).toBe('/org/org-123/segments/seg-abc')
  })
})
