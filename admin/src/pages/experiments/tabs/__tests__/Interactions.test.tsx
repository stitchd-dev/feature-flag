/**
 * Interactions tab — P5 N-way tests.
 *
 * Exercises the N-way (order 2 + 3) rendering, Bayesian columns, order badges,
 * insufficient-data masking, and the generalized hasSignificantInteraction helper
 * (fires on high-bayes-prob rows as well as frequentist-significant rows).
 *
 * Uses renderToString (node env) consistent with the sibling tab tests.
 */
import { describe, it, expect } from 'vitest'
import { renderToString } from 'react-dom/server'
import {
  InteractionsTab,
  hasSignificantInteraction,
} from '../Interactions'
import type { ExperimentInteraction } from '../../../../lib/api/exclusionGroups'

// ─── Fixtures ────────────────────────────────────────────────────────────────

const TWO_WAY_SIGNIFICANT: ExperimentInteraction = {
  experiment_ids: ['exp-a', 'exp-b'],
  experiment_names: ['Checkout Redesign', 'Pricing Banner'],
  interaction_order: 2,
  term: '2way:exp-axexp-b',
  context_type: 'user',
  metric_key: 'checkout_completed',
  shared_count: 9800,
  interaction_estimate: 0.052,
  p_value: 0.0012,
  df: 1,
  significant: true,
  insufficient_data: false,
  bayes_prob: 0.97,
  bayes_expected: 0.049,
  bayes_ci_low: 0.015,
  bayes_ci_high: 0.085,
}

const THREE_WAY_ROW: ExperimentInteraction = {
  experiment_ids: ['exp-a', 'exp-b', 'exp-c'],
  experiment_names: ['Checkout Redesign', 'Pricing Banner', 'Homepage Hero'],
  interaction_order: 3,
  term: '3way:exp-axexp-bxexp-c',
  context_type: 'user',
  metric_key: 'revenue_per_user',
  shared_count: 3200,
  interaction_estimate: -0.008,
  p_value: 0.18,
  df: 2,
  significant: false,
  insufficient_data: false,
  bayes_prob: 0.62,
  bayes_expected: -0.006,
  bayes_ci_low: -0.02,
  bayes_ci_high: 0.008,
}

const INSUFFICIENT_DATA: ExperimentInteraction = {
  experiment_ids: ['exp-a', 'exp-d'],
  experiment_names: ['Checkout Redesign', 'Onboarding Flow'],
  interaction_order: 2,
  term: '2way:exp-axexp-d',
  context_type: 'session',
  metric_key: 'activation_rate',
  shared_count: 7,
  interaction_estimate: 0.0,
  p_value: 0.0,
  df: 1,
  // significant may be true on the wire but must be masked by insufficient_data
  significant: true,
  insufficient_data: true,
  bayes_prob: 0.0,
  bayes_expected: 0.0,
  bayes_ci_low: 0.0,
  bayes_ci_high: 0.0,
}

// A row that is NOT frequentist-significant but has high bayes_prob (> 0.95).
const HIGH_BAYES_ONLY: ExperimentInteraction = {
  experiment_ids: ['exp-e', 'exp-f'],
  experiment_names: ['Layout Test', 'CTA Color'],
  interaction_order: 2,
  term: '2way:exp-exexp-f',
  context_type: 'user',
  metric_key: 'click_rate',
  shared_count: 40000,
  interaction_estimate: 0.007,
  p_value: 0.06,
  df: 1,
  significant: false,
  insufficient_data: false,
  bayes_prob: 0.96,
  bayes_expected: 0.007,
  bayes_ci_low: 0.001,
  bayes_ci_high: 0.013,
}

// A row where all metrics say "not significant".
const BENIGN: ExperimentInteraction = {
  experiment_ids: ['exp-g', 'exp-h'],
  experiment_names: ['Font Size', 'Button Shape'],
  interaction_order: 2,
  term: '2way:exp-gxexp-h',
  context_type: 'user',
  metric_key: 'bounce_rate',
  shared_count: 500,
  interaction_estimate: 0.001,
  p_value: 0.73,
  df: 1,
  significant: false,
  insufficient_data: false,
  bayes_prob: 0.21,
  bayes_expected: 0.0009,
  bayes_ci_low: -0.003,
  bayes_ci_high: 0.005,
}

// ─── hasSignificantInteraction ────────────────────────────────────────────────

describe('hasSignificantInteraction (N-way + Bayesian)', () => {
  it('returns true when a frequentist-significant row is present', () => {
    expect(hasSignificantInteraction([BENIGN, TWO_WAY_SIGNIFICANT])).toBe(true)
  })

  it('returns true when only a high-bayes-prob (> 0.95) row is present', () => {
    expect(hasSignificantInteraction([HIGH_BAYES_ONLY])).toBe(true)
  })

  it('returns false for an all-insufficient set', () => {
    expect(hasSignificantInteraction([INSUFFICIENT_DATA])).toBe(false)
  })

  it('returns false when no rows are significant and all bayes_prob ≤ 0.95', () => {
    expect(hasSignificantInteraction([BENIGN, THREE_WAY_ROW])).toBe(false)
  })

  it('ignores insufficient rows even with high bayes_prob', () => {
    const highBayesInsuf: ExperimentInteraction = {
      ...INSUFFICIENT_DATA,
      bayes_prob: 0.99,
    }
    expect(hasSignificantInteraction([highBayesInsuf])).toBe(false)
  })

  it('returns false for an empty list', () => {
    expect(hasSignificantInteraction([])).toBe(false)
  })
})

// ─── Table render ─────────────────────────────────────────────────────────────

describe('InteractionsTab — N-way render', () => {
  it('renders order badges for 2-way and 3-way rows', () => {
    const html = renderToString(
      <InteractionsTab
        interactions={[TWO_WAY_SIGNIFICANT, THREE_WAY_ROW]}
        loading={false}
        error={null}
      />,
    )
    expect(html).toMatch(/data-order="2"/)
    expect(html).toMatch(/data-order="3"/)
    expect(html).toMatch(/2-way/)
    expect(html).toMatch(/3-way/)
  })

  it('renders all experiment names in the Interaction column', () => {
    const html = renderToString(
      <InteractionsTab
        interactions={[THREE_WAY_ROW]}
        loading={false}
        error={null}
      />,
    )
    expect(html).toMatch(/Checkout Redesign/)
    expect(html).toMatch(/Pricing Banner/)
    expect(html).toMatch(/Homepage Hero/)
  })

  it('renders Bayesian prob column as a percentage', () => {
    const html = renderToString(
      <InteractionsTab
        interactions={[TWO_WAY_SIGNIFICANT]}
        loading={false}
        error={null}
      />,
    )
    // bayes_prob 0.97 → 97.0%
    expect(html).toMatch(/97\.0%/)
  })

  it('renders Bayesian credible interval column', () => {
    const html = renderToString(
      <InteractionsTab
        interactions={[TWO_WAY_SIGNIFICANT]}
        loading={false}
        error={null}
      />,
    )
    expect(html).toMatch(/\[0\.0150, 0\.0850\]/)
  })

  it('renders "Significant" badge for frequentist-significant row', () => {
    const html = renderToString(
      <InteractionsTab
        interactions={[TWO_WAY_SIGNIFICANT]}
        loading={false}
        error={null}
      />,
    )
    expect(html).toMatch(/data-significant="true"/)
    expect(html).toMatch(/Significant/)
    expect(html).toMatch(/badge warning/)
  })

  it('renders "Not significant" badge for non-significant row', () => {
    const html = renderToString(
      <InteractionsTab
        interactions={[THREE_WAY_ROW]}
        loading={false}
        error={null}
      />,
    )
    expect(html).toMatch(/data-significant="false"/)
    expect(html).toMatch(/Not significant/)
    expect(html).toMatch(/badge success/)
  })

  it('renders "Insufficient data" badge and hides all stats', () => {
    const html = renderToString(
      <InteractionsTab
        interactions={[INSUFFICIENT_DATA]}
        loading={false}
        error={null}
      />,
    )
    expect(html).toMatch(/data-insufficient="true"/)
    expect(html).toMatch(/Insufficient data/)
    // sentinel 0.0 values must not appear as real numbers
    expect(html).not.toMatch(/0\.0000/)
    // no significance badge should appear
    expect(html).not.toMatch(/data-significant/)
    expect(html).not.toMatch(/badge warning/)
    expect(html).not.toMatch(/>Significant</)
  })

  it('renders a mix of 2-way significant, 3-way, and insufficient rows correctly', () => {
    const html = renderToString(
      <InteractionsTab
        interactions={[TWO_WAY_SIGNIFICANT, THREE_WAY_ROW, INSUFFICIENT_DATA]}
        loading={false}
        error={null}
      />,
    )
    // All three rows present
    expect(html).toMatch(/checkout_completed/)
    expect(html).toMatch(/revenue_per_user/)
    expect(html).toMatch(/activation_rate/)
    // Badges
    expect(html).toMatch(/data-order="2"/)
    expect(html).toMatch(/data-order="3"/)
    expect(html).toMatch(/data-insufficient="true"/)
    expect(html).toMatch(/data-significant="true"/)
    expect(html).toMatch(/data-significant="false"/)
  })
})
