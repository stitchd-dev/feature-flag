/**
 * Exposures tab — Phase 9 Task 9.2 tests.
 *
 * Coverage (node env + renderToString per Phase 8 convention):
 *   • Per-variant SRM table: assigned / expected / deviation / chi-sq.
 *   • Health pill class mapping for green / yellow / red.
 *   • Red pill when overall_chi_sq_p < 0.001.
 *   • "Bound to: <label>" badge variants — rule + default_rule.
 *   • Pure helper `srmHealthClass()` covers all three thresholds.
 *   • Pure helper `clampPage()` clamps URL `?page=` to [1, totalPages].
 */
import { describe, it, expect } from 'vitest'
import { renderToString } from 'react-dom/server'
import {
  ExposuresPanel,
  srmHealthClass,
  clampPage,
} from './Exposures'
import type {
  ExperimentResults,
  SrmResultJson,
  ExposureRow,
} from '../../../lib/types'

// ── Fixtures ─────────────────────────────────────────────────────────────────

const HEALTHY_SRM: SrmResultJson = {
  per_variant: [
    { variant_key: 'control', observed_count: 1010, expected_count: 1000, deviation: 0.01 },
    { variant_key: 'variant-a', observed_count: 990, expected_count: 1000, deviation: -0.01 },
  ],
  overall_chi_sq_p: 0.42,
  health: 'green',
}

const RED_SRM: SrmResultJson = {
  per_variant: [
    { variant_key: 'control', observed_count: 1500, expected_count: 1000, deviation: 0.5 },
    { variant_key: 'variant-a', observed_count: 500, expected_count: 1000, deviation: -0.5 },
  ],
  overall_chi_sq_p: 0.0005, // < 0.001 → red
  health: 'red',
}

function buildResults(
  srm: SrmResultJson,
  boundLabel = 'checkout-rule',
  boundKind: 'rule' | 'default_rule' = 'rule',
): ExperimentResults {
  return {
    results_by_context_type: {
      user: {
        variants: [],
        srm,
        guardrails: [],
      },
    },
    bound_target: {
      kind: boundKind,
      rule_id: boundKind === 'rule' ? 'rule-1' : null,
      label: boundLabel,
    },
    pre_period_days: 0,
  }
}

const EXPOSURE_ROWS: ExposureRow[] = [
  {
    context_type: 'user',
    context_key: 'alice',
    variant_key: 'control',
    assigned_at: '2026-05-19T10:00:00Z',
    matched_rule_id: 'rule-1',
  },
  {
    context_type: 'user',
    context_key: 'bob',
    variant_key: 'variant-a',
    assigned_at: '2026-05-19T11:00:00Z',
    matched_rule_id: null, // default-rule fallthrough
  },
]

// ── srmHealthClass (pure helper) ─────────────────────────────────────────────

describe('srmHealthClass', () => {
  it('returns "red" when chi_sq_p < 0.001', () => {
    expect(srmHealthClass(0.0005)).toBe('red')
  })

  it('returns "yellow" when 0.001 <= chi_sq_p < 0.01', () => {
    expect(srmHealthClass(0.005)).toBe('yellow')
  })

  it('returns "green" when chi_sq_p >= 0.01', () => {
    expect(srmHealthClass(0.5)).toBe('green')
  })

  it('returns "green" at exactly 0.01 (boundary)', () => {
    expect(srmHealthClass(0.01)).toBe('green')
  })

  it('returns "red" at exactly 0.0009 (boundary)', () => {
    expect(srmHealthClass(0.0009)).toBe('red')
  })

  it('returns "yellow" at exactly 0.001 (boundary)', () => {
    expect(srmHealthClass(0.001)).toBe('yellow')
  })

  it('returns "green" when chi_sq_p is null (no data)', () => {
    expect(srmHealthClass(null)).toBe('green')
  })
})

// ── clampPage (URL page param helper) ────────────────────────────────────────

describe('clampPage', () => {
  it('clamps below 1 → 1', () => {
    expect(clampPage(0, 100)).toBe(1)
    expect(clampPage(-5, 100)).toBe(1)
  })

  it('clamps above totalPages → totalPages', () => {
    expect(clampPage(99, 3)).toBe(3)
  })

  it('returns page when within range', () => {
    expect(clampPage(2, 5)).toBe(2)
  })

  it('handles totalPages = 0 → 1', () => {
    expect(clampPage(1, 0)).toBe(1)
  })

  it('parses string page params', () => {
    expect(clampPage('2', 5)).toBe(2)
    expect(clampPage('abc', 5)).toBe(1) // NaN → 1
  })
})

// ── ExposuresPanel render ────────────────────────────────────────────────────

describe('ExposuresPanel', () => {
  it('renders the "Bound to: <rule label>" badge for rule kind', () => {
    const r = buildResults(HEALTHY_SRM, 'checkout-rule', 'rule')
    const html = renderToString(
      <ExposuresPanel
        results={r}
        activeContextType="user"
        exposures={null}
        page={1}
        onPageChange={() => {}}
        loading={false}
        error={null}
      />,
    )
    expect(html).toMatch(/Bound to/i)
    expect(html).toMatch(/checkout-rule/)
  })

  it('renders the "Default rule" label for default_rule kind', () => {
    const r = buildResults(HEALTHY_SRM, 'Default rule (fallthrough)', 'default_rule')
    const html = renderToString(
      <ExposuresPanel
        results={r}
        activeContextType="user"
        exposures={null}
        page={1}
        onPageChange={() => {}}
        loading={false}
        error={null}
      />,
    )
    expect(html).toMatch(/Default rule \(fallthrough\)/)
  })

  it('renders a green pill when SRM is healthy', () => {
    const r = buildResults(HEALTHY_SRM)
    const html = renderToString(
      <ExposuresPanel
        results={r}
        activeContextType="user"
        exposures={null}
        page={1}
        onPageChange={() => {}}
        loading={false}
        error={null}
      />,
    )
    expect(html).toMatch(/data-srm-health="green"/)
  })

  it('renders a red pill when chi_sq_p < 0.001', () => {
    const r = buildResults(RED_SRM)
    const html = renderToString(
      <ExposuresPanel
        results={r}
        activeContextType="user"
        exposures={null}
        page={1}
        onPageChange={() => {}}
        loading={false}
        error={null}
      />,
    )
    expect(html).toMatch(/data-srm-health="red"/)
  })

  it('renders per-variant SRM rows', () => {
    const r = buildResults(HEALTHY_SRM)
    const html = renderToString(
      <ExposuresPanel
        results={r}
        activeContextType="user"
        exposures={null}
        page={1}
        onPageChange={() => {}}
        loading={false}
        error={null}
      />,
    )
    expect(html).toMatch(/control/)
    expect(html).toMatch(/variant-a/)
    // Observed counts surfaced
    expect(html).toMatch(/1[,.]?010/)
    expect(html).toMatch(/990/)
  })

  it('renders the paginated exposure table when exposures is non-null', () => {
    const r = buildResults(HEALTHY_SRM)
    const html = renderToString(
      <ExposuresPanel
        results={r}
        activeContextType="user"
        exposures={{
          items: EXPOSURE_ROWS,
          total: 2,
          page: 1,
          per_page: 50,
        }}
        page={1}
        onPageChange={() => {}}
        loading={false}
        error={null}
      />,
    )
    // Two context keys rendered
    expect(html).toMatch(/alice/)
    expect(html).toMatch(/bob/)
    // matched_rule_id null → rendered as "—"
    expect(html).toMatch(/—/)
  })

  it('renders a loading state when loading is true and no exposures yet', () => {
    const r = buildResults(HEALTHY_SRM)
    const html = renderToString(
      <ExposuresPanel
        results={r}
        activeContextType="user"
        exposures={null}
        page={1}
        onPageChange={() => {}}
        loading={true}
        error={null}
      />,
    )
    expect(html).toMatch(/Loading|loading/)
  })

  it('renders an error banner when error is set', () => {
    const r = buildResults(HEALTHY_SRM)
    const html = renderToString(
      <ExposuresPanel
        results={r}
        activeContextType="user"
        exposures={null}
        page={1}
        onPageChange={() => {}}
        loading={false}
        error="something exploded"
      />,
    )
    expect(html).toMatch(/something exploded/)
  })

  it('scopes the panel title to the active context type', () => {
    const r = buildResults(HEALTHY_SRM)
    const html = renderToString(
      <ExposuresPanel
        results={r}
        activeContextType="account"
        exposures={null}
        page={1}
        onPageChange={() => {}}
        loading={false}
        error={null}
      />,
    )
    // Per spec §4: "Exposures (<context_type>)" so the scope is unmissable.
    expect(html).toMatch(/Exposures.*account|account.*Exposures/i)
  })
})
