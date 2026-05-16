import { useState, useEffect, useCallback } from 'react'
import {
  ComposedChart,
  Bar,
  Line,
  XAxis,
  YAxis,
  CartesianGrid,
  Tooltip,
  Legend,
  ResponsiveContainer,
} from 'recharts'
import { api } from '../../lib/api'

// ── Types ─────────────────────────────────────────────────────────────────────

interface EvalStatsBucket {
  ts: string
  total: number
  by_variant: Record<string, number>
  disabled_count: number
  unique_context_keys: number
}

interface EvalStatsResponse {
  granularity: string
  buckets: EvalStatsBucket[]
}

// ── Range presets ─────────────────────────────────────────────────────────────

type RangeKey = '1h' | '6h' | '24h' | '7d' | '30d'

const RANGES: { key: RangeKey; label: string; hours: number }[] = [
  { key: '1h',  label: 'Last 1h',  hours: 1 },
  { key: '6h',  label: 'Last 6h',  hours: 6 },
  { key: '24h', label: 'Last 24h', hours: 24 },
  { key: '7d',  label: 'Last 7d',  hours: 168 },
  { key: '30d', label: 'Last 30d', hours: 720 },
]

function resolveGranularity(hours: number): string {
  return hours > 24 ? 'day' : 'hour'
}

// ── Chart colours ─────────────────────────────────────────────────────────────

const PALETTE = [
  'var(--accent)',
  '#22c55e',
  '#f59e0b',
  '#8b5cf6',
  '#06b6d4',
  '#f43f5e',
]

// ── Helpers ───────────────────────────────────────────────────────────────────

function formatBucketTs(ts: string, granularity: string): string {
  const d = new Date(ts)
  if (granularity === 'day') {
    return d.toLocaleDateString(undefined, { month: 'short', day: 'numeric' })
  }
  return d.toLocaleTimeString(undefined, { hour: '2-digit', minute: '2-digit' })
}

// ── Main component ────────────────────────────────────────────────────────────

interface AnalyticsTabProps {
  projectId: string
  flagId: string
}

export function AnalyticsTab({ projectId, flagId }: AnalyticsTabProps) {
  const [range, setRange] = useState<RangeKey>('24h')
  const [stats, setStats] = useState<EvalStatsResponse | null>(null)
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState<string | null>(null)

  const selectedRange = RANGES.find((r) => r.key === range)!
  const granularity = resolveGranularity(selectedRange.hours)

  const fetchStats = useCallback(async () => {
    if (!projectId || !flagId) return
    const now = new Date()
    const from = new Date(now.getTime() - selectedRange.hours * 60 * 60 * 1000)
    const params = new URLSearchParams({
      from: from.toISOString(),
      to: now.toISOString(),
      granularity,
    })
    try {
      const { data } = await api.get<EvalStatsResponse>(
        `/v1/projects/${projectId}/flags/${flagId}/eval-stats?${params}`
      )
      setStats(data)
      setError(null)
    } catch (err: unknown) {
      const msg =
        (err as { response?: { data?: { error?: string } } })?.response?.data?.error ??
        (err as { message?: string })?.message ??
        'Failed to load analytics'
      setError(msg)
    } finally {
      setLoading(false)
    }
  }, [projectId, flagId, selectedRange.hours, granularity])

  // Initial load + polling when range ≤ 24 h.
  useEffect(() => {
    setLoading(true)
    setStats(null)
    fetchStats()

    if (selectedRange.hours <= 24) {
      const id = setInterval(fetchStats, 60_000)
      return () => clearInterval(id)
    }
    return undefined
  }, [fetchStats, selectedRange.hours])

  // Derive variant keys from all buckets.
  const variantKeys: string[] = stats
    ? Array.from(
        new Set(stats.buckets.flatMap((b) => Object.keys(b.by_variant)))
      )
    : []

  // Build recharts-compatible data array.
  const chartData = (stats?.buckets ?? []).map((b) => {
    const row: Record<string, number | string> = {
      ts: formatBucketTs(b.ts, stats?.granularity ?? granularity),
      disabled: b.disabled_count,
      unique_ctx: b.unique_context_keys,
    }
    for (const vk of variantKeys) {
      row[vk] = b.by_variant[vk] ?? 0
    }
    return row
  })

  const appliedGranularity = stats?.granularity ?? granularity
  const granularityLabel = appliedGranularity === 'day' ? 'Daily' : 'Hourly'

  return (
    <div className="card">
      <div className="card-header" style={{ display: 'flex', alignItems: 'center', gap: 8 }}>
        <div className="card-title" style={{ flex: 1 }}>
          Evaluation Analytics
          <span
            style={{
              marginLeft: 8,
              fontSize: 11,
              color: 'var(--fg-muted)',
              fontWeight: 400,
            }}
          >
            {granularityLabel}
          </span>
        </div>
        <div style={{ display: 'flex', gap: 4 }}>
          {RANGES.map((r) => (
            <button
              key={r.key}
              className={`btn sm${range === r.key ? ' primary' : ''}`}
              onClick={() => setRange(r.key)}
              style={range === r.key ? { borderColor: 'var(--accent)', color: 'var(--accent)' } : {}}
            >
              {r.label}
            </button>
          ))}
        </div>
      </div>

      <div style={{ padding: '12px 18px 18px' }}>
        {loading && (
          <div style={{ textAlign: 'center', padding: 40, color: 'var(--fg-muted)', fontSize: 13 }}>
            Loading…
          </div>
        )}

        {!loading && error && (
          <div
            style={{
              textAlign: 'center',
              padding: 40,
              color: 'var(--fg-muted)',
              fontSize: 13,
            }}
          >
            {error}
          </div>
        )}

        {!loading && !error && stats && chartData.length === 0 && (
          <div
            style={{
              textAlign: 'center',
              padding: 40,
              color: 'var(--fg-muted)',
              fontSize: 13,
            }}
            data-testid="empty-state"
          >
            No evaluations recorded in this time range.
          </div>
        )}

        {!loading && !error && chartData.length > 0 && (
          <ResponsiveContainer width="100%" height={280}>
            <ComposedChart data={chartData} margin={{ top: 4, right: 32, left: 0, bottom: 0 }}>
              <CartesianGrid strokeDasharray="3 3" stroke="var(--border-faint)" />
              <XAxis dataKey="ts" tick={{ fontSize: 11, fill: 'var(--fg-muted)' }} />
              <YAxis yAxisId="left" tick={{ fontSize: 11, fill: 'var(--fg-muted)' }} />
              <YAxis
                yAxisId="right"
                orientation="right"
                tick={{ fontSize: 11, fill: 'var(--fg-muted)' }}
              />
              <Tooltip
                contentStyle={{
                  background: 'var(--surface)',
                  border: '1px solid var(--border)',
                  fontSize: 12,
                }}
              />
              <Legend wrapperStyle={{ fontSize: 12 }} />

              {/* Stacked bars per variant key */}
              {variantKeys.map((vk, i) => (
                <Bar
                  key={vk}
                  yAxisId="left"
                  dataKey={vk}
                  name={vk}
                  stackId="variants"
                  fill={PALETTE[i % PALETTE.length]}
                />
              ))}

              {/* Disabled evaluations — shaded on top */}
              <Bar
                yAxisId="left"
                dataKey="disabled"
                name="disabled"
                stackId="variants"
                fill="var(--fg-subtle)"
                opacity={0.4}
              />

              {/* Unique context keys on secondary Y-axis */}
              <Line
                yAxisId="right"
                type="monotone"
                dataKey="unique_ctx"
                name="unique contexts"
                stroke="var(--accent)"
                dot={false}
                strokeDasharray="4 2"
              />
            </ComposedChart>
          </ResponsiveContainer>
        )}
      </div>
    </div>
  )
}
