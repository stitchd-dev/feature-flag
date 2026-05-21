/**
 * Results tab — Phase 9 Task 9.1 tests.
 *
 * Strategy: the admin vitest env is `environment: 'node'` (no jsdom), per the
 * Phase 8 convention. We render the component to a string with
 * `react-dom/server.renderToString` and assert on the resulting HTML shape.
 * Interactive behaviour (toggle persistence) is exercised via the
 * `viewToggleStorageKey` + `readPersistedView` pure helpers exported alongside
 * the component.
 *
 * Coverage:
 *   • Frequentist view renders mean ± CI, p-value, lift columns.
 *   • Bayesian view renders P(beats control) + credible interval.
 *   • Winner highlighting respects `goal_direction = increase` (positive lift wins)
 *     vs `goal_direction = decrease` (negative lift wins).
 *   • Multi-variant matrix appears only when variants > 2.
 *   • Bonferroni-corrected p-value annotation appears when variants > 2.
 *   • View-toggle storage key is `experiment_${experimentId}_results_view`.
 *   • Persisted view is restored from localStorage on mount.
 */
import { describe, it, expect, beforeEach, afterEach } from 'vitest'
import { renderToString } from 'react-dom/server'
import {
  Results,
  viewToggleStorageKey,
  readPersistedView,
} from './Results'
import type { ExperimentResults, VariantResultJson } from '../../../lib/types'

// ── localStorage shim (same approach as ContextTypeContext.test.tsx) ─────────

interface FakeStorage {
  store: Record<string, string>
  getItem: (k: string) => string | null
  setItem: (k: string, v: string) => void
  removeItem: (k: string) => void
  clear: () => void
  readonly length: number
  key: (i: number) => string | null
}

function makeFakeStorage(initial: Record<string, string> = {}): FakeStorage {
  const store = { ...initial }
  return {
    store,
    getItem: (k) => (k in store ? store[k] : null),
    setItem: (k, v) => {
      store[k] = String(v)
    },
    removeItem: (k) => {
      delete store[k]
    },
    clear: () => {
      Object.keys(store).forEach((k) => delete store[k])
    },
    get length() {
      return Object.keys(store).length
    },
    key: (i) => Object.keys(store)[i] ?? null,
  }
}

const globalAny = globalThis as unknown as {
  window?: unknown
  localStorage?: FakeStorage
}
let originalWindow: unknown
let originalLocalStorage: FakeStorage | undefined

beforeEach(() => {
  originalWindow = globalAny.window
  originalLocalStorage = globalAny.localStorage
  globalAny.window = { localStorage: makeFakeStorage() }
  globalAny.localStorage = (globalAny.window as { localStorage: FakeStorage })
    .localStorage
})

afterEach(() => {
  globalAny.window = originalWindow
  globalAny.localStorage = originalLocalStorage
})

// ── Fixtures ─────────────────────────────────────────────────────────────────

const CONTROL: VariantResultJson = {
  variant_key: 'control',
  assigned_count: 1000,
  mean: 0.10,
  sample_size: 1000,
  p_value: null,
  ci_lower: 0.09,
  ci_upper: 0.11,
  prob_to_beat_control: null,
  expected_lift: null,
  is_winner: false,
}

const TREATMENT_A: VariantResultJson = {
  variant_key: 'variant-a',
  assigned_count: 1000,
  mean: 0.12,
  sample_size: 1000,
  p_value: 0.012,
  ci_lower: 0.105,
  ci_upper: 0.135,
  prob_to_beat_control: 0.95,
  expected_lift: 0.04, // +4.0% lift
  is_winner: true,
}

const TREATMENT_B: VariantResultJson = {
  variant_key: 'variant-b',
  assigned_count: 1000,
  mean: 0.115,
  sample_size: 1000,
  p_value: 0.18,
  ci_lower: 0.10,
  ci_upper: 0.13,
  prob_to_beat_control: 0.62,
  expected_lift: 0.02,
  is_winner: false,
}

function buildResults(
  variants: VariantResultJson[],
): ExperimentResults {
  return {
    results_by_context_type: {
      user: {
        variants,
        srm: { per_variant: [], overall_chi_sq_p: 0.5, health: 'green' },
        guardrails: [],
      },
    },
    bound_target: { kind: 'rule', rule_id: 'rule-1', label: 'checkout-rule' },
    pre_period_days: 0,
  }
}

const TWO_VARIANT = buildResults([CONTROL, TREATMENT_A])
const THREE_VARIANT = buildResults([CONTROL, TREATMENT_A, TREATMENT_B])

// ── viewToggleStorageKey ─────────────────────────────────────────────────────

describe('viewToggleStorageKey', () => {
  it('formats key as experiment_${id}_results_view', () => {
    expect(viewToggleStorageKey('checkout-v2')).toBe(
      'experiment_checkout-v2_results_view',
    )
  })

  it('preserves UUID-style ids verbatim', () => {
    const id = 'a1b2c3d4-1234-5678-9abc-def012345678'
    expect(viewToggleStorageKey(id)).toBe(`experiment_${id}_results_view`)
  })
})

// ── readPersistedView ────────────────────────────────────────────────────────

describe('readPersistedView', () => {
  it('returns "frequentist" when nothing persisted', () => {
    expect(readPersistedView('exp-1', 'frequentist')).toBe('frequentist')
  })

  it('returns "bayesian" when stored', () => {
    globalAny.localStorage!.setItem(
      viewToggleStorageKey('exp-2'),
      'bayesian',
    )
    expect(readPersistedView('exp-2', 'frequentist')).toBe('bayesian')
  })

  it('falls back to default when stored value is invalid', () => {
    globalAny.localStorage!.setItem(viewToggleStorageKey('exp-3'), 'garbage')
    expect(readPersistedView('exp-3', 'bayesian')).toBe('bayesian')
  })

  it('honours the experiment.model default when no persisted value', () => {
    expect(readPersistedView('exp-4', 'bayesian')).toBe('bayesian')
  })
})

// ── Results component ────────────────────────────────────────────────────────

describe('Results (component)', () => {
  it('renders a card for two-variant frequentist results', () => {
    const html = renderToString(
      <Results
        experimentId="exp-1"
        activeContextType="user"
        results={TWO_VARIANT}
        defaultModel="frequentist"
        goalDirection="increase"
        metricNames={['conversion']}
      />,
    )
    expect(html).toMatch(/variant-a/)
    expect(html).toMatch(/control/)
    // Frequentist columns
    expect(html).toMatch(/p-value/i)
    // The CI range should be rendered (0.105, 0.135 → +10.5%, +13.5% formatted)
    expect(html).toMatch(/0\.\d+/)
  })

  it('highlights winner for goal_direction=increase when lift is positive', () => {
    const html = renderToString(
      <Results
        experimentId="exp-1"
        activeContextType="user"
        results={TWO_VARIANT}
        defaultModel="frequentist"
        goalDirection="increase"
        metricNames={['conversion']}
      />,
    )
    // Winner row gets the data-winner attribute hook so tests can pin it
    // without depending on exact background-color CSS.
    expect(html).toMatch(/data-winner="true"[^>]*>[\s\S]*?variant-a/)
  })

  it('does NOT highlight a positive-lift winner when goal_direction=decrease', () => {
    // For a decrease-direction metric, +4% lift is bad — NO winner highlight.
    const html = renderToString(
      <Results
        experimentId="exp-1"
        activeContextType="user"
        results={TWO_VARIANT}
        defaultModel="frequentist"
        goalDirection="decrease"
        metricNames={['conversion']}
      />,
    )
    // The control row + variant-a row both rendered, but no data-winner=true
    // on variant-a since the lift goes the wrong way.
    expect(html).not.toMatch(/data-winner="true"/)
  })

  it('highlights winner for goal_direction=decrease when lift is negative', () => {
    // Mirror fixture: variant-a has -4% lift, decrease direction → winner.
    const decreasingWinner: VariantResultJson = {
      ...TREATMENT_A,
      expected_lift: -0.04,
      is_winner: true,
    }
    const results = buildResults([CONTROL, decreasingWinner])
    const html = renderToString(
      <Results
        experimentId="exp-1"
        activeContextType="user"
        results={results}
        defaultModel="frequentist"
        goalDirection="decrease"
        metricNames={['conversion']}
      />,
    )
    expect(html).toMatch(/data-winner="true"[^>]*>[\s\S]*?variant-a/)
  })

  it('renders three-variant matrix when variants > 2', () => {
    const html = renderToString(
      <Results
        experimentId="exp-1"
        activeContextType="user"
        results={THREE_VARIANT}
        defaultModel="frequentist"
        goalDirection="increase"
        metricNames={['conversion']}
      />,
    )
    expect(html).toMatch(/variant-b/)
    // Multi-variant matrix has a marker hook.
    expect(html).toMatch(/data-testid="multivariant-matrix"/)
  })

  it('does NOT render multi-variant matrix when variants = 2', () => {
    const html = renderToString(
      <Results
        experimentId="exp-1"
        activeContextType="user"
        results={TWO_VARIANT}
        defaultModel="frequentist"
        goalDirection="increase"
        metricNames={['conversion']}
      />,
    )
    expect(html).not.toMatch(/data-testid="multivariant-matrix"/)
  })

  it('mentions Bonferroni correction when variants > 2', () => {
    const html = renderToString(
      <Results
        experimentId="exp-1"
        activeContextType="user"
        results={THREE_VARIANT}
        defaultModel="frequentist"
        goalDirection="increase"
        metricNames={['conversion']}
      />,
    )
    expect(html).toMatch(/Bonferroni/i)
  })

  it('renders bayesian columns when defaultModel=bayesian', () => {
    const html = renderToString(
      <Results
        experimentId="exp-1"
        activeContextType="user"
        results={TWO_VARIANT}
        defaultModel="bayesian"
        goalDirection="increase"
        metricNames={['conversion']}
      />,
    )
    // Bayesian view shows P(beats control) and credible interval markers
    expect(html).toMatch(/P\(/) // P(...
    // The winning variant's prob_to_beat_control = 0.95 → "95%"
    expect(html).toMatch(/95%/)
  })

  it('renders an empty-state when results has no rows for the active context', () => {
    const empty = buildResults([])
    const html = renderToString(
      <Results
        experimentId="exp-1"
        activeContextType="user"
        results={empty}
        defaultModel="frequentist"
        goalDirection="increase"
        metricNames={['conversion']}
      />,
    )
    // Empty state — no rows means no variant_key text or data-winner.
    expect(html).not.toMatch(/data-winner="true"/)
    expect(html).toMatch(/No (results|data)/i)
  })

  it('renders a missing-context message when activeContextType is absent from results', () => {
    // The Results component reads by activeContextType; if the requested
    // context is absent (e.g. results structure exists but no key for it),
    // we still want the page to fall back to the first available type via
    // the same picker used in helpers.
    const html = renderToString(
      <Results
        experimentId="exp-1"
        activeContextType="missing-type"
        results={TWO_VARIANT}
        defaultModel="frequentist"
        goalDirection="increase"
        metricNames={['conversion']}
      />,
    )
    // Falls back to "user" (the only key present) — variant rows still render.
    expect(html).toMatch(/variant-a/)
  })
})
