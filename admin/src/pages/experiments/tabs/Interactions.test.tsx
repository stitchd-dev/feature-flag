/**
 * Interactions tab tests (Phase 8 — xexp, P8.T3).
 *
 * Coverage (node env + renderToString):
 *   • Pure helpers: hasSignificantInteraction, formatPValue, formatEstimate.
 *   • Table render: other experiment name, context type, metric, shared count,
 *     estimate, p-value, significance badge.
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
} from './Interactions'
import type { ExperimentInteraction } from '../../../lib/api/exclusionGroups'

const SIGNIFICANT: ExperimentInteraction = {
  experiment_id_a: 'exp-a',
  experiment_id_b: 'exp-b',
  other_experiment_name: 'Pricing banner test',
  context_type: 'user',
  metric_key: 'checkout_completed',
  shared_count: 12500,
  interaction_estimate: 0.0421,
  p_value: 0.0003,
  significant: true,
  insufficient_data: false,
}

const NOT_SIGNIFICANT: ExperimentInteraction = {
  experiment_id_a: 'exp-a',
  experiment_id_b: 'exp-c',
  other_experiment_name: 'Homepage hero',
  context_type: 'account',
  metric_key: 'revenue_per_user',
  shared_count: 800,
  interaction_estimate: -0.001,
  p_value: 0.62,
  significant: false,
  insufficient_data: false,
}

// Backend returns 0.0 sentinels + insufficient_data=true when there isn't
// enough shared exposure. The `significant` flag may even be true on the wire,
// but the row must never be treated as a real significant result.
const INSUFFICIENT: ExperimentInteraction = {
  experiment_id_a: 'exp-a',
  experiment_id_b: 'exp-d',
  other_experiment_name: 'New onboarding flow',
  context_type: 'user',
  metric_key: 'activation_rate',
  shared_count: 12,
  interaction_estimate: 0.0,
  p_value: 0.0,
  significant: true,
  insufficient_data: true,
}

// ─── Pure helpers ────────────────────────────────────────────────────────────

describe('hasSignificantInteraction', () => {
  it('returns true when any interaction is significant', () => {
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

  it('renders an "Insufficient data" badge instead of p=0.0 / significance', () => {
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
