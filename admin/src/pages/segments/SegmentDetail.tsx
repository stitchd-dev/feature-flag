import { useNavigate, useParams } from 'react-router-dom'
import { PageHeader } from '../../components/primitives'
import { I } from '../../components/icons'
import { SEGMENTS, FLAGS } from '../../lib/mockData'

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
  const seg = SEGMENTS.find((s) => s.key === key) ?? SEGMENTS[0]
  const usedByFlags = FLAGS.filter((f) => f.segments.includes(seg.key))

  return (
    <>
      <PageHeader
        crumbs={[<a key="1" onClick={() => navigate('/segments')} style={{ cursor: 'pointer' }}>Segments</a>, seg.key]}
        title={seg.key}
        mono
        subtitle={seg.name}
        badge={<span className={`badge ${seg.type === 'rule' ? 'info' : ''}`}>{seg.type === 'rule' ? 'rule-based' : 'list-based'}</span>}
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
            <div className="stat-label">Members</div>
            <div className="stat-value">{seg.members.toLocaleString()}</div>
            <div className="stat-delta up">+24 this week</div>
          </div>
          <div className="stat">
            <div className="stat-label">Context type</div>
            <div className="stat-value" style={{ fontFamily: 'var(--font-mono)', fontSize: 20 }}>{seg.contextType}</div>
          </div>
          <div className="stat">
            <div className="stat-label">Used by</div>
            <div className="stat-value">{seg.usedBy}</div>
            <div className="stat-delta">flags & rules</div>
          </div>
          <div className="stat">
            <div className="stat-label">Last evaluated</div>
            <div className="stat-value" style={{ fontSize: 18 }}>3s ago</div>
            <div className="stat-delta">11K evals/min</div>
          </div>
        </div>

        <div className="split-2-third">
          {seg.type === 'rule' ? (
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
              <div className="card-header"><div className="card-title">Used by</div></div>
              <div className="card-body" style={{ padding: 0 }}>
                {usedByFlags.length === 0 && (
                  <div style={{ padding: '16px 18px', color: 'var(--fg-faint)', fontSize: 13 }}>No flags reference this segment.</div>
                )}
                {usedByFlags.map((f) => (
                  <div key={f.key} style={{ padding: '10px 18px', borderBottom: '1px solid var(--border-faint)', display: 'flex', alignItems: 'center', gap: 8, cursor: 'pointer' }} onClick={() => navigate(`/flags/${f.key}`)}>
                    <span className={`toggle ${f.state === 'on' ? 'on' : ''}`} style={{ transform: 'scale(0.75)' }} />
                    <span className="mono-key">{f.key}</span>
                    <span className={`type-pill ${f.type}`} style={{ marginLeft: 'auto' }}>{f.type}</span>
                  </div>
                ))}
              </div>
            </div>
            <div className="card">
              <div className="card-header">
                <div className="card-title">Test context</div>
                <button className="btn sm"><I.play size={12} /> Test</button>
              </div>
              <div className="card-body">
                <pre className="code">{`{
  "_type": "${seg.contextType}",
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
