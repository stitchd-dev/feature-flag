import { useNavigate } from 'react-router-dom'
import { PageHeader, VariantBar, Sparkline } from '../components/primitives'
import { I } from '../components/icons'
import { FLAGS, EXPERIMENTS } from '../lib/mockData'

const SERVICES = [
  ['gateway', '8080', 'healthy'],
  ['flag-service', '50051', 'healthy'],
  ['segmentation-service', '50052', 'healthy'],
  ['analytics-service', '50053', 'healthy'],
  ['experimentation-service', '50054', 'healthy'],
  ['auth-service', '50055', 'healthy'],
]

export function Dashboard() {
  const navigate = useNavigate()

  return (
    <>
      <PageHeader
        crumbs={['Acme Cloud', 'stitchd-app', 'production']}
        title="Dashboard"
        subtitle="Snapshot of flag evaluations, experiments, and event volume across the production environment."
        actions={
          <>
            <button className="btn"><I.refresh size={13} /> Refresh</button>
            <button className="btn"><I.download size={13} /> Export</button>
            <button className="btn primary" onClick={() => navigate('/flags')}><I.plus size={14} /> New flag</button>
          </>
        }
      />
      <div className="page-body">
        <div className="stat-grid" style={{ marginBottom: 18 }}>
          <div className="stat">
            <div className="stat-label">Flag evaluations / 24h</div>
            <div className="stat-value">19.4M</div>
            <div className="stat-delta up">▲ 12.4% vs prior 24h</div>
            <Sparkline data={[12, 14, 15, 14, 16, 18, 17, 19, 20, 18, 21, 19]} />
          </div>
          <div className="stat">
            <div className="stat-label">Active flags</div>
            <div className="stat-value">142</div>
            <div className="stat-delta">3 archived this week</div>
            <Sparkline data={[120, 124, 128, 131, 134, 136, 138, 139, 140, 141, 142, 142]} color="var(--info)" />
          </div>
          <div className="stat">
            <div className="stat-label">Running experiments</div>
            <div className="stat-value">6</div>
            <div className="stat-delta up">2 ready to ship</div>
            <Sparkline data={[3, 3, 4, 4, 5, 5, 5, 6, 6, 6, 6, 6]} color="var(--success)" />
          </div>
          <div className="stat">
            <div className="stat-label">Eval p95 latency</div>
            <div className="stat-value">8ms</div>
            <div className="stat-delta">SDK in-process</div>
            <Sparkline data={[9, 8, 9, 8, 7, 8, 9, 8, 7, 8, 8, 8]} color="var(--warning)" />
          </div>
        </div>

        <div className="split-2-third">
          <div className="card">
            <div className="card-header">
              <div className="card-title"><I.flag size={14} /> Recently changed flags</div>
              <button className="btn sm" onClick={() => navigate('/flags')}>See all <I.arrowRight size={12} /></button>
            </div>
            <table className="table">
              <thead>
                <tr><th>Key</th><th>Type</th><th>Rollout</th><th>Updated</th><th></th></tr>
              </thead>
              <tbody>
                {FLAGS.slice(0, 5).map((f) => (
                  <tr key={f.key} className="row-clickable" onClick={() => navigate(`/flags/${f.key}`)}>
                    <td>
                      <div style={{ display: 'flex', alignItems: 'center', gap: 8 }}>
                        <span className={`toggle ${f.state === 'on' ? 'on' : ''}`} onClick={(e) => e.stopPropagation()} />
                        <span className="mono-key">{f.key}</span>
                      </div>
                    </td>
                    <td><span className={`type-pill ${f.type}`}>{f.type}</span></td>
                    <td style={{ minWidth: 160 }}><VariantBar variants={f.variants} /></td>
                    <td style={{ color: 'var(--fg-muted)' }}>{f.updated}</td>
                    <td><I.chevronRight size={14} stroke="var(--fg-subtle)" /></td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>

          <div className="stack">
            <div className="card">
              <div className="card-header">
                <div className="card-title"><I.beaker size={14} /> Experiments</div>
                <button className="btn sm" onClick={() => navigate('/experiments')}>See all</button>
              </div>
              <div className="card-body" style={{ padding: 0 }}>
                {EXPERIMENTS.slice(0, 4).map((e) => (
                  <div key={e.key} style={{ padding: '12px 18px', borderBottom: '1px solid var(--border-faint)', cursor: 'pointer' }} onClick={() => navigate(`/experiments/${e.key}`)}>
                    <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'center', marginBottom: 4 }}>
                      <div style={{ fontWeight: 600, fontSize: 13 }}>{e.name}</div>
                      <span className={`badge ${e.state === 'running' ? 'info' : e.state === 'completed' ? 'success' : ''}`}>{e.state}</span>
                    </div>
                    <div style={{ fontSize: 11, color: 'var(--fg-muted)', display: 'flex', gap: 12 }}>
                      <span>{e.model}</span>
                      <span>·</span>
                      <span style={{ color: e.lift.startsWith('+') ? 'var(--success)' : e.lift.startsWith('−') ? 'var(--danger)' : 'var(--fg-muted)', fontFamily: 'var(--font-mono)' }}>{e.lift}</span>
                      <span>·</span>
                      <span>{e.confidence}% conf</span>
                    </div>
                  </div>
                ))}
              </div>
            </div>

            <div className="card">
              <div className="card-header"><div className="card-title"><I.zap size={14} /> Service health</div></div>
              <div className="card-body" style={{ padding: 0 }}>
                {SERVICES.map(([svc, port, status]) => (
                  <div key={svc} style={{ display: 'flex', alignItems: 'center', padding: '9px 18px', borderBottom: '1px solid var(--border-faint)', fontSize: 12 }}>
                    <span className="env-dot" style={{ width: 7, height: 7 }} />
                    <span style={{ marginLeft: 10, fontFamily: 'var(--font-mono)', flex: 1 }}>stitchd-{svc}</span>
                    <span style={{ fontFamily: 'var(--font-mono)', color: 'var(--fg-muted)', marginRight: 12 }}>:{port}</span>
                    <span className="badge success">{status}</span>
                  </div>
                ))}
              </div>
            </div>
          </div>
        </div>
      </div>
    </>
  )
}
