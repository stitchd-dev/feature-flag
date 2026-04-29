import { useState, useEffect } from 'react'
import { useNavigate } from 'react-router-dom'
import { useTweaks } from '../../hooks/useTweaks'
import { PageHeader, VariantBar, Sparkline } from '../../components/primitives'
import { I } from '../../components/icons'
import { useOrgContext } from '../../context/OrgContext'
import { api } from '../../lib/api'

interface FlagResponse {
  flag_id: string
  project_id: string
  key: string
  name: string
  description: string
  flag_type: string // "bool" | "string" | "int" | "double" | "json"
  status: string // "enabled" | "disabled"
  version: number
  created_at: string
  updated_at: string
}

function Toggle({ on, onClick }: { on: boolean; onClick: (e: React.MouseEvent) => void }) {
  return <span className={`toggle ${on ? 'on' : ''}`} onClick={onClick} />
}

function FlagTableRow({ flag, orgId, onToggle }: { flag: FlagResponse; orgId: string; onToggle: (key: string) => void }) {
  const navigate = useNavigate()
  return (
    <tr className="row-clickable" onClick={() => navigate(`/org/${orgId}/flags/${flag.key}`)}>
      <td><Toggle on={flag.status === 'enabled'} onClick={(e) => { e.stopPropagation(); onToggle(flag.key) }} /></td>
      <td>
        <div style={{ display: 'flex', flexDirection: 'column', gap: 2 }}>
          <span className="mono-key">{flag.key}</span>
          <span style={{ fontSize: 11, color: 'var(--fg-muted)' }}>{flag.name}</span>
        </div>
      </td>
      <td><span className={`type-pill ${flag.flag_type}`}>{flag.flag_type}</span></td>
      <td style={{ minWidth: 180 }}><VariantBar variants={[{ name: flag.status === 'enabled' ? 'on' : 'off', alloc: 100 }]} /></td>
      <td>
        <div style={{ display: 'flex', alignItems: 'center', gap: 8 }}>
          <Sparkline data={[]} height={20} />
          <span style={{ fontFamily: 'var(--font-mono)', fontSize: 12, color: 'var(--fg-muted)' }}>—</span>
        </div>
      </td>
      <td><span style={{ color: 'var(--fg-faint)' }}>—</span></td>
      <td><span style={{ color: 'var(--fg-muted)' }}>—</span></td>
      <td style={{ color: 'var(--fg-muted)', fontFamily: 'var(--font-mono)', fontSize: 11 }}>{new Date(flag.updated_at).toLocaleDateString()}</td>
      <td><I.chevronRight size={14} stroke="var(--fg-subtle)" /></td>
    </tr>
  )
}

function FlagCard({ flag, orgId, onToggle }: { flag: FlagResponse; orgId: string; onToggle: (key: string) => void }) {
  const navigate = useNavigate()
  return (
    <div className="card" style={{ cursor: 'pointer' }} onClick={() => navigate(`/org/${orgId}/flags/${flag.key}`)}>
      <div style={{ padding: 14, borderBottom: '1px solid var(--border-faint)' }}>
        <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'flex-start', marginBottom: 8 }}>
          <Toggle on={flag.status === 'enabled'} onClick={(e) => { e.stopPropagation(); onToggle(flag.key) }} />
          <span className={`type-pill ${flag.flag_type}`}>{flag.flag_type}</span>
        </div>
        <div style={{ fontFamily: 'var(--font-mono)', fontWeight: 600, fontSize: 13, marginBottom: 2 }}>{flag.key}</div>
        <div style={{ fontSize: 12, color: 'var(--fg-muted)' }}>{flag.name}</div>
      </div>
      <div style={{ padding: 14 }}>
        <VariantBar variants={[{ name: flag.status === 'enabled' ? 'on' : 'off', alloc: 100 }]} />
        <div style={{ display: 'flex', justifyContent: 'space-between', marginTop: 10, fontSize: 11, color: 'var(--fg-muted)' }}>
          <span>v{flag.version}</span>
          <span>{new Date(flag.updated_at).toLocaleDateString()}</span>
        </div>
      </div>
    </div>
  )
}

export function FlagsList() {
  const navigate = useNavigate()
  const { tweaks } = useTweaks()
  const { projectId, orgId } = useOrgContext()
  const [layout, setLayout] = useState<'table' | 'cards' | 'grouped'>(tweaks.flagsLayout)
  const [search, setSearch] = useState('')
  const [flags, setFlags] = useState<FlagResponse[]>([])
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [enabledSet, setEnabledSet] = useState<Set<string>>(new Set())

  useEffect(() => {
    if (!projectId) return
    setLoading(true)
    setError(null)
    api.get<FlagResponse[]>(`/v1/projects/${projectId}/flags`)
      .then(({ data }) => {
        setFlags(data)
        setEnabledSet(new Set(data.filter((f) => f.status === 'enabled').map((f) => f.key)))
      })
      .catch((err) => setError(err?.response?.data?.message ?? err.message ?? 'Failed to load flags'))
      .finally(() => setLoading(false))
  }, [projectId])

  function toggleFlag(key: string) {
    setEnabledSet((prev) => {
      const next = new Set(prev)
      if (next.has(key)) next.delete(key)
      else next.add(key)
      return next
    })
  }

  const displayFlags = flags.map((f) => ({ ...f, status: enabledSet.has(f.key) ? 'enabled' : 'disabled' }))

  const filtered = displayFlags.filter((f) =>
    !search || f.key.includes(search) || f.name.toLowerCase().includes(search.toLowerCase())
  )

  const onCount = filtered.filter((f) => f.status === 'enabled').length
  const offCount = filtered.filter((f) => f.status === 'disabled').length

  if (!projectId) {
    return (
      <>
        <PageHeader
          crumbs={['Flags']}
          title="Feature Flags"
          subtitle="Manage feature flags for your project."
        />
        <div className="page-body">
          <div className="card">
            <div className="empty">
              <div className="empty-icon"><I.flag size={20} /></div>
              <div className="empty-title">No project selected</div>
              <div className="empty-desc">No project selected — set a project ID in environments settings</div>
              <button className="btn primary" style={{ marginTop: 8 }} onClick={() => navigate(`/org/${orgId}/environments`)}>
                Go to Environments
              </button>
            </div>
          </div>
        </div>
      </>
    )
  }

  return (
    <>
      <PageHeader
        crumbs={['Flags']}
        title="Feature Flags"
        subtitle={`${flags.length} flags. First-true rule wins; flag types are immutable after creation.`}
        actions={
          <>
            <button className="btn"><I.filter size={13} /> Filters</button>
            <button className="btn"><I.archive size={13} /> Archived</button>
            <button className="btn primary" onClick={() => navigate(`/org/${orgId}/flags/new`)}><I.plus size={14} /> New flag</button>
          </>
        }
      />
      <div className="page-body">
        {loading && (
          <div style={{ display: 'flex', justifyContent: 'center', padding: 48 }}>
            <span style={{ color: 'var(--fg-muted)', fontSize: 14 }}>Loading flags…</span>
          </div>
        )}

        {error && !loading && (
          <div style={{ padding: '12px 16px', background: 'var(--danger-bg)', border: '1px solid rgba(196,43,28,0.3)', borderRadius: 8, color: 'var(--danger)', fontSize: 13, marginBottom: 16 }}>
            <I.alert size={14} style={{ verticalAlign: 'middle', marginRight: 6 }} />
            {error}
          </div>
        )}

        {!loading && !error && (
          <>
            <div style={{ display: 'flex', gap: 12, alignItems: 'center', marginBottom: 16 }}>
              <div className="search-input" style={{ maxWidth: 320 }}>
                <I.search size={14} />
                <input
                  className="input"
                  placeholder="Filter by key, name…"
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
              </div>
            </div>

            {flags.length === 0 && (
              <div className="card">
                <div className="empty">
                  <div className="empty-icon"><I.flag size={20} /></div>
                  <div className="empty-title">No flags yet</div>
                  <div className="empty-desc">Create your first feature flag to start controlling feature rollouts.</div>
                  <button className="btn primary" style={{ marginTop: 8 }}><I.plus size={13} /> New flag</button>
                </div>
              </div>
            )}

            {layout === 'table' && flags.length > 0 && (
              <div className="card">
                <div className="table-wrap">
                  <table className="table">
                    <thead>
                      <tr>
                        <th style={{ width: 56 }}></th>
                        <th>Key</th>
                        <th>Type</th>
                        <th>Status</th>
                        <th>30d evals</th>
                        <th>Segments</th>
                        <th>Owner</th>
                        <th>Updated</th>
                        <th></th>
                      </tr>
                    </thead>
                    <tbody>
                      {filtered.map((f) => <FlagTableRow key={f.key} flag={f} orgId={orgId} onToggle={toggleFlag} />)}
                    </tbody>
                  </table>
                </div>
              </div>
            )}

            {layout === 'cards' && flags.length > 0 && (
              <div style={{ display: 'grid', gridTemplateColumns: 'repeat(auto-fill, minmax(320px, 1fr))', gap: 12 }}>
                {filtered.map((f) => <FlagCard key={f.key} flag={f} orgId={orgId} onToggle={toggleFlag} />)}
              </div>
            )}

            {layout === 'grouped' && flags.length > 0 && (
              <div className="stack">
                <div className="card">
                  <div className="card-header">
                    <div className="card-title">
                      <I.flag size={14} /> All Flags <span className="badge">{filtered.length}</span>
                    </div>
                  </div>
                  <table className="table">
                    <tbody>
                      {filtered.map((f) => (
                        <tr key={f.key} className="row-clickable" onClick={() => navigate(`/org/${orgId}/flags/${f.key}`)}>
                          <td style={{ width: 40 }}>
                            <Toggle on={f.status === 'enabled'} onClick={(e) => { e.stopPropagation(); toggleFlag(f.key) }} />
                          </td>
                          <td><span className="mono-key">{f.key}</span></td>
                          <td><span className={`type-pill ${f.flag_type}`}>{f.flag_type}</span></td>
                          <td style={{ color: 'var(--fg-muted)', fontFamily: 'var(--font-mono)', fontSize: 11 }}>{new Date(f.updated_at).toLocaleDateString()}</td>
                        </tr>
                      ))}
                    </tbody>
                  </table>
                </div>
              </div>
            )}
          </>
        )}
      </div>
    </>
  )
}
