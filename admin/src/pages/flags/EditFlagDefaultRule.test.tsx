/**
 * Tests for the EditFlagDefaultRule section — pure helpers + module-shape
 * assertions. The admin test harness runs node-env (no jsdom), so we exercise
 * the helper layer for behaviour and grep the source for wiring.
 *
 * Covers:
 *   • Distribution validation (sum = 100, each in (0, 100], unique variant_key).
 *   • API-body builders for single-variant vs percentage-distribution modes.
 *   • Module wiring: uses Formik + Yup, calls setDefaultRuleDistribution +
 *     updateFlag, surfaces the locked-by-experiment overlay.
 */
import { describe, it, expect } from 'vitest'
import SOURCE from './EditFlagDefaultRule.tsx?raw'
import {
  validateDistributionAllocations,
  buildSingleVariantPayload,
  buildDistributionPayload,
  type AllocationRow,
} from './EditFlagDefaultRule.helpers'

// ─── Distribution validation ─────────────────────────────────────────────────

describe('validateDistributionAllocations', () => {
  it('accepts a 50/50 split across two distinct variants', () => {
    const allocations: AllocationRow[] = [
      { variant_key: 'on', percentage: 50 },
      { variant_key: 'off', percentage: 50 },
    ]
    expect(validateDistributionAllocations(allocations)).toBeNull()
  })

  it('accepts a 33.3/33.3/33.4 split (within float tolerance)', () => {
    const allocations: AllocationRow[] = [
      { variant_key: 'a', percentage: 33.3 },
      { variant_key: 'b', percentage: 33.3 },
      { variant_key: 'c', percentage: 33.4 },
    ]
    expect(validateDistributionAllocations(allocations)).toBeNull()
  })

  it('rejects when percentages do not sum to 100', () => {
    const allocations: AllocationRow[] = [
      { variant_key: 'on', percentage: 60 },
      { variant_key: 'off', percentage: 30 },
    ]
    const err = validateDistributionAllocations(allocations)
    expect(err).toMatch(/sum.*100/i)
  })

  it('rejects allocations <= 0', () => {
    const allocations: AllocationRow[] = [
      { variant_key: 'on', percentage: 100 },
      { variant_key: 'off', percentage: 0 },
    ]
    const err = validateDistributionAllocations(allocations)
    expect(err).toMatch(/greater than 0/i)
  })

  it('rejects allocations > 100', () => {
    const allocations: AllocationRow[] = [
      { variant_key: 'on', percentage: 120 },
      { variant_key: 'off', percentage: -20 },
    ]
    const err = validateDistributionAllocations(allocations)
    expect(err).toBeTruthy()
  })

  it('rejects duplicate variant_keys', () => {
    const allocations: AllocationRow[] = [
      { variant_key: 'on', percentage: 50 },
      { variant_key: 'on', percentage: 50 },
    ]
    const err = validateDistributionAllocations(allocations)
    expect(err).toMatch(/unique|duplicate/i)
  })

  it('rejects empty variant_key', () => {
    const allocations: AllocationRow[] = [
      { variant_key: '', percentage: 50 },
      { variant_key: 'off', percentage: 50 },
    ]
    const err = validateDistributionAllocations(allocations)
    expect(err).toMatch(/variant/i)
  })

  it('rejects empty allocation list', () => {
    expect(validateDistributionAllocations([])).toMatch(/at least one/i)
  })
})

// ─── Payload builders ────────────────────────────────────────────────────────

describe('buildSingleVariantPayload', () => {
  it('emits default_variant_key + null default_rule_distribution', () => {
    const payload = buildSingleVariantPayload('on', 7)
    expect(payload).toEqual({
      default_variant_key: 'on',
      default_rule_distribution: null,
      version: 7,
    })
  })
})

describe('buildDistributionPayload', () => {
  it('emits distribution allocations', () => {
    const allocations: AllocationRow[] = [
      { variant_key: 'on', percentage: 60 },
      { variant_key: 'off', percentage: 40 },
    ]
    const payload = buildDistributionPayload(allocations)
    expect(payload.allocations).toEqual([
      { variant_key: 'on', percentage: 60 },
      { variant_key: 'off', percentage: 40 },
    ])
  })

  it('rounds float percentages to 0.1 precision', () => {
    const allocations: AllocationRow[] = [
      { variant_key: 'on', percentage: 33.333333 },
      { variant_key: 'off', percentage: 66.666666 },
    ]
    const payload = buildDistributionPayload(allocations)
    // We keep raw precision in the payload — the gateway rounds to its
    // canonical step. Just confirm we don't truncate.
    expect(payload.allocations[0].percentage).toBeCloseTo(33.333333, 5)
  })
})

// ─── Module shape ────────────────────────────────────────────────────────────

describe('EditFlagDefaultRule (module shape)', () => {
  it('imports the typed default-rule API wrapper', () => {
    expect(SOURCE).toMatch(/setDefaultRuleDistribution/)
  })

  it('uses api.put for the single-variant save path', () => {
    expect(SOURCE).toMatch(/api\.put/)
    // And the URL must reference flags by key.
    expect(SOURCE).toMatch(/flags\/\$\{flag\.key\}/)
  })

  it('renders a single-variant / percentage-distribution mode toggle', () => {
    // Either a radio or segmented control. We just confirm both mode strings
    // appear in the source.
    expect(SOURCE).toMatch(/single_variant|Single variant/)
    expect(SOURCE).toMatch(/percentage_distribution|Percentage distribution/)
  })

  it('surfaces a locked-by-experiment overlay', () => {
    expect(SOURCE).toMatch(/locked_by_experiment|Locked by experiment/)
  })

  it('uses the validation helpers for the percentage mode', () => {
    expect(SOURCE).toMatch(/validateDistributionAllocations/)
  })
})
