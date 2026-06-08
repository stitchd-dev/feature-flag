import { useEffect, useState } from 'react'
import { BarChart, Bar, XAxis, YAxis, Tooltip, ResponsiveContainer, Cell } from 'recharts'
import type { LifecycleEntityKind, DependencyGraph as Graph, DependencyEdge } from '../../lib/types'
import { getDependencies } from '../../lib/api'
import { extractErrorMessage } from '../../lib/errors'
import { edgeKindLabel, edgeLabel } from './dependencyHelpers'

interface Props {
  projectId: string
  entityKind: LifecycleEntityKind
  entityId: string
}

/**
 * Dependency-graph visualization (flag_lifecycle_20260604, Phase 8.4).
 *
 * Renders the upstream prerequisites + downstream dependents from the
 * `/dependencies` endpoint as a clear typed two-column tree, plus a small
 * recharts overview bar of the upstream/downstream counts.
 */
export function DependencyGraph({ projectId, entityKind, entityId }: Props) {
  const [graph, setGraph] = useState<Graph | null>(null)
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState<string | null>(null)

  useEffect(() => {
    if (!projectId || !entityId) return
    const controller = new AbortController()
    setLoading(true)
    setError(null)
    getDependencies(projectId, entityKind, entityId, controller.signal)
      .then(setGraph)
      .catch((err) => {
        if (!controller.signal.aborted) setError(extractErrorMessage(err))
      })
      .finally(() => {
        if (!controller.signal.aborted) setLoading(false)
      })
    return () => controller.abort()
  }, [projectId, entityKind, entityId])

  if (loading) {
    return <div style={{ fontSize: 13, color: 'var(--fg-muted)', padding: 12 }}>Loading dependency graph…</div>
  }
  if (error) {
    return <div role="alert" style={{ fontSize: 13, color: 'var(--danger)', padding: 12 }}>{error}</div>
  }
  if (!graph) return null

  const chartData = [
    { name: 'Upstream', value: graph.upstream.length, fill: 'var(--accent, #3b82f6)' },
    { name: 'Downstream', value: graph.downstream.length, fill: 'var(--success, #22c55e)' },
  ]
  const empty = graph.upstream.length === 0 && graph.downstream.length === 0

  return (
    <div style={{ display: 'flex', flexDirection: 'column', gap: 14 }}>
      {!empty && (
        <div style={{ height: 120 }}>
          <ResponsiveContainer width="100%" height="100%">
            <BarChart data={chartData} layout="vertical" margin={{ left: 8, right: 16, top: 4, bottom: 4 }}>
              <XAxis type="number" allowDecimals={false} tick={{ fontSize: 11, fill: 'var(--fg-muted)' }} />
              <YAxis
                type="category"
                dataKey="name"
                width={80}
                tick={{ fontSize: 11, fill: 'var(--fg-muted)' }}
              />
              <Tooltip />
              <Bar dataKey="value" radius={[0, 4, 4, 0]}>
                {chartData.map((d) => (
                  <Cell key={d.name} fill={d.fill} />
                ))}
              </Bar>
            </BarChart>
          </ResponsiveContainer>
        </div>
      )}

      <div
        style={{
          display: 'grid',
          gridTemplateColumns: '1fr 1fr',
          gap: 14,
        }}
      >
        <EdgeColumn
          title="Upstream — depends on"
          emptyText="This entity has no prerequisites or segment references."
          accent="var(--accent, #3b82f6)"
          edges={graph.upstream}
        />
        <EdgeColumn
          title="Downstream — depended on by"
          emptyText="Nothing references this entity."
          accent="var(--success, #22c55e)"
          edges={graph.downstream}
        />
      </div>

      {graph.note && (
        <div style={{ fontSize: 11, color: 'var(--fg-muted)', fontStyle: 'italic' }}>
          Note: {graph.note}
        </div>
      )}
    </div>
  )
}

function EdgeColumn({
  title,
  emptyText,
  accent,
  edges,
}: {
  title: string
  emptyText: string
  accent: string
  edges: DependencyEdge[]
}) {
  return (
    <div>
      <div style={{ fontSize: 12, fontWeight: 600, marginBottom: 8 }}>{title}</div>
      {edges.length === 0 ? (
        <div style={{ fontSize: 12, color: 'var(--fg-muted)' }}>{emptyText}</div>
      ) : (
        <div style={{ display: 'flex', flexDirection: 'column', gap: 6 }}>
          {edges.map((e, i) => (
            <div
              key={`${e.entity_kind}-${e.id}-${i}`}
              style={{
                display: 'flex',
                alignItems: 'center',
                gap: 8,
                padding: '6px 10px',
                border: '1px solid var(--border)',
                borderLeft: `3px solid ${accent}`,
                borderRadius: 6,
              }}
            >
              <span
                style={{
                  fontSize: 10,
                  textTransform: 'uppercase',
                  letterSpacing: '0.04em',
                  color: 'var(--fg-muted)',
                  fontWeight: 700,
                }}
              >
                {e.entity_kind}
              </span>
              <code style={{ fontSize: 12, fontWeight: 600 }}>{edgeLabel(e)}</code>
              <span style={{ flex: 1 }} />
              <span style={{ fontSize: 11, color: 'var(--fg-muted)' }}>{edgeKindLabel(e.kind)}</span>
            </div>
          ))}
        </div>
      )}
    </div>
  )
}
