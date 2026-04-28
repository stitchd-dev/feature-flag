import { useState } from 'react'
import { useNavigate } from 'react-router-dom'
import { useTweaks } from '../../hooks/useTweaks'
import { PageHeader, VariantBar, Sparkline } from '../../components/primitives'
import { I } from '../../components/icons'
import { FLAGS } from '../../lib/mockData'
import type { Flag } from '../../lib/mockData'

const ALL_SEGMENTS = ['beta-customers', 'high-risk-merchants', 'new-signups-7d', '(no segment)']

function Toggle({ on, onClick }: { on: boolean; onClick: (e: React.MouseEvent) => void }) {
  return <span className={`toggle ${on ? 'on' : ''}`} onClick={onClick} />
}

function FlagTableRow({ flag, onToggle }: { flag: Flag; onToggle: (key: string) => void }) {
  const navigate = useNavigate()
  return (
    <tr className="row-clickable" onClick={() => navigate(`/flags/${flag.key}`)}>
      <td><Toggle on={flag.state === 'on'} onClick={(e) => { e.stopPropagation(); onToggle(flag.key) }} /></td>
      <td>
        <div style={{ display: 'flex', flexDirection: 'column', gap: 2 }}>
          <span className="mono-key">{flag.key}</span>
          <span style={{ fontSize: 11, color: 'var(--fg-muted)' }}>
            {flag.name}
            {flag.staged && <span className="badge warning" style={{ marginLeft: 8 }}>staged</span>}
          </span>
        </div>
      </td>
      <td><span className={`type-pill ${flag.type}`}>{flag.type}</span></td>
      <td style={{ minWidth: 180 }}><VariantBar variants={flag.variants} /></td>
      <td>
        <div style={{ display: 'flex', alignItems: 'center', gap: 8 }}>
          <Sparkline data={flag.sparkline} height={20} />
          <span style={{ fontFamily: 'var(--font-mono)', fontSize: 12, color: 'var(--fg-muted)' }}>{flag.lastEval}</span>
        </div>
      </td>
      <td>
        <div style={{ display: 'flex', gap: 4, flexWrap: 'wrap' }}>
          {flag.segments.slice(0, 2).map((s) => <span key={s} className="badge">{s}</span>)}
          {flag.segments.length > 2 && <span className="badge">+{flag.segments.length - 2}</span>}
          {flag.segments.length === 0 && <span style={{ color: 'var(--fg-faint)' }}>—</span>}
        </div>
      </td>
      <td><span style={{ color: 'var(--fg-muted)' }}>{flag.owner}</span></td>
      <td style={{ color: 'var(--fg-muted)' }}>{flag.updated}</td>
      <td><I.chevronRight size={14} stroke="var(--fg-subtle)" /></td>
    </tr>
  )
}

function FlagCard({ flag, onToggle }: { flag: Flag; onToggle: (key: string) => void }) {
  const navigate = useNavigate()
  return (
    <div className="card" style={{ cursor: 'pointer' }} onClick={() => navigate(`/flags/${flag.key}`)}>
      <div style={{ padding: 14, borderBottom: '1px solid var(--border-faint)' }}>
        <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'flex-start', marginBottom: 8 }}>
          <Toggle on={flag.state === 'on'} onClick={(e) => { e.stopPropagation(); onToggle(flag.key) }} />
          <span className={`type-pill ${flag.type}`}>{flag.type}</span>
        </div>
        <div style={{ fontFamily: 'var(--font-mono)', fontWeight: 600, fontSize: 13, marginBottom: 2 }}>{flag.key}</div>
        <div style={{ fontSize: 12, color: 'var(--fg-muted)' }}>{flag.name}</div>
      </div>
      <div style={{ padding: 14 }}>
        <VariantBar variants={flag.variants} />
        <div style={{ display: 'flex', justifyContent: 'space-between', marginTop: 10, fontSize: 11, color: 'var(--fg-muted)' }}>
          <span>{flag.rules} rules · {flag.segments.length} segments</span>
          <span>{flag.lastEval} evals</span>
        </div>
      </div>
    </div>
  )
}

export function FlagsList() {
  const navigate = useNavigate()
  const { tweaks } = useTweaks()
  const [layout, setLayout] = useState<'table' | 'cards' | 'grouped'>(tweaks.flagsLayout)
  const [search, setSearch] = useState('')
  const [flags, setFlags] = useState(FLAGS)

  const filtered = flags.filter((f) =>
    !search || f.key.includes(search) || f.name.toLowerCase().includes(search.toLowerCase()) || f.owner.includes(search)
  )

  function toggleFlag(key: string) {
    setFlags((prev) => prev.map((f) => f.key === key ? { ...f, state: f.state === 'on' ? 'off' : 'on' } : f))
  }

  const onCount = filtered.filter((f) => f.state === 'on').length
  const offCount = filtered.filter((f) => f.state === 'off').length
  const stagedCount = filtered.filter((f) => f.staged).length

  return (
    <>
      <PageHeader
        crumbs={['stitchd-app', 'Flags']}
        title="Feature Flags"
        subtitle={`${flags.length} flags across 4 environments. First-true rule wins; flag types are immutable after creation.`}
        actions={
          <>
            <button className="btn"><I.filter size={13} /> Filters</button>
            <button className="btn"><I.archive size={13} /> Archived</button>
            <button className="btn primary" onClick={() => navigate('/flags/new')}><I.plus size={14} /> New flag</button>
          </>
        }
      />
      <div className="page-body">
        <div style={{ display: 'flex', gap: 12, alignItems: 'center', marginBottom: 16 }}>
          <div className="search-input" style={{ maxWidth: 320 }}>
            <I.search size={14} />
            <input
              className="input"
              placeholder="Filter by key, owner, segment…"
              value={search}
              onChange={(e) => setSearch(e.target.value)}
            />
          </div>
          <div style={{ display: 'flex', gap: 4, padding: 3, background: 'var(--bg-sunken)', borderRadius: 8, border: '1px solid var(--border)' }}>
            {([['table', I.list], ['cards', I.grid], ['grouped', I.layers]] as const).map(([k, Ic]) => (
              <button
                key={k}
                onClick={() => setLayout(k)}
                className="icon-btn"
                style={{
                  background: layout === k ? 'var(--surface)' : 'transparent',
                  color: layout === k ? 'var(--fg)' : 'var(--fg-muted)',
                  boxShadow: layout === k ? 'var(--shadow-xs)' : 'none',
                }}
              >
                <Ic size={14} />
              </button>
            ))}
          </div>
          <div style={{ marginLeft: 'auto', display: 'flex', gap: 8 }}>
            <span className="badge"><span className="dot" style={{ background: 'var(--success)' }} />{onCount} on</span>
            <span className="badge">{offCount} off</span>
            {stagedCount > 0 && <span className="badge warning">{stagedCount} staged</span>}
          </div>
        </div>

        {layout === 'table' && (
          <div className="card">
            <div className="table-wrap">
              <table className="table">
                <thead>
                  <tr>
                    <th style={{ width: 56 }}></th>
                    <th>Key</th>
                    <th>Type</th>
                    <th>Rollout</th>
                    <th>30d evals</th>
                    <th>Segments</th>
                    <th>Owner</th>
                    <th>Updated</th>
                    <th></th>
                  </tr>
                </thead>
                <tbody>
                  {filtered.map((f) => <FlagTableRow key={f.key} flag={f} onToggle={toggleFlag} />)}
                </tbody>
              </table>
            </div>
          </div>
        )}

        {layout === 'cards' && (
          <div style={{ display: 'grid', gridTemplateColumns: 'repeat(auto-fill, minmax(320px, 1fr))', gap: 12 }}>
            {filtered.map((f) => <FlagCard key={f.key} flag={f} onToggle={toggleFlag} />)}
          </div>
        )}

        {layout === 'grouped' && (
          <div className="stack">
            {ALL_SEGMENTS.map((seg) => {
              const items = filtered.filter((f) =>
                seg === '(no segment)' ? f.segments.length === 0 : f.segments.includes(seg)
              )
              if (!items.length) return null
              return (
                <div key={seg} className="card">
                  <div className="card-header">
                    <div className="card-title">
                      <I.segment size={14} /> <span className="mono-key">{seg}</span> <span className="badge">{items.length}</span>
                    </div>
                  </div>
                  <table className="table">
                    <tbody>
                      {items.map((f) => (
                        <tr key={f.key} className="row-clickable" onClick={() => navigate(`/flags/${f.key}`)}>
                          <td style={{ width: 40 }}>
                            <Toggle on={f.state === 'on'} onClick={(e) => { e.stopPropagation(); toggleFlag(f.key) }} />
                          </td>
                          <td><span className="mono-key">{f.key}</span></td>
                          <td><span className={`type-pill ${f.type}`}>{f.type}</span></td>
                          <td style={{ minWidth: 160 }}><VariantBar variants={f.variants} /></td>
                          <td style={{ color: 'var(--fg-muted)' }}>{f.updated}</td>
                        </tr>
                      ))}
                    </tbody>
                  </table>
                </div>
              )
            })}
          </div>
        )}
      </div>
    </>
  )
}
