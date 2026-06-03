/**
 * Interactions tab tests — N-way interactions (P5, updated from P8).
 *
 * Coverage (node env + renderToString):
 *   • Pure helpers: hasSignificantInteraction, formatPValue, formatEstimate,
 *     formatBayesProb, formatBayesCI, formatTerm.
 *   • Table render: experiment names, order badge, context type, metric,
 *     shared count, estimate, p-value, Bayesian columns, significance badge.
 *   • Significance badge variant: "warning" + data-significant="true" when
 *     significant, "success" + data-significant="false" otherwise.
 *   • Empty + loading + error states.
 */
import { describe, it, expect } from 'vitest'
import { renderToString } from 'react-dom/server'
import {
  InteractionsTab,
  hasSignificantInteraction,
  formatPValue,
  formatEstimate,
  formatBayesProb,
  formatBayesCI,
  formatTerm,
} from './Interactions'
import type { ExperimentInteraction } from '../../../lib/api/exclusionGroups'

const SIGNIFICANT: ExperimentInteraction = {
  experiment_ids: ['exp-a', 'exp-b'],
  experiment_names: ['Checkout Redesign', 'Pricing banner test'],
  interaction_order: 2,
  term: '2way:exp-axexp-b',
  context_type: 'user',
  metric_key: 'checkout_completed',
  shared_count: 12500,
  interaction_estimate: 0.0421,
  p_value: 0.0003,
  df: 1,
  significant: true,
  insufficient_data: false,
  bayes_prob: 0.97,
  bayes_expected: 0.038,
  bayes_ci_low: 0.01,
  bayes_ci_high: 0.07,
}

const NOT_SIGNIFICANT: ExperimentInteraction = {
  experiment_ids: ['exp-a', 'exp-c'],
  experiment_names: ['Checkout Redesign', 'Homepage hero'],
  interaction_order: 2,
  term: '2way:exp-axexp-c',
  context_type: 'account',
  metric_key: 'revenue_per_user',
  shared_count: 800,
  interaction_estimate: -0.001,
  p_value: 0.62,
  df: 1,
  significant: false,
  insufficient_data: false,
  bayes_prob: 0.42,
  bayes_expected: -0.0008,
  bayes_ci_low: -0.012,
  bayes_ci_high: 0.010,
}

// Backend returns 0.0 sentinels + insufficient_data=true when there isn't
// enough shared exposure. The `significant` flag may even be true on the wire,
// but the row must never be treated as a real significant result.
const INSUFFICIENT: ExperimentInteraction = {
  experiment_ids: ['exp-a', 'exp-d'],
  experiment_names: ['Checkout Redesign', 'New onboarding flow'],
  interaction_order: 2,
  term: '2way:exp-axexp-d',
  context_type: 'user',
  metric_key: 'activation_rate',
  shared_count: 12,
  interaction_estimate: 0.0,
  p_value: 0.0,
  df: 1,
  significant: true,
  insufficient_data: true,
  bayes_prob: 0.0,
  bayes_expected: 0.0,
  bayes_ci_low: 0.0,
  bayes_ci_high: 0.0,
}

// ─── Pure helpers ────────────────────────────────────────────────────────────

describe('hasSignificantInteraction', () => {
  it('returns true when any interaction is frequentist-significant', () => {
    expect(hasSignificantInteraction([NOT_SIGNIFICANT, SIGNIFICANT])).toBe(true)
  })

  it('returns false when none are significant', () => {
    expect(hasSignificantInteraction([NOT_SIGNIFICANT])).toBe(false)
  })

  it('returns false for an empty list', () => {
    expect(hasSignificantInteraction([])).toBe(false)
  })

  it('ignores insufficient-data rows even when flagged significant', () => {
    expect(hasSignificantInteraction([INSUFFICIENT])).toBe(false)
    expect(hasSignificantInteraction([NOT_SIGNIFICANT, INSUFFICIENT])).toBe(false)
  })

  it('returns false for a high-bayes-prob row that is not frequentist-significant (bayes is uncorrected, does not drive the banner)', () => {
    const highBayes: ExperimentInteraction = {
      ...NOT_SIGNIFICANT,
      significant: false,
      bayes_prob: 0.96,
    }
    // bayes_prob is an uncorrected directional posterior (≈0.5 under the null,
    // easily >0.95 for a modest effect); it must NOT fire the page-level warning.
    expect(hasSignificantInteraction([highBayes])).toBe(false)
  })

  it('returns false when bayes_prob is high but insufficient_data is set', () => {
    const highBayesInsufficient: ExperimentInteraction = {
      ...INSUFFICIENT,
      bayes_prob: 0.99,
    }
    expect(hasSignificantInteraction([highBayesInsufficient])).toBe(false)
  })
})

describe('formatPValue', () => {
  it('formats to 4 decimals', () => {
    expect(formatPValue(0.0003)).toBe('0.0003')
  })

  it('renders "—" for null/undefined/NaN', () => {
    expect(formatPValue(null)).toBe('—')
    expect(formatPValue(undefined)).toBe('—')
    expect(formatPValue(NaN)).toBe('—')
  })
})

describe('formatEstimate', () => {
  it('prefixes a positive estimate with +', () => {
    expect(formatEstimate(0.0421)).toBe('+0.0421')
  })

  it('keeps the sign on negative estimates', () => {
    expect(formatEstimate(-0.001)).toBe('-0.0010')
  })

  it('renders "—" for null', () => {
    expect(formatEstimate(null)).toBe('—')
  })
})

describe('formatBayesProb', () => {
  it('formats as a percentage to 1 decimal', () => {
    expect(formatBayesProb(0.97)).toBe('97.0%')
    expect(formatBayesProb(0.42)).toBe('42.0%')
  })

  it('renders "—" for null/undefined/NaN', () => {
    expect(formatBayesProb(null)).toBe('—')
    expect(formatBayesProb(undefined)).toBe('—')
    expect(formatBayesProb(NaN)).toBe('—')
  })
})

describe('formatBayesCI', () => {
  it('formats as [low, high] to 4 decimals', () => {
    expect(formatBayesCI(0.01, 0.07)).toBe('[0.0100, 0.0700]')
  })

  it('renders "—" when either bound is null', () => {
    expect(formatBayesCI(null, 0.07)).toBe('—')
    expect(formatBayesCI(0.01, null)).toBe('—')
  })
})

describe('formatTerm', () => {
  it('returns "Main effect" for order 1 or main: prefix', () => {
    expect(formatTerm('main:exp-a', 1)).toBe('Main effect')
    expect(formatTerm('main:exp-a', 2)).toBe('Main effect')
  })

  it('returns "A×B" for order 2 or 2way: prefix', () => {
    expect(formatTerm('2way:axb', 2)).toBe('A×B')
  })

  it('returns "A×B×C" for order 3 or 3way: prefix', () => {
    expect(formatTerm('3way:axbxc', 3)).toBe('A×B×C')
  })
})

// ─── Render ──────────────────────────────────────────────────────────────────

describe('InteractionsTab render', () => {
  it('renders a row per interaction with all columns', () => {
    const html = renderToString(
      <InteractionsTab
        interactions={[SIGNIFICANT, NOT_SIGNIFICANT]}
        loading={false}
        error={null}
      />,
    )
    expect(html).toMatch(/Pricing banner test/)
    expect(html).toMatch(/Homepage hero/)
    expect(html).toMatch(/checkout_completed/)
    expect(html).toMatch(/revenue_per_user/)
    // shared counts (formatted with grouping)
    expect(html).toMatch(/12[,.]?500/)
    expect(html).toMatch(/800/)
    // estimate + p-value
    expect(html).toMatch(/\+0\.0421/)
    expect(html).toMatch(/0\.0003/)
  })

  it('renders order badges (2-way / 3-way)', () => {
    const row3way: ExperimentInteraction = {
      experiment_ids: ['exp-a', 'exp-b', 'exp-c'],
      experiment_names: ['Exp A', 'Exp B', 'Exp C'],
      interaction_order: 3,
      term: '3way:exp-axexp-bxexp-c',
      context_type: 'user',
      metric_key: 'revenue',
      shared_count: 5000,
      interaction_estimate: 0.012,
      p_value: 0.021,
      df: 2,
      significant: false,
      insufficient_data: false,
      bayes_prob: 0.88,
      bayes_expected: 0.010,
      bayes_ci_low: -0.002,
      bayes_ci_high: 0.025,
    }
    const html = renderToString(
      <InteractionsTab
        interactions={[SIGNIFICANT, row3way]}
        loading={false}
        error={null}
      />,
    )
    expect(html).toMatch(/data-order="2"/)
    expect(html).toMatch(/data-order="3"/)
    expect(html).toMatch(/2-way/)
    expect(html).toMatch(/3-way/)
  })

  it('renders Bayesian columns (prob, expected, CI)', () => {
    const html = renderToString(
      <InteractionsTab interactions={[SIGNIFICANT]} loading={false} error={null} />,
    )
    // bayes_prob 0.97 → 97.0%
    expect(html).toMatch(/97\.0%/)
    // credible interval
    expect(html).toMatch(/\[0\.0100, 0\.0700\]/)
  })

  it('renders a "Significant" warning badge for significant rows', () => {
    const html = renderToString(
      <InteractionsTab interactions={[SIGNIFICANT]} loading={false} error={null} />,
    )
    expect(html).toMatch(/data-significant="true"/)
    expect(html).toMatch(/Significant/)
    expect(html).toMatch(/badge warning/)
  })

  it('renders a "Not significant" success badge for non-significant rows', () => {
    const html = renderToString(
      <InteractionsTab interactions={[NOT_SIGNIFICANT]} loading={false} error={null} />,
    )
    expect(html).toMatch(/data-significant="false"/)
    expect(html).toMatch(/Not significant/)
    expect(html).toMatch(/badge success/)
  })

  it('renders an "Insufficient data" badge and hides stats', () => {
    const html = renderToString(
      <InteractionsTab interactions={[INSUFFICIENT]} loading={false} error={null} />,
    )
    expect(html).toMatch(/data-insufficient="true"/)
    expect(html).toMatch(/Insufficient data/)
    // The 0.0 sentinels must NOT render as numbers, nor as a significance badge.
    expect(html).not.toMatch(/0\.0000/)
    expect(html).not.toMatch(/data-significant/)
    expect(html).not.toMatch(/badge warning/)
    expect(html).not.toMatch(/>Significant</)
    // Bayesian columns should also be hidden (shown as "—")
    expect(html).not.toMatch(/0\.0%/)
  })

  it('renders an empty state when there are no interactions', () => {
    const html = renderToString(
      <InteractionsTab interactions={[]} loading={false} error={null} />,
    )
    expect(html).toMatch(/No interactions/i)
  })

  it('renders a loading state', () => {
    const html = renderToString(
      <InteractionsTab interactions={[]} loading={true} error={null} />,
    )
    expect(html).toMatch(/Loading|loading/)
  })

  it('renders an error banner', () => {
    const html = renderToString(
      <InteractionsTab interactions={[]} loading={false} error="boom" />,
    )
    expect(html).toMatch(/boom/)
  })
})
