import { useState } from 'react'
import { useOrgContext } from '../../context/OrgContext'
import { useContextTypeSuggestions, useContextParamSuggestions } from '../../hooks/useContextSuggestions'
import { I } from '../../components/icons'

// ─── ParamRow ─────────────────────────────────────────────────────────────────

function ParamRow({ param }: { param: { param_key: string; inferred_type: string; is_private: boolean; last_seen_at: string } }) {
  return (
    <div style={{
      display: 'flex', alignItems: 'center', gap: 10,
      padding: '6px 12px 6px 32px',
      borderTop: '1px solid var(--border-faint)',
      fontSize: 12,
    }}>
      <span style={{ flex: 1, fontFamily: 'var(--font-mono)', color: 'var(--fg)' }}>
        {param.param_key}
      </span>
      <span style={{
        padding: '1px 7px', borderRadius: 10, fontSize: 11,
        background: 'var(--surface-raised, var(--surface))',
        border: '1px solid var(--border)',
        color: 'var(--fg-muted)',
        fontFamily: 'var(--font-mono)',
      }}>
        {param.inferred_type}
      </span>
      {param.is_private && (
        <span title="Private parameter" style={{ fontSize: 13 }}>🔒</span>
      )}
      <span style={{ fontSize: 11, color: 'var(--fg-subtle)', whiteSpace: 'nowrap' }}>
        {new Date(param.last_seen_at).toLocaleDateString()}
      </span>
    </div>
  )
}

// ─── TypeRow ──────────────────────────────────────────────────────────────────

function TypeRow({ envId, contextType, lastSeenAt }: { envId: string; contextType: string; lastSeenAt: string }) {
  const [expanded, setExpanded] = useState(false)
  const { data: params, loading } = useContextParamSuggestions(expanded ? envId : null, contextType)

  return (
    <div style={{ borderBottom: '1px solid var(--border-faint)' }}>
      <button
        onClick={() => setExpanded((v) => !v)}
        style={{
          width: '100%', background: 'none', border: 'none', cursor: 'pointer',
          display: 'flex', alignItems: 'center', gap: 10,
          padding: '10px 16px', textAlign: 'left',
        }}
      >
        <span style={{ color: 'var(--fg-subtle)', fontSize: 12, width: 12 }}>
          {expanded ? '▾' : '▸'}
        </span>
        <span style={{ flex: 1, fontFamily: 'var(--font-mono)', fontWeight: 600, fontSize: 13 }}>
          {contextType}
        </span>
        <span style={{ fontSize: 11, color: 'var(--fg-subtle)', whiteSpace: 'nowrap' }}>
          last seen {new Date(lastSeenAt).toLocaleDateString()}
        </span>
      </button>

      {expanded && (
        <div>
          {loading && (
            <div style={{ padding: '6px 32px', fontSize: 12, color: 'var(--fg-muted)', fontStyle: 'italic' }}>
              Loading params…
            </div>
          )}
          {!loading && params.length === 0 && (
            <div style={{ padding: '6px 32px', fontSize: 12, color: 'var(--fg-subtle)', fontStyle: 'italic' }}>
              No params observed in the last 90 days
            </div>
          )}
          {params.map((p) => (
            <ParamRow key={p.param_key} param={p} />
          ))}
        </div>
      )}
    </div>
  )
}

// ─── ContextExplorer ──────────────────────────────────────────────────────────

export function ContextExplorer() {
  const { envId } = useOrgContext()
  const { data: types, loading, error } = useContextTypeSuggestions(envId)

  return (
    <div style={{ maxWidth: 840, margin: '0 auto', padding: '24px 16px' }}>
      <div style={{ marginBottom: 20 }}>
        <h1 style={{ fontSize: 20, fontWeight: 700, margin: 0 }}>Context Explorer</h1>
        <p style={{ fontSize: 13, color: 'var(--fg-muted)', marginTop: 4 }}>
          Context types and parameters observed in the last 90 days for the current environment.
        </p>
      </div>

      {!envId && (
        <div style={{ padding: '24px', textAlign: 'center', color: 'var(--fg-muted)', fontSize: 13 }}>
          Select an environment to view context intelligence data.
        </div>
      )}

      {envId && loading && (
        <div style={{ padding: '24px', textAlign: 'center', color: 'var(--fg-muted)', fontSize: 13 }}>
          Loading context types…
        </div>
      )}

      {envId && error && (
        <div style={{
          padding: '12px 16px', borderRadius: 6,
          background: 'var(--danger-bg, #fee2e2)', color: 'var(--danger)', fontSize: 13,
        }}>
          {error}
        </div>
      )}

      {envId && !loading && !error && types.length === 0 && (
        <div className="card" data-testid="empty-state">
          <div className="empty" style={{ padding: '40px 24px' }}>
            <div className="empty-icon"><I.database size={20} /></div>
            <div className="empty-title">No context data yet</div>
            <div className="empty-desc">
              Context types and parameters will appear here after flag evaluations are recorded.
              <br />Data is retained for 90 days.
            </div>
          </div>
        </div>
      )}

      {envId && !loading && types.length > 0 && (
        <div className="card" style={{ padding: 0, overflow: 'hidden' }}>
          <div style={{
            display: 'flex', alignItems: 'center', gap: 10,
            padding: '12px 16px',
            borderBottom: '1px solid var(--border)',
            fontSize: 12, fontWeight: 600, color: 'var(--fg-muted)',
            textTransform: 'uppercase', letterSpacing: '0.05em',
          }}>
            <span style={{ flex: 1 }}>Context Type</span>
            <span>Last Seen</span>
          </div>
          {types.map((t) => (
            <TypeRow
              key={t.context_type}
              envId={envId}
              contextType={t.context_type}
              lastSeenAt={t.last_seen_at}
            />
          ))}
        </div>
      )}
    </div>
  )
}
