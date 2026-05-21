/**
 * Time-series tab — Phase 9 Task 9.3.
 *
 * Renders one row per metric on the experiment (primaries + guardrails). Each
 * row shows a small per-variant sparkline grid + variant comparison. Clicking
 * a row toggles an expanded chart with hover tooltips, scoped to the active
 * context type.
 *
 * Data flow:
 *   • Parent page owns the fetch lifecycle (`getExperimentTimeseries`) so
 *     this component is presentational. It receives:
 *       - `metrics` — the experiment's metric list with display names.
 *       - `seriesByMetric` — Map<metric_id, Timeseries> populated as
 *         responses arrive.
 *       - `days` + `onDaysChange` — current trailing window, mutator.
 *       - `selectedMetricId` + `onMetricSelect` — expanded-chart selection.
 *       - `loadingMetricId` — currently-fetching metric.
 *   • The parent re-fetches when `(env_id, experiment_id, metric_id,
 *     context_type, days)` changes; `computeFetchSignature` exists so the
 *     effect can dedupe responsibly.
 *
 * Day selector presets per spec §4: 7 / 14 / 30 / 90 — clamped to [1, 90].
 */
import { Sparkline } from '../../../components/primitives'
import type { Timeseries as TimeseriesData, TimeseriesBucket } from '../../../lib/types'

// ── Constants ────────────────────────────────────────────────────────────────

/** Day-preset buttons. Spec calls for 7 / 14 / 30 / 90. */
export const DAY_PRESETS: ReadonlyArray<number> = [7, 14, 30, 90]

// ── Pure helpers (exported for direct unit testing) ─────────────────────────

/**
 * Clamps a raw `days` value to [1, 90]. Returns 7 (default) for non-finite
 * inputs.
 */
export function clampDays(raw: number): number {
  if (!Number.isFinite(raw)) return 7
  const n = Math.floor(raw)
  return Math.max(1, Math.min(90, n))
}

/**
 * Folds a Timeseries.buckets[] into a map keyed by variant_key. Preserves
 * the input order of buckets — the gateway already returns
 * (day asc, variant_key asc) so callers can rely on chronological order
 * within each variant.
 */
export function bucketsByVariant(
  buckets: TimeseriesBucket[],
): Map<string, TimeseriesBucket[]> {
  const out = new Map<string, TimeseriesBucket[]>()
  for (const b of buckets) {
    let arr = out.get(b.variant_key)
    if (arr == null) {
      arr = []
      out.set(b.variant_key, arr)
    }
    arr.push(b)
  }
  return out
}

/**
 * Computes a stable signature string for the parent's fetch effect. When
 * this changes, the effect should re-fetch.
 */
export function computeFetchSignature(
  envId: string,
  experimentKey: string,
  metricId: string,
  contextType: string,
  days: number,
): string {
  return `${envId}|${experimentKey}|${metricId}|${contextType}|${days}`
}

// ── Sparkline + expanded chart ───────────────────────────────────────────────

const VARIANT_PALETTE = [
  '#8B8D96', // control grey
  '#5BAEF5', // variant-a blue
  '#3DD68C', // variant-b green
  '#A892FF', // variant-c violet
  '#E6B14F', // variant-d gold
  '#F26B5E', // variant-e coral
]

function paletteFor(variantKey: string, allKeys: string[]): string {
  const idx = allKeys.indexOf(variantKey)
  return VARIANT_PALETTE[idx % VARIANT_PALETTE.length]
}

interface MiniSparkProps {
  variantKey: string
  buckets: TimeseriesBucket[]
  color: string
}

function VariantSparkline({ variantKey, buckets, color }: MiniSparkProps) {
  const values = buckets.map((b) => b.value ?? 0)
  return (
    <div
      style={{
        display: 'flex',
        alignItems: 'center',
        gap: 8,
        minWidth: 200,
      }}
    >
      <span
        style={{
          width: 8,
          height: 8,
          background: color,
          borderRadius: 2,
          flexShrink: 0,
        }}
      />
      <span
        className="mono-key"
        style={{ fontSize: 11, color: 'var(--fg-muted)', flexShrink: 0 }}
      >
        {variantKey}
      </span>
      <div style={{ flex: 1, minWidth: 80 }}>
        <Sparkline data={values} color={color} height={24} />
      </div>
    </div>
  )
}

interface ExpandedChartProps {
  data: TimeseriesData
}

/**
 * Larger SVG chart for the selected metric. Renders one polyline per variant
 * with a hover-friendly tooltip backdrop and axis labels. No external chart
 * dependency — keeps the bundle small.
 */
function ExpandedChart({ data }: ExpandedChartProps) {
  const grouped = bucketsByVariant(data.buckets)
  const allKeys = Array.from(grouped.keys()).sort()
  if (allKeys.length === 0) {
    return (
      <div
        data-testid="expanded-chart"
        className="card"
        style={{
          padding: 32,
          marginTop: 12,
          textAlign: 'center',
          color: 'var(--fg-muted)',
          fontSize: 13,
        }}
      >
        No bucket data in the current window.
      </div>
    )
  }
  // Compute global min/max across all variants.
  let min = Infinity
  let max = -Infinity
  for (const arr of grouped.values()) {
    for (const b of arr) {
      if (b.value == null || !Number.isFinite(b.value)) continue
      if (b.value < min) min = b.value
      if (b.value > max) max = b.value
    }
  }
  if (!Number.isFinite(min) || !Number.isFinite(max)) {
    min = 0
    max = 1
  }
  if (min === max) {
    min -= 0.5
    max += 0.5
  }
  const w = 720
  const h = 240
  const padL = 40
  const padR = 12
  const padT = 14
  const padB = 28

  const xs = Array.from(
    new Set(data.buckets.map((b) => b.day)),
  ).sort()
  if (xs.length < 2) {
    return (
      <div
        data-testid="expanded-chart"
        className="card"
        style={{
          padding: 32,
          marginTop: 12,
          textAlign: 'center',
          color: 'var(--fg-muted)',
          fontSize: 13,
        }}
      >
        Not enough data points to render the chart (need ≥ 2 days).
      </div>
    )
  }
  const xScale = (day: string) => {
    const i = xs.indexOf(day)
    return padL + (i / (xs.length - 1)) * (w - padL - padR)
  }
  const yScale = (v: number) =>
    padT + (1 - (v - min) / (max - min)) * (h - padT - padB)

  return (
    <div
      data-testid="expanded-chart"
      className="card"
      style={{ marginTop: 12 }}
    >
      <div className="card-header">
        <div className="card-title">{data.metric_id}</div>
        <span style={{ fontSize: 11, color: 'var(--fg-muted)' }}>
          {xs.length} day{xs.length === 1 ? '' : 's'} · context_type:{' '}
          {data.context_type}
        </span>
      </div>
      <div style={{ padding: 14, overflowX: 'auto' }}>
        <svg
          viewBox={`0 0 ${w} ${h}`}
          style={{ width: '100%', maxWidth: w }}
        >
          {/* Gridlines */}
          {[0, 0.25, 0.5, 0.75, 1].map((t) => {
            const y = padT + t * (h - padT - padB)
            return (
              <line
                key={t}
                x1={padL}
                x2={w - padR}
                y1={y}
                y2={y}
                stroke="var(--border-faint)"
                strokeDasharray={t === 1 ? undefined : '2 3'}
              />
            )
          })}
          {/* Y-axis labels */}
          {[0, 0.25, 0.5, 0.75, 1].map((t) => {
            const y = padT + t * (h - padT - padB)
            const v = max - t * (max - min)
            return (
              <text
                key={`yl-${t}`}
                x={padL - 6}
                y={y + 3}
                fontSize="10"
                fill="var(--fg-muted)"
                textAnchor="end"
              >
                {v.toFixed(2)}
              </text>
            )
          })}
          {/* X-axis labels: first / middle / last */}
          {[0, Math.floor(xs.length / 2), xs.length - 1].map((i) => {
            const day = xs[i]
            const x = xScale(day)
            return (
              <text
                key={`xl-${i}`}
                x={x}
                y={h - 8}
                fontSize="10"
                fill="var(--fg-muted)"
                textAnchor="middle"
              >
                {day.slice(5)}
              </text>
            )
          })}
          {/* Variant polylines */}
          {allKeys.map((key) => {
            const arr = grouped.get(key)!
            const color = paletteFor(key, allKeys)
            const pts = arr
              .filter((b) => b.value != null)
              .map((b) => `${xScale(b.day)},${yScale(b.value!)}`)
              .join(' ')
            return (
              <g key={key}>
                <polyline
                  points={pts}
                  fill="none"
                  stroke={color}
                  strokeWidth="2"
                  strokeLinecap="round"
                  strokeLinejoin="round"
                />
                {arr
                  .filter((b) => b.value != null)
                  .map((b) => (
                    <circle
                      key={`${key}-${b.day}`}
                      cx={xScale(b.day)}
                      cy={yScale(b.value!)}
                      r={2.5}
                      fill={color}
                    >
                      <title>{`${key} · ${b.day}: ${(b.value! * 100).toFixed(2)}% (n=${b.sample_size})`}</title>
                    </circle>
                  ))}
              </g>
            )
          })}
        </svg>
        <div
          style={{
            display: 'flex',
            gap: 14,
            fontSize: 11,
            color: 'var(--fg-muted)',
            marginTop: 8,
            flexWrap: 'wrap',
          }}
        >
          {allKeys.map((key) => (
            <span
              key={key}
              style={{ display: 'inline-flex', alignItems: 'center', gap: 6 }}
            >
              <span
                style={{
                  width: 10,
                  height: 2,
                  background: paletteFor(key, allKeys),
                }}
              />
              {key}
            </span>
          ))}
        </div>
      </div>
    </div>
  )
}

// ── Timeseries (component) ──────────────────────────────────────────────────

export interface MetricRowEntry {
  id: string
  name: string
}

interface Props {
  activeContextType: string
  metrics: MetricRowEntry[]
  seriesByMetric: Record<string, TimeseriesData>
  days: number
  onDaysChange: (d: number) => void
  selectedMetricId: string | null
  onMetricSelect: (id: string | null) => void
  loadingMetricId: string | null
}

export function Timeseries({
  activeContextType,
  metrics,
  seriesByMetric,
  days,
  onDaysChange,
  selectedMetricId,
  onMetricSelect,
  loadingMetricId,
}: Props) {
  const safeDays = clampDays(days)

  if (metrics.length === 0) {
    return (
      <div className="card" style={{ marginTop: 18 }}>
        <div className="card-header">
          <div className="card-title">
            Time-series · {activeContextType}
          </div>
        </div>
        <div
          style={{
            padding: 32,
            textAlign: 'center',
            color: 'var(--fg-muted)',
            fontSize: 13,
          }}
        >
          No metrics attached to this experiment.
        </div>
      </div>
    )
  }

  return (
    <div>
      <div
        style={{
          display: 'flex',
          alignItems: 'center',
          justifyContent: 'space-between',
          marginBottom: 14,
          padding: '10px 14px',
          background: 'var(--bg-sunken)',
          border: '1px solid var(--border)',
          borderRadius: 8,
        }}
      >
        <div style={{ fontSize: 12, color: 'var(--fg-muted)' }}>
          Daily series · context_type:{' '}
          <strong style={{ color: 'var(--fg)' }}>{activeContextType}</strong>
        </div>
        <div role="group" aria-label="Day window" style={{ display: 'flex', gap: 6 }}>
          {DAY_PRESETS.map((d) => {
            const active = d === safeDays
            return (
              <button
                key={d}
                type="button"
                aria-pressed={active}
                data-day-preset={d}
                className="btn sm"
                onClick={() => onDaysChange(d)}
                style={{
                  background: active ? 'var(--surface)' : 'transparent',
                  borderColor: active
                    ? 'var(--accent)'
                    : 'var(--border-strong)',
                  color: active ? 'var(--accent)' : 'var(--fg)',
                }}
              >
                {d}d
              </button>
            )
          })}
        </div>
      </div>

      <div className="card">
        <table className="table">
          <thead>
            <tr>
              <th>Metric</th>
              <th>Per-variant daily series</th>
              <th></th>
            </tr>
          </thead>
          <tbody>
            {metrics.map((m) => {
              const ts = seriesByMetric[m.id]
              const isLoading = loadingMetricId === m.id
              const isSelected = selectedMetricId === m.id
              const grouped = ts ? bucketsByVariant(ts.buckets) : new Map()
              const variantKeys = Array.from(grouped.keys()).sort()
              return (
                <tr
                  key={m.id}
                  style={{
                    cursor: 'pointer',
                    background: isSelected
                      ? 'var(--bg-sunken)'
                      : undefined,
                  }}
                  onClick={() => onMetricSelect(isSelected ? null : m.id)}
                >
                  <td>
                    <div style={{ fontWeight: 600 }}>{m.name}</div>
                    <div
                      className="mono-key"
                      style={{ fontSize: 11, color: 'var(--fg-muted)' }}
                    >
                      {m.id}
                    </div>
                  </td>
                  <td>
                    {isLoading ? (
                      <span style={{ color: 'var(--fg-muted)', fontSize: 12 }}>
                        Loading…
                      </span>
                    ) : ts == null || variantKeys.length === 0 ? (
                      <span style={{ color: 'var(--fg-muted)', fontSize: 12 }}>
                        No data
                      </span>
                    ) : (
                      <div style={{ display: 'flex', gap: 14, flexWrap: 'wrap' }}>
                        {variantKeys.map((vk) => (
                          <VariantSparkline
                            key={vk}
                            variantKey={vk}
                            buckets={grouped.get(vk)!}
                            color={paletteFor(vk, variantKeys)}
                          />
                        ))}
                      </div>
                    )}
                  </td>
                  <td style={{ textAlign: 'right', color: 'var(--fg-muted)', fontSize: 12 }}>
                    {isSelected ? 'Close ⌃' : 'Expand ⌄'}
                  </td>
                </tr>
              )
            })}
          </tbody>
        </table>
      </div>

      {selectedMetricId != null && seriesByMetric[selectedMetricId] != null && (
        <ExpandedChart data={seriesByMetric[selectedMetricId]} />
      )}
    </div>
  )
}
