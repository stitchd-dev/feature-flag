import { useState, useEffect } from 'react'
import { useNavigate, useParams } from 'react-router-dom'
import { PageHeader } from '../../components/primitives'
import { I } from '../../components/icons'
import { useOrgContext } from '../../context/OrgContext'
import { api } from '../../lib/api'

interface SegmentResponse {
  segment_id: string
  environment_id: string
  key: string
  name: string
  description: string
  segment_type: string // "rule_based" | "list_based"
  version: number
  created_at: string
  updated_at: string
}

const RULE_RULES = [
  [['context.type', '==', 'user'], ['plan', 'in', '[pro, enterprise]']],
  [['signup_age_days', '>=', '30']],
  [['NOT email', 'ends_with', '@stitchd.dev']],
]

const LIST_ENTRIES = [
  ['u_priya@stitchd.dev', 'yesterday', 'Priya R.'],
  ['u_marco@stitchd.dev', 'yesterday', 'Priya R.'],
  ['u_lin@stitchd.dev', '2d ago', 'Marco G.'],
  ['u_devon@stitchd.dev', '2d ago', 'Marco G.'],
  ['u_sara@stitchd.dev', '1w ago', 'Lin T.'],
]

export function SegmentDetail() {
  const { key } = useParams<{ key: string }>()
  const navigate = useNavigate()
  const { envId, orgId } = useOrgContext()
  const [segment, setSegment] = useState<SegmentResponse | null>(null)
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState<string | null>(null)

  useEffect(() => {
    if (!envId || !key) return
    setLoading(true)
    setError(null)
    api.get<SegmentResponse>(`/v1/environments/${envId}/segments/${key}`)
      .then(({ data }) => setSegment(data))
      .catch((err) => setError(err?.response?.data?.message ?? err.message ?? 'Failed to load segment'))
      .finally(() => setLoading(false))
  }, [envId, key])

  if (!envId) {
    return (
      <div className="page-body">
        <div style={{ padding: '12px 16px', background: 'var(--warning-bg)', border: '1px solid rgba(184,118,26,0.35)', borderRadius: 8, fontSize: 13, marginBottom: 16 }}>
          No environment selected — set an environment ID in environments settings
        </div>
      </div>
    )
  }

  if (loading) {
    return (
      <div style={{ display: 'flex', justifyContent: 'center', padding: 48 }}>
        <span style={{ color: 'var(--fg-muted)', fontSize: 14 }}>Loading segment…</span>
      </div>
    )
  }

  if (error) {
    return (
      <div className="page-body">
        <div style={{ padding: '12px 16px', background: 'var(--danger-bg)', border: '1px solid rgba(196,43,28,0.3)', borderRadius: 8, color: 'var(--danger)', fontSize: 13 }}>
          <I.alert size={14} style={{ verticalAlign: 'middle', marginRight: 6 }} />
          {error}
        </div>
      </div>
    )
  }

  if (!segment) return null

  const isRuleBased = segment.segment_type === 'rule_based'

  return (
    <>
      <PageHeader
        crumbs={[<a key="1" onClick={() => navigate(`/org/${orgId}/segments`)} style={{ cursor: 'pointer' }}>Segments</a>, segment.key]}
        title={segment.key}
        mono
        subtitle={segment.name}
        badge={<span className={`badge ${isRuleBased ? 'info' : ''}`}>{isRuleBased ? 'rule-based' : 'list-based'}</span>}
        actions={
          <>
            <button className="btn"><I.copy size={13} /> Duplicate</button>
            <button className="btn primary"><I.pencil size={13} /> Edit</button>
          </>
        }
      />
      <div className="page-body">
        <div className="stat-grid" style={{ marginBottom: 18 }}>
          <div className="stat">
            <div className="stat-label">Type</div>
            <div className="stat-value" style={{ fontFamily: 'var(--font-mono)', fontSize: 20 }}>{isRuleBased ? 'rule' : 'list'}</div>
          </div>
          <div className="stat">
            <div className="stat-label">Version</div>
            <div className="stat-value">v{segment.version}</div>
          </div>
          <div className="stat">
            <div className="stat-label">Created</div>
            <div className="stat-value" style={{ fontSize: 18 }}>{new Date(segment.created_at).toLocaleDateString()}</div>
          </div>
          <div className="stat">
            <div className="stat-label">Updated</div>
            <div className="stat-value" style={{ fontSize: 18 }}>{new Date(segment.updated_at).toLocaleDateString()}</div>
          </div>
        </div>

        <div className="split-2-third">
          {isRuleBased ? (
            <div className="card">
              <div className="card-header">
                <div className="card-title"><I.toggle size={14} /> Rule definition</div>
                <button className="btn sm"><I.plus size={12} /> Add rule</button>
              </div>
              <div style={{ padding: 18 }}>
                {RULE_RULES.map((rule, i) => (
                  <div key={i} style={{ padding: '12px 0', borderBottom: i < RULE_RULES.length - 1 ? '1px solid var(--border-faint)' : 'none', display: 'flex', alignItems: 'center', gap: 12 }}>
                    <div style={{ width: 24, height: 24, borderRadius: 5, background: 'var(--bg-sunken)', display: 'grid', placeItems: 'center', fontFamily: 'var(--font-mono)', fontSize: 11, fontWeight: 600 }}>{i + 1}</div>
                    <div style={{ display: 'flex', flexWrap: 'wrap', gap: 6, flex: 1 }}>
                      {rule.map(([k, op, v], j) => (
                        <span key={j} style={{ display: 'inline-flex', alignItems: 'center', gap: 6 }}>
                          {j > 0 && <span style={{ fontSize: 10, fontFamily: 'var(--font-mono)', color: 'var(--fg-subtle)', padding: '3px 6px', background: 'var(--bg-sunken)', borderRadius: 3 }}>AND</span>}
                          <span style={{ display: 'inline-flex', gap: 6, padding: '3px 8px', background: 'var(--bg-sunken)', border: '1px solid var(--border)', borderRadius: 5, fontFamily: 'var(--font-mono)', fontSize: 12 }}>
                            <span style={{ color: 'var(--info)' }}>{k}</span>
                            <span style={{ color: 'var(--fg-subtle)' }}>{op}</span>
                            <span style={{ color: 'var(--success)' }}>{v}</span>
                          </span>
                        </span>
                      ))}
                    </div>
                    <button className="icon-btn"><I.more size={14} /></button>
                  </div>
                ))}
                <div style={{ marginTop: 12, padding: 10, background: 'var(--info-bg)', border: '1px solid rgba(31,111,191,0.25)', borderRadius: 6, fontSize: 12, display: 'flex', gap: 8, color: 'var(--info)' }}>
                  <I.info size={13} /> First-true wins. Rules within a row are AND'd; rows are OR'd.
                </div>
              </div>
            </div>
          ) : (
            <div className="card">
              <div className="card-header">
                <div className="card-title"><I.list size={14} /> List entries</div>
                <div style={{ display: 'flex', gap: 6 }}>
                  <button className="btn sm">Include ({LIST_ENTRIES.length})</button>
                  <button className="btn sm" style={{ borderColor: 'transparent', color: 'var(--fg-muted)' }}>Exclude (0)</button>
                </div>
              </div>
              <table className="table">
                <thead><tr><th>Key</th><th>Added</th><th>By</th></tr></thead>
                <tbody>
                  {LIST_ENTRIES.map(([k, added, by], i) => (
                    <tr key={i}>
                      <td><span className="mono-key">{k}</span></td>
                      <td style={{ color: 'var(--fg-muted)' }}>{added}</td>
                      <td style={{ color: 'var(--fg-muted)' }}>{by}</td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          )}

          <div className="stack">
            <div className="card">
              <div className="card-header"><div className="card-title">Details</div></div>
              <div className="card-body">
                <div style={{ fontSize: 12, lineHeight: 1.8 }}>
                  {segment.description && (
                    <div style={{ marginBottom: 10, color: 'var(--fg-muted)' }}>{segment.description}</div>
                  )}
                  {[
                    ['Segment ID', segment.segment_id],
                    ['Environment', segment.environment_id],
                    ['Version', `v${segment.version}`],
                  ].map(([k, v]) => (
                    <div key={k} style={{ display: 'flex', justifyContent: 'space-between' }}>
                      <span style={{ color: 'var(--fg-muted)' }}>{k}</span>
                      <span className="mono-key" style={{ fontSize: 11 }}>{v}</span>
                    </div>
                  ))}
                </div>
              </div>
            </div>
            <div className="card">
              <div className="card-header">
                <div className="card-title">Test context</div>
                <button className="btn sm"><I.play size={12} /> Test</button>
              </div>
              <div className="card-body">
                <pre className="code">{`{
  "_type": "user",
  "key": "u_8421",
  "parameters": {
    "plan": "pro",
    "signup_age_days": 45
  }
}`}</pre>
                <div style={{ marginTop: 10, padding: 8, background: 'var(--success-bg)', borderRadius: 5, fontSize: 12, color: 'var(--success)', fontWeight: 600 }}>
                  <I.check size={13} style={{ verticalAlign: 'middle', marginRight: 4 }} /> matches segment
                </div>
              </div>
            </div>
          </div>
        </div>
      </div>
    </>
  )
}
