/**
 * Unit tests for ExperimentDetail page helpers.
 *
 * These pin the API → display-model derivation logic that replaced the
 * `EXPERIMENTS` mock in Phase 8. Each helper has a small surface area; tests
 * focus on the boundary cases the page actually hits:
 *
 *   • Loading: API returns nothing.
 *   • Partial: experiment loaded but results pending (or null).
 *   • Active-context-type missing from results: fallback to first key.
 *   • Bayesian vs frequentist confidence picker.
 *   • Winner-row detection.
 */
import { describe, it, expect } from 'vitest'
import {
  asConfidencePercent,
  boundTargetLabel,
  buildExperimentDisplay,
  findControlRow,
  findWinnerRow,
  formatLift,
  formatSamples,
  listContextTypes,
  pickContextTypeResult,
} from './ExperimentDetail.helpers'
import type { ExperimentResults, VariantResultJson } from '../../lib/types'
import type { ExperimentSummary } from '../../lib/api'

// ── Fixtures ─────────────────────────────────────────────────────────────────

const CONTROL_ROW: VariantResultJson = {
  variant_key: 'control',
  assigned_count: 1000,
  mean: 0.1,
  sample_size: 1000,
  p_value: null,
  ci_lower: 0.09,
  ci_upper: 0.11,
  prob_to_beat_control: null,
  expected_lift: null,
  is_winner: false,
}

const WINNER_ROW: VariantResultJson = {
  variant_key: 'variant-a',
  assigned_count: 1000,
  mean: 0.12,
  sample_size: 1000,
  p_value: 0.012,
  ci_lower: 0.105,
  ci_upper: 0.135,
  prob_to_beat_control: 0.95,
  expected_lift: 0.04,
  is_winner: true,
}

const EXP_SUMMARY: ExperimentSummary = {
  experiment_id: 'exp-1',
  environment_id: 'env-1',
  key: 'checkout-v2',
  name: 'Checkout V2',
  description: 'desc',
  flag_key: 'checkout-flag',
  status: 'running',
  model: 'frequentist',
  metric_ids: ['metric-1'],
  variants: 2,
  started_at: '2026-05-01T00:00:00Z',
  ended_at: null,
  created_at: '2026-05-01T00:00:00Z',
  updated_at: '2026-05-01T00:00:00Z',
}

function buildResults(blocks: ExperimentResults['results_by_context_type']): ExperimentResults {
  return {
    results_by_context_type: blocks,
    bound_target: { kind: 'rule', rule_id: 'rule-1', label: 'checkout-rule' },
    pre_period_days: 0,
  }
}

const FULL_RESULTS = buildResults({
  user: {
    variants: [CONTROL_ROW, WINNER_ROW],
    srm: { per_variant: [], overall_chi_sq_p: 0.5, health: 'green' },
    guardrails: [],
  },
  account: {
    variants: [CONTROL_ROW, { ...WINNER_ROW, is_winner: false, p_value: 0.4, expected_lift: 0.01, prob_to_beat_control: 0.6 }],
    srm: { per_variant: [], overall_chi_sq_p: 0.5, health: 'green' },
    guardrails: [],
  },
})

// ── pickContextTypeResult ────────────────────────────────────────────────────

describe('pickContextTypeResult', () => {
  it('returns null when results is null', () => {
    expect(pickContextTypeResult(null, 'user')).toBeNull()
  })

  it('returns null when results has no context types', () => {
    expect(pickContextTypeResult(buildResults({}), 'user')).toBeNull()
  })

  it('returns the requested context type when present', () => {
    const r = pickContextTypeResult(FULL_RESULTS, 'account')
    expect(r?.contextType).toBe('account')
    expect(r?.block.variants).toHaveLength(2)
  })

  it('falls back to the first key when the requested type is missing', () => {
    const r = pickContextTypeResult(FULL_RESULTS, 'organization')
    // First in insertion order is 'user'.
    expect(r?.contextType).toBe('user')
  })

  it('falls back to the first key when activeContextType is null', () => {
    const r = pickContextTypeResult(FULL_RESULTS, null)
    expect(r?.contextType).toBe('user')
  })
})

// ── findControlRow + findWinnerRow ───────────────────────────────────────────

describe('findControlRow', () => {
  it('returns the row with null p_value + null expected_lift', () => {
    const ctrl = findControlRow([CONTROL_ROW, WINNER_ROW])
    expect(ctrl?.variant_key).toBe('control')
  })

  it('returns null for an empty list', () => {
    expect(findControlRow([])).toBeNull()
  })

  it('returns the first row when none match the control predicate', () => {
    // Two non-control rows — fallback to first.
    const rows = [WINNER_ROW, { ...WINNER_ROW, variant_key: 'variant-b', is_winner: false }]
    expect(findControlRow(rows)?.variant_key).toBe('variant-a')
  })
})

describe('findWinnerRow', () => {
  it('returns the row flagged is_winner', () => {
    expect(findWinnerRow([CONTROL_ROW, WINNER_ROW])?.variant_key).toBe('variant-a')
  })

  it('returns null when no winner is flagged', () => {
    expect(findWinnerRow([CONTROL_ROW])).toBeNull()
  })
})

// ── Formatters ───────────────────────────────────────────────────────────────

describe('formatLift', () => {
  it('formats positive lift with a plus sign', () => {
    expect(formatLift(0.047)).toBe('+4.7%')
  })

  it('formats negative lift with a minus sign', () => {
    expect(formatLift(-0.012)).toBe('−1.2%')
  })

  it('returns "—" for null', () => {
    expect(formatLift(null)).toBe('—')
  })

  it('returns "—" for NaN', () => {
    expect(formatLift(NaN)).toBe('—')
  })
})

describe('asConfidencePercent', () => {
  it('maps a fraction in [0, 1] to an integer percent', () => {
    expect(asConfidencePercent(0.95)).toBe(95)
    expect(asConfidencePercent(0.123)).toBe(12)
  })

  it('returns 0 for null', () => {
    expect(asConfidencePercent(null)).toBe(0)
  })

  it('clamps to [0, 100]', () => {
    expect(asConfidencePercent(-0.5)).toBe(0)
    expect(asConfidencePercent(2.0)).toBe(100)
  })
})

describe('formatSamples', () => {
  it('uses thousands separators', () => {
    expect(formatSamples(412415)).toBe('412,415')
  })

  it('returns "0" for null', () => {
    expect(formatSamples(null)).toBe('0')
  })

  it('floors fractional input', () => {
    expect(formatSamples(1234.7)).toBe('1,234')
  })
})

// ── buildExperimentDisplay ───────────────────────────────────────────────────

describe('buildExperimentDisplay', () => {
  it('uses defaults when both exp and results are null', () => {
    const d = buildExperimentDisplay(null, null, null, false)
    expect(d.variants).toBe(2)
    expect(d.model).toBe('frequentist')
    expect(d.lift).toBe('—')
    expect(d.confidence).toBe(0)
    expect(d.samples).toBe('0')
    expect(d.remaining).toBe('—')
    expect(d.flag).toBe('—')
    expect(d.primary).toBe('—')
    expect(d.started).toBe('—')
  })

  it('uses the experiment summary when results are still loading', () => {
    const d = buildExperimentDisplay(EXP_SUMMARY, null, null, false)
    expect(d.variants).toBe(2)
    expect(d.model).toBe('frequentist')
    expect(d.flag).toBe('checkout-flag')
    expect(d.primary).toBe('metric-1')
    expect(d.started).not.toBe('—') // formatted date
    expect(d.lift).toBe('—')
    expect(d.confidence).toBe(0)
    expect(d.remaining).toBe('—')
  })

  it('reports frequentist confidence = 1 - p_value when winner has a p_value', () => {
    const d = buildExperimentDisplay(EXP_SUMMARY, FULL_RESULTS, 'user', false)
    expect(d.confidence).toBe(99) // 1 - 0.012 = 0.988 → 99
    expect(d.lift).toBe('+4.0%')
    expect(d.remaining).toBe('ready')
  })

  it('reports bayesian confidence = prob_to_beat_control when useBayesian=true', () => {
    const d = buildExperimentDisplay(EXP_SUMMARY, FULL_RESULTS, 'user', true)
    expect(d.confidence).toBe(95) // prob_to_beat_control = 0.95
  })

  it('sums sample_size across variant rows', () => {
    const d = buildExperimentDisplay(EXP_SUMMARY, FULL_RESULTS, 'user', false)
    expect(d.samples).toBe('2,000') // 1000 control + 1000 winner
  })

  it('falls back to the first context type when requested is missing', () => {
    const d = buildExperimentDisplay(EXP_SUMMARY, FULL_RESULTS, 'organization', false)
    expect(d.remaining).toBe('ready') // user block has a winner
  })

  it('reports "running" when there is data but no winner yet', () => {
    const partial = buildResults({
      user: {
        variants: [CONTROL_ROW, { ...WINNER_ROW, is_winner: false }],
        srm: { per_variant: [], overall_chi_sq_p: 0.5, health: 'green' },
        guardrails: [],
      },
    })
    const d = buildExperimentDisplay(EXP_SUMMARY, partial, 'user', false)
    expect(d.remaining).toBe('running')
  })
})

// ── boundTargetLabel + listContextTypes ──────────────────────────────────────

describe('boundTargetLabel', () => {
  it('returns "—" when results are null', () => {
    expect(boundTargetLabel(null)).toBe('—')
  })

  it('passes through the bound_target.label from results', () => {
    expect(boundTargetLabel(FULL_RESULTS)).toBe('checkout-rule')
  })
})

describe('listContextTypes', () => {
  it('returns sorted keys from results.results_by_context_type', () => {
    expect(listContextTypes(FULL_RESULTS)).toEqual(['account', 'user'])
  })

  it('returns empty when results is null', () => {
    expect(listContextTypes(null)).toEqual([])
  })

  it('returns empty when no context types are present', () => {
    expect(listContextTypes(buildResults({}))).toEqual([])
  })
})
