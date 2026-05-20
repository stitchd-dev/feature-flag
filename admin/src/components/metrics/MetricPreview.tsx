/**
 * MetricPreview — time-series preview for a saved metric.
 *
 * Calls `POST /v1/metrics/{id}/preview` with `{ days }` and renders a
 * sparkline + numeric summary (last / min / max / mean over the window).
 *
 * Tri-state pattern mirrors `EventDetail.tsx` (worker_13):
 *   - loading            → "Loading preview…"
 *   - error              → "Failed to load preview" + retry
 *   - success + empty[]  → graceful "preview not yet available" notice
 *                          (Phase 4 backend stub returns `buckets: []`)
 *   - success + buckets  → sparkline + summary table
 *
 * The component is intentionally edit-only — CreateMetricModal passes
 * `metricId === undefined` and we render a "save first" prompt instead.
 *
 * @see feature-flag-7an.6.6 — this task
 * @see feature-flag-7an.4.2 — Phase 4 ClickHouse pipeline (empty buckets today)
 */
import { useCallback, useEffect, useState } from 'react'
import { Sparkline } from '../primitives'
import { I } from '../icons'
import { api } from '../../lib/api'
import { extractErrorMessage } from '../../lib/errors'

// ── Types ─────────────────────────────────────────────────────────────────────

interface PreviewBucket {
  day: string
  value: number
}

interface PreviewResponse {
  buckets: PreviewBucket[]
  /** Phase 3/4 — present when the analytics service can't produce real data yet. */
  warning?: string
}

interface BucketSummary {
  count: number
  last: number
  min: number
  max: number
  mean: number
}

// ── Pure helpers (mirrored verbatim in MetricPreview.test.ts) ────────────────

/** Coerce a possibly-NaN/undefined/null bucket value to a finite number. */
function safeValue(v: unknown): number {
  const n = typeof v === 'number' ? v : Number.NaN
  return Number.isFinite(n) ? n : 0
}

/**
 * Reduce a buckets array to its summary. Empty array → all zeros (count 0).
 * NaN / undefined / null bucket values are coerced to 0 so the summary
 * never leaks NaN into the UI.
 */
function summariseBuckets(buckets: PreviewBucket[]): BucketSummary {
  if (!buckets || buckets.length === 0) {
    return { count: 0, last: 0, min: 0, max: 0, mean: 0 }
  }
  const values = buckets.map((b) => safeValue(b.value))
  const sum = values.reduce((a, b) => a + b, 0)
  return {
    count: values.length,
    last: values[values.length - 1],
    min: Math.min(...values),
    max: Math.max(...values),
    mean: sum / values.length,
  }
}

/** Build the preview endpoint path. `days` lives in the POST body. */
function previewUrl(metricId: string): string {
  return `/v1/metrics/${encodeURIComponent(metricId)}/preview`
}

/** Shape the request body for POST /v1/metrics/{id}/preview. */
function previewBodyShape(days: number): { days: number } {
  // Server-side default is 7 days when days <= 0.
  return { days: days > 0 ? days : 7 }
}

// ── Component ────────────────────────────────────────────────────────────────

interface Props {
  /** Metric id — undefined when the parent is CreateMetricModal (unsaved). */
  metricId: string | undefined
  /** Lookback window in days — default 7. */
  days?: number
}

type FetchState =
  | { kind: 'idle' }
  | { kind: 'loading' }
  | { kind: 'success'; data: PreviewResponse }
  | { kind: 'error'; message: string }

export function MetricPreview({ metricId, days = 7 }: Props) {
  const [state, setState] = useState<FetchState>({ kind: 'idle' })

  const load = useCallback(
    (signal?: AbortSignal) => {
      if (!metricId) {
        setState({ kind: 'idle' })
        return
      }
      setState({ kind: 'loading' })
      api
        .post<PreviewResponse>(previewUrl(metricId), previewBodyShape(days), { signal })
        .then(({ data }) => {
          if (signal?.aborted) return
          setState({ kind: 'success', data })
        })
        .catch((err: unknown) => {
          if (signal?.aborted) return
          setState({ kind: 'error', message: extractErrorMessage(err) })
        })
    },
    [metricId, days],
  )

  useEffect(() => {
    const ac = new AbortController()
    load(ac.signal)
    return () => ac.abort()
  }, [load])

  // ── Render branches ─────────────────────────────────────────────────────

  // Unsaved metric — CreateMetricModal callsite.
  if (!metricId) {
    return (
      <PreviewCard>
        <div style={mutedHint}>
          <I.info size={13} style={{ marginTop: 1, flexShrink: 0 }} />
          <span>Save the metric to enable preview.</span>
        </div>
      </PreviewCard>
    )
  }

  if (state.kind === 'idle' || state.kind === 'loading') {
    return (
      <PreviewCard>
        <div style={mutedHint}>
          <span>Loading preview…</span>
        </div>
      </PreviewCard>
    )
  }

  if (state.kind === 'error') {
    return (
      <PreviewCard>
        <div style={{ ...mutedHint, color: 'var(--danger, #b91c1c)' }}>
          <I.alert size={13} style={{ marginTop: 1, flexShrink: 0 }} />
          <span>Failed to load preview: {state.message}</span>
          <button
            type="button"
            className="btn sm"
            style={{ marginLeft: 'auto' }}
            onClick={() => load()}
          >
            Retry
          </button>
        </div>
      </PreviewCard>
    )
  }

  const summary = summariseBuckets(state.data.buckets)
  const series = state.data.buckets.map((b) => (Number.isFinite(b.value) ? b.value : 0))

  if (summary.count === 0) {
    return (
      <PreviewCard>
        <div style={mutedHint}>
          <I.info size={13} style={{ marginTop: 1, flexShrink: 0 }} />
          <span>
            {state.data.warning
              ? state.data.warning
              : 'Preview not yet available — Phase 4 backend implementation pending (feature-flag-7an.4.2).'}
          </span>
        </div>
      </PreviewCard>
    )
  }

  return (
    <PreviewCard>
      <div
        style={{
          display: 'flex',
          alignItems: 'center',
          justifyContent: 'space-between',
          marginBottom: 10,
        }}
      >
        <div style={{ fontSize: 12, fontWeight: 600, color: 'var(--fg)' }}>
          <I.metric size={13} /> Preview ({days}d)
        </div>
        <div style={{ minWidth: 140 }}>
          <Sparkline data={series} height={32} />
        </div>
      </div>
      <table
        style={{
          width: '100%',
          fontSize: 12,
          borderCollapse: 'collapse',
        }}
      >
        <thead>
          <tr style={{ color: 'var(--fg-muted)', textAlign: 'left' }}>
            <th style={th}>Last</th>
            <th style={th}>Min</th>
            <th style={th}>Max</th>
            <th style={th}>Mean</th>
            <th style={th}>Buckets</th>
          </tr>
        </thead>
        <tbody>
          <tr style={{ fontFamily: 'var(--font-mono)' }}>
            <td style={td}>{fmt(summary.last)}</td>
            <td style={td}>{fmt(summary.min)}</td>
            <td style={td}>{fmt(summary.max)}</td>
            <td style={td}>{fmt(summary.mean)}</td>
            <td style={td}>{summary.count}</td>
          </tr>
        </tbody>
      </table>
      {state.data.warning && (
        <div
          style={{
            marginTop: 8,
            fontSize: 11,
            color: 'var(--fg-muted)',
            display: 'flex',
            gap: 6,
            alignItems: 'flex-start',
          }}
        >
          <I.info size={11} style={{ marginTop: 2, flexShrink: 0 }} />
          <span>{state.data.warning}</span>
        </div>
      )}
    </PreviewCard>
  )
}

// ── Display helpers ─────────────────────────────────────────────────────────

function fmt(n: number): string {
  if (!Number.isFinite(n)) return '—'
  // Two decimal places unless integer.
  return Number.isInteger(n) ? String(n) : n.toFixed(2)
}

const mutedHint: React.CSSProperties = {
  display: 'flex',
  gap: 8,
  alignItems: 'flex-start',
  fontSize: 12,
  color: 'var(--fg-muted)',
}

const th: React.CSSProperties = {
  fontWeight: 500,
  padding: '4px 8px 4px 0',
  fontSize: 11,
  textTransform: 'uppercase',
  letterSpacing: 0.3,
}

const td: React.CSSProperties = {
  padding: '4px 8px 0 0',
}

function PreviewCard({ children }: { children: React.ReactNode }) {
  return (
    <div
      style={{
        padding: '12px 14px',
        background: 'var(--bg-sunken)',
        borderRadius: 8,
      }}
    >
      {children}
    </div>
  )
}
