/**
 * Bandit Results view tests — FR7, Phase 12 Task 12.2.
 *
 * Node-env harness (no jsdom): render to a string with
 * `react-dom/server.renderToString` and assert on the HTML, plus exercise the
 * pure helpers directly.
 *
 * Coverage:
 *   • buildAllocationSeries — oldest-first, excludes `bandit_objectives`, drops
 *     non-numeric / empty runs.
 *   • deriveConvergenceState / convergenceLabel — Exploring / Converged /
 *     Committed / Rolled out.
 *   • Allocation chart renders polylines from mocked history.
 *   • Convergence badge states.
 *   • Guardrail-violation badge from objective posteriors.
 *   • Hidden / not-applicable for a non-bandit experiment.
 *   • Campaign status + lifecycle timeline.
 */
import { describe, it, expect } from 'vitest'
import { renderToString } from 'react-dom/server'
import {
  BanditTab,
  buildAllocationSeries,
  allocationArmKeys,
  deriveConvergenceState,
  convergenceLabel,
} from './Bandit'
import type {
  BanditState,
  BanditAllocationHistory,
} from '../../../lib/api/bandit'

// ── Fixtures ─────────────────────────────────────────────────────────────────

const HISTORY: BanditAllocationHistory = {
  // Newest-first, as the gateway returns.
  runs: [
    {
      fired_at: '2026-06-03T00:00:00Z',
      action: 'reallocate',
      outcome: 'applied',
      new_allocation: {
        control: 2000,
        treatment: 8000,
        bandit_objectives: { objectives: [] },
      },
    },
    {
      fired_at: '2026-06-02T00:00:00Z',
      action: 'reallocate',
      outcome: 'applied',
      new_allocation: { control: 4000, treatment: 6000 },
    },
    {
      fired_at: '2026-06-01T00:00:00Z',
      action: 'reallocate',
      outcome: 'applied',
      new_allocation: { control: 5000, treatment: 5000 },
    },
  ],
}

const STATE: BanditState = {
  experiment_id: 'exp-1',
  is_bandit: true,
  current_allocation: [
    { variant_key: 'treatment', weight_bp: 8000 },
    { variant_key: 'control', weight_bp: 2000 },
  ],
  bandit_config: { algorithm: 'thompson' },
  has_converged: false,
  committed: false,
  objectives: {
    objectives: [
      {
        metric_id: 'metric-conv',
        role: 'scalar',
        goal: 'increase',
        variants: [
          {
            variant_key: 'control',
            mean: 0.1,
            ci_lower: 0.08,
            ci_upper: 0.12,
            n: 1000,
            guardrail_violated: false,
          },
          {
            variant_key: 'treatment',
            mean: 0.3,
            ci_lower: 0.27,
            ci_upper: 0.33,
            n: 1000,
            guardrail_violated: false,
          },
        ],
      },
    ],
  },
}

// ── buildAllocationSeries ────────────────────────────────────────────────────

describe('buildAllocationSeries', () => {
  it('returns points oldest-first', () => {
    const series = buildAllocationSeries(HISTORY.runs)
    expect(series.map((p) => p.firedAt)).toEqual([
      '2026-06-01T00:00:00Z',
      '2026-06-02T00:00:00Z',
      '2026-06-03T00:00:00Z',
    ])
  })

  it('excludes the reserved bandit_objectives key', () => {
    const series = buildAllocationSeries(HISTORY.runs)
    const last = series[series.length - 1]
    expect(Object.keys(last.weights).sort()).toEqual(['control', 'treatment'])
    expect(last.weights).not.toHaveProperty('bandit_objectives')
  })

  it('keeps numeric arm weights', () => {
    const series = buildAllocationSeries(HISTORY.runs)
    expect(series[0].weights).toEqual({ control: 5000, treatment: 5000 })
    expect(series[2].weights).toEqual({ control: 2000, treatment: 8000 })
  })

  it('drops runs with no usable allocation', () => {
    const series = buildAllocationSeries([
      { fired_at: 't1', action: 'skip', outcome: 'skipped', new_allocation: null },
      {
        fired_at: 't2',
        action: 'reallocate',
        outcome: 'applied',
        new_allocation: { bandit_objectives: {} },
      },
    ])
    expect(series).toEqual([])
  })

  it('allocationArmKeys collects + sorts all arms', () => {
    const series = buildAllocationSeries(HISTORY.runs)
    expect(allocationArmKeys(series)).toEqual(['control', 'treatment'])
  })
})

// ── deriveConvergenceState / convergenceLabel ────────────────────────────────

describe('deriveConvergenceState', () => {
  it('exploring when not converged', () => {
    const s = deriveConvergenceState({ ...STATE, has_converged: false, committed: false })
    expect(s.kind).toBe('exploring')
    expect(convergenceLabel(s)).toBe('Exploring')
  })

  it('converged when a winner is known but not committed', () => {
    const s = deriveConvergenceState({
      ...STATE,
      has_converged: true,
      committed: false,
      converged_variant: 'treatment',
      converged_prob: 0.96,
    })
    expect(s.kind).toBe('converged')
    expect(convergenceLabel(s)).toBe('Converged: treatment (96%)')
  })

  it('committed when committed but campaign still active', () => {
    const s = deriveConvergenceState({
      ...STATE,
      has_converged: true,
      committed: true,
      converged_variant: 'treatment',
      converged_prob: 0.99,
    })
    expect(s.kind).toBe('committed')
    expect(convergenceLabel(s)).toBe('Committed: treatment (99%)')
  })

  it('rolled out when committed + campaign concluded', () => {
    const s = deriveConvergenceState({
      ...STATE,
      has_converged: true,
      committed: true,
      converged_variant: 'treatment',
      converged_prob: 0.99,
      campaign_id: 'camp-1',
      campaign_status: 'concluded',
    })
    expect(s.kind).toBe('rolled_out')
    expect(convergenceLabel(s)).toBe('Rolled out: treatment (99%)')
  })
})

// ── Render ───────────────────────────────────────────────────────────────────

describe('BanditTab render', () => {
  it('renders the allocation chart with polylines from mocked history', () => {
    const html = renderToString(
      <BanditTab state={STATE} history={HISTORY} loading={false} error={null} />,
    )
    expect(html).toMatch(/data-testid="allocation-chart"/)
    expect(html).toMatch(/Allocation over time/)
    expect(html).toMatch(/<polyline/)
    // Both arms in the legend.
    expect(html).toMatch(/control/)
    expect(html).toMatch(/treatment/)
  })

  it('renders the current weights table', () => {
    const html = renderToString(
      <BanditTab state={STATE} history={HISTORY} loading={false} error={null} />,
    )
    expect(html).toMatch(/Current weights/)
    // treatment 8000bp → 80.0%
    expect(html).toMatch(/80\.0%/)
  })

  it('renders the exploring convergence badge by default', () => {
    const html = renderToString(
      <BanditTab state={STATE} history={HISTORY} loading={false} error={null} />,
    )
    expect(html).toMatch(/data-convergence="exploring"/)
    expect(html).toMatch(/Exploring/)
  })

  it('renders a converged badge when a winner is declared', () => {
    const converged: BanditState = {
      ...STATE,
      has_converged: true,
      converged_variant: 'treatment',
      converged_prob: 0.97,
    }
    const html = renderToString(
      <BanditTab state={converged} history={HISTORY} loading={false} error={null} />,
    )
    expect(html).toMatch(/data-convergence="converged"/)
    // The badge text is a single pre-built string (no SSR comment markers).
    expect(html).toMatch(/Converged: treatment \(97%\)/)
  })

  it('renders a committed badge', () => {
    const committed: BanditState = {
      ...STATE,
      has_converged: true,
      committed: true,
      converged_variant: 'treatment',
    }
    const html = renderToString(
      <BanditTab state={committed} history={HISTORY} loading={false} error={null} />,
    )
    expect(html).toMatch(/data-convergence="committed"/)
  })

  it('renders objective posteriors with mean + CI', () => {
    const html = renderToString(
      <BanditTab state={STATE} history={HISTORY} loading={false} error={null} />,
    )
    // "Objective · " and the metric id are separate JSX segments (SSR markers).
    expect(html).toMatch(/Objective ·[^<]*<!-- -->metric-conv/)
    expect(html).toMatch(/0\.3000/)
    // CI bounds are separate JSX segments too.
    expect(html).toMatch(/0\.2700/)
    expect(html).toMatch(/0\.3300/)
  })

  it('renders a guardrail-violation badge when an arm violates a guardrail', () => {
    const violated: BanditState = {
      ...STATE,
      objectives: {
        objectives: [
          {
            metric_id: 'metric-guard',
            role: 'guardrail',
            goal: 'increase',
            variants: [
              {
                variant_key: 'treatment',
                mean: 0.05,
                ci_lower: 0.03,
                ci_upper: 0.07,
                n: 800,
                guardrail_violated: true,
              },
            ],
          },
        ],
      },
    }
    const html = renderToString(
      <BanditTab state={violated} history={HISTORY} loading={false} error={null} />,
    )
    expect(html).toMatch(/data-testid="guardrail-badge"/)
    expect(html).toMatch(/Guardrail violated/)
    expect(html).toMatch(/data-guardrail-violated="true"/)
  })

  it('renders the lifecycle timeline with action rows', () => {
    const html = renderToString(
      <BanditTab state={STATE} history={HISTORY} loading={false} error={null} />,
    )
    expect(html).toMatch(/Lifecycle/)
    expect(html).toMatch(/data-action="reallocate"/)
    expect(html).toMatch(/Reallocated/)
  })

  it('renders campaign status when a campaign is attached', () => {
    const withCampaign: BanditState = {
      ...STATE,
      campaign_id: 'camp-1',
      campaign_status: 'active',
    }
    const html = renderToString(
      <BanditTab state={withCampaign} history={HISTORY} loading={false} error={null} />,
    )
    expect(html).toMatch(/data-testid="campaign-status"/)
    // SSR comment markers split "Campaign: " and "active".
    expect(html).toMatch(/Campaign:[^<]*<!-- -->active/)
  })

  it('renders a not-applicable message for a non-bandit experiment', () => {
    const fixed: BanditState = { ...STATE, is_bandit: false }
    const html = renderToString(
      <BanditTab state={fixed} history={null} loading={false} error={null} />,
    )
    expect(html).toMatch(/data-testid="bandit-not-applicable"/)
    expect(html).not.toMatch(/data-testid="allocation-chart"/)
  })

  it('renders a not-applicable message when state is null', () => {
    const html = renderToString(
      <BanditTab state={null} history={null} loading={false} error={null} />,
    )
    expect(html).toMatch(/data-testid="bandit-not-applicable"/)
  })

  it('renders a loading state', () => {
    const html = renderToString(
      <BanditTab state={null} history={null} loading={true} error={null} />,
    )
    expect(html).toMatch(/Loading bandit/)
  })

  it('renders an error state', () => {
    const html = renderToString(
      <BanditTab state={null} history={null} loading={false} error="boom" />,
    )
    expect(html).toMatch(/boom/)
  })

  it('shows an empty allocation chart when there is no history', () => {
    const html = renderToString(
      <BanditTab state={STATE} history={{ runs: [] }} loading={false} error={null} />,
    )
    expect(html).toMatch(/No allocation history yet/)
  })
})
