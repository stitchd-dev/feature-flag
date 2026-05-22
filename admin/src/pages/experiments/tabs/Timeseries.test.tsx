/**
 * Timeseries tab — Phase 9 Task 9.3 tests.
 *
 * Coverage:
 *   • clampDays() bounds the days parameter into [1, 90] per spec.
 *   • bucketsByVariant() folds a Timeseries.buckets[] into per-variant series.
 *   • computeFetchSignature() detects context-type + days + metric-id changes
 *     so the page's fetch effect can re-run only when relevant.
 *   • Timeseries component renders the metric-row list + days selector +
 *     expanded chart slot when a metric is selected.
 *   • Day selector is clamped to [1, 90] and the configured presets are
 *     exposed verbatim.
 */
import { describe, it, expect } from 'vitest'
import { renderToString } from 'react-dom/server'
import {
  Timeseries,
  clampDays,
  bucketsByVariant,
  computeFetchSignature,
  DAY_PRESETS,
} from './Timeseries'
import type { Timeseries as TimeseriesData, TimeseriesBucket } from '../../../lib/types'

// ── Fixtures ─────────────────────────────────────────────────────────────────

const BUCKETS: TimeseriesBucket[] = [
  { day: '2026-05-15', variant_key: 'control', value: 0.10, sample_size: 100 },
  { day: '2026-05-15', variant_key: 'variant-a', value: 0.12, sample_size: 100 },
  { day: '2026-05-16', variant_key: 'control', value: 0.11, sample_size: 110 },
  { day: '2026-05-16', variant_key: 'variant-a', value: 0.13, sample_size: 105 },
  { day: '2026-05-17', variant_key: 'control', value: 0.105, sample_size: 95 },
  { day: '2026-05-17', variant_key: 'variant-a', value: 0.135, sample_size: 90 },
]

const TS_USER: TimeseriesData = {
  metric_id: 'metric-1',
  context_type: 'user',
  buckets: BUCKETS,
}

// ── clampDays ────────────────────────────────────────────────────────────────

describe('clampDays', () => {
  it('clamps below 1 → 1', () => {
    expect(clampDays(0)).toBe(1)
    expect(clampDays(-5)).toBe(1)
  })

  it('clamps above 90 → 90', () => {
    expect(clampDays(180)).toBe(90)
    expect(clampDays(91)).toBe(90)
  })

  it('passes through values in range', () => {
    expect(clampDays(7)).toBe(7)
    expect(clampDays(14)).toBe(14)
    expect(clampDays(30)).toBe(30)
  })

  it('floors fractional days', () => {
    expect(clampDays(7.9)).toBe(7)
  })

  it('coerces NaN to 7 (default)', () => {
    expect(clampDays(NaN)).toBe(7)
  })
})

// ── DAY_PRESETS ──────────────────────────────────────────────────────────────

describe('DAY_PRESETS', () => {
  it('exposes 7, 14, 30, 90 as the documented presets', () => {
    expect(DAY_PRESETS).toEqual([7, 14, 30, 90])
  })
})

// ── bucketsByVariant ────────────────────────────────────────────────────────

describe('bucketsByVariant', () => {
  it('groups buckets per variant_key in stable day order', () => {
    const grouped = bucketsByVariant(BUCKETS)
    expect(grouped.size).toBe(2)
    const ctrl = grouped.get('control')
    expect(ctrl).toHaveLength(3)
    expect(ctrl?.map((b) => b.day)).toEqual([
      '2026-05-15',
      '2026-05-16',
      '2026-05-17',
    ])
    expect(ctrl?.map((b) => b.value)).toEqual([0.10, 0.11, 0.105])
  })

  it('handles empty buckets', () => {
    expect(bucketsByVariant([]).size).toBe(0)
  })

  it('respects insertion order of the day strings (already ASC per gateway)', () => {
    // The gateway returns buckets ordered by (day asc, variant_key asc); we
    // rely on that here. Days within each variant preserve input order.
    const out = bucketsByVariant(BUCKETS)
    expect(Array.from(out.keys()).sort()).toEqual(['control', 'variant-a'])
  })
})

// ── computeFetchSignature ────────────────────────────────────────────────────

describe('computeFetchSignature', () => {
  it('returns a deterministic signature for the same inputs', () => {
    expect(computeFetchSignature('env-1', 'exp-1', 'm-1', 'user', 7)).toBe(
      computeFetchSignature('env-1', 'exp-1', 'm-1', 'user', 7),
    )
  })

  it('changes when context_type changes', () => {
    const a = computeFetchSignature('env-1', 'exp-1', 'm-1', 'user', 7)
    const b = computeFetchSignature('env-1', 'exp-1', 'm-1', 'account', 7)
    expect(a).not.toBe(b)
  })

  it('changes when days changes', () => {
    const a = computeFetchSignature('env-1', 'exp-1', 'm-1', 'user', 7)
    const b = computeFetchSignature('env-1', 'exp-1', 'm-1', 'user', 14)
    expect(a).not.toBe(b)
  })

  it('changes when metric_id changes', () => {
    const a = computeFetchSignature('env-1', 'exp-1', 'm-1', 'user', 7)
    const b = computeFetchSignature('env-1', 'exp-1', 'm-2', 'user', 7)
    expect(a).not.toBe(b)
  })
})

// ── Timeseries (component) ───────────────────────────────────────────────────

describe('Timeseries (component)', () => {
  it('renders one row per metric_id', () => {
    const html = renderToString(
      <Timeseries
        activeContextType="user"
        metrics={[
          { id: 'metric-1', name: 'conversion' },
          { id: 'metric-2', name: 'revenue' },
        ]}
        seriesByMetric={{ 'metric-1': TS_USER }}
        days={7}
        onDaysChange={() => {}}
        selectedMetricId={null}
        onMetricSelect={() => {}}
        loadingMetricId={null}
      />,
    )
    expect(html).toMatch(/conversion/)
    expect(html).toMatch(/revenue/)
  })

  it('renders the day-preset selector with all four options', () => {
    const html = renderToString(
      <Timeseries
        activeContextType="user"
        metrics={[{ id: 'metric-1', name: 'conversion' }]}
        seriesByMetric={{}}
        days={7}
        onDaysChange={() => {}}
        selectedMetricId={null}
        onMetricSelect={() => {}}
        loadingMetricId={null}
      />,
    )
    expect(html).toMatch(/data-day-preset="7"/)
    expect(html).toMatch(/data-day-preset="14"/)
    expect(html).toMatch(/data-day-preset="30"/)
    expect(html).toMatch(/data-day-preset="90"/)
  })

  it('marks the active day preset with aria-pressed="true"', () => {
    const html = renderToString(
      <Timeseries
        activeContextType="user"
        metrics={[{ id: 'metric-1', name: 'conversion' }]}
        seriesByMetric={{}}
        days={14}
        onDaysChange={() => {}}
        selectedMetricId={null}
        onMetricSelect={() => {}}
        loadingMetricId={null}
      />,
    )
    // aria-pressed=true on the 14d button. React SSR inserts a `<!-- -->`
    // comment between adjacent text nodes (the literal "14" and "d"), so
    // pin via the data-day-preset attribute on the same element.
    expect(html).toMatch(/aria-pressed="true"[^>]*data-day-preset="14"/)
  })

  it('renders an expanded chart hint when a metric is selected', () => {
    const html = renderToString(
      <Timeseries
        activeContextType="user"
        metrics={[{ id: 'metric-1', name: 'conversion' }]}
        seriesByMetric={{ 'metric-1': TS_USER }}
        days={7}
        onDaysChange={() => {}}
        selectedMetricId="metric-1"
        onMetricSelect={() => {}}
        loadingMetricId={null}
      />,
    )
    expect(html).toMatch(/data-testid="expanded-chart"/)
  })

  it('does not render expanded chart when no metric selected', () => {
    const html = renderToString(
      <Timeseries
        activeContextType="user"
        metrics={[{ id: 'metric-1', name: 'conversion' }]}
        seriesByMetric={{ 'metric-1': TS_USER }}
        days={7}
        onDaysChange={() => {}}
        selectedMetricId={null}
        onMetricSelect={() => {}}
        loadingMetricId={null}
      />,
    )
    expect(html).not.toMatch(/data-testid="expanded-chart"/)
  })

  it('scopes title to the active context type', () => {
    const html = renderToString(
      <Timeseries
        activeContextType="account"
        metrics={[{ id: 'metric-1', name: 'conversion' }]}
        seriesByMetric={{}}
        days={7}
        onDaysChange={() => {}}
        selectedMetricId={null}
        onMetricSelect={() => {}}
        loadingMetricId={null}
      />,
    )
    expect(html).toMatch(/account/)
  })

  it('renders an empty state when there are no metrics', () => {
    const html = renderToString(
      <Timeseries
        activeContextType="user"
        metrics={[]}
        seriesByMetric={{}}
        days={7}
        onDaysChange={() => {}}
        selectedMetricId={null}
        onMetricSelect={() => {}}
        loadingMetricId={null}
      />,
    )
    expect(html).toMatch(/No metrics/i)
  })

  it('shows loading marker for the metric currently being fetched', () => {
    const html = renderToString(
      <Timeseries
        activeContextType="user"
        metrics={[{ id: 'metric-1', name: 'conversion' }]}
        seriesByMetric={{}}
        days={7}
        onDaysChange={() => {}}
        selectedMetricId={null}
        onMetricSelect={() => {}}
        loadingMetricId="metric-1"
      />,
    )
    expect(html).toMatch(/Loading|loading/)
  })
})
