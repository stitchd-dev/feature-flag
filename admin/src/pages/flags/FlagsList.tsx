import { useState } from 'react'
import { useNavigate } from 'react-router-dom'
import { useTweaks } from '../../hooks/useTweaks'
import { usePermissions } from '../../hooks/usePermissions'
import { usePaginatedList } from '../../hooks/usePaginatedList'
import { PageHeader, VariantBar, Sparkline } from '../../components/primitives'
import { I } from '../../components/icons'
import { Pagination } from '../../components/Pagination'
import { LoadingSpinner } from '../../components/LoadingSpinner'
import { ErrorBanner } from '../../components/ErrorBanner'
import { EmptyState } from '../../components/EmptyState'
import { PermissionGate } from '../../components/PermissionGate'
import { useOrgContext } from '../../context/OrgContext'
import { api } from '../../lib/api'
import { PERMISSIONS } from '../../lib/permissions'
import type { AdminFlagResponse, CursorPage } from '../../lib/types'
import { CreateFlagModal } from './CreateFlagModal'
import { FlagBadges, buildPrerequisiteOfSet } from './flagBadges'

const PER_PAGE = 50

type FlagTypeFilter = 'all' | 'bool' | 'string' | 'int' | 'double' | 'json'

function Toggle({ on, onClick }: { on: boolean; onClick: (e: React.MouseEvent) => void }) {
  return <span className={`toggle ${on ? 'on' : ''}`} onClick={onClick} />
}

function FlagTableRow({ flag, orgId, onToggle, isPrerequisiteOf }: { flag: AdminFlagResponse; orgId: string; onToggle: (key: string) => void; isPrerequisiteOf: boolean }) {
  const navigate = useNavigate()
  return (
    <tr className="row-clickable" onClick={() => navigate(`/org/${orgId}/flags/${flag.key}`)}>
      <td><Toggle on={flag.enabled} onClick={(e) => { e.stopPropagation(); onToggle(flag.key) }} /></td>
      <td>
        <div style={{ display: 'flex', flexDirection: 'column', gap: 2 }}>
          <span style={{ display: 'flex', alignItems: 'center', gap: 6, flexWrap: 'wrap' }}>
            <span className="mono-key">{flag.key}</span>
            <FlagBadges flag={flag} isPrerequisiteOf={isPrerequisiteOf} />
          </span>
          <span style={{ fontSize: 11, color: 'var(--fg-muted)' }}>{flag.name}</span>
        </div>
      </td>
      <td><span className={`type-pill ${flag.flag_type}`}>{flag.flag_type}</span></td>
      <td style={{ minWidth: 180 }}><VariantBar variants={[{ name: flag.enabled ? 'on' : 'off', alloc: 100 }]} /></td>
      <td>
        <div style={{ display: 'flex', alignItems: 'center', gap: 8 }}>
          <Sparkline data={[]} height={20} />
          <span style={{ fontFamily: 'var(--font-mono)', fontSize: 12, color: 'var(--fg-muted)' }}>—</span>
        </div>
      </td>
      <td><span style={{ color: 'var(--fg-faint)' }}>—</span></td>
      <td><span style={{ color: 'var(--fg-muted)' }}>—</span></td>
      <td style={{ color: 'var(--fg-muted)', fontFamily: 'var(--font-mono)', fontSize: 11 }}>
        {flag.updated_at ? new Date(flag.updated_at).toLocaleDateString() : '—'}
      </td>
      <td><I.chevronRight size={14} stroke="var(--fg-subtle)" /></td>
    </tr>
  )
}

function FlagCard({ flag, orgId, onToggle, isPrerequisiteOf }: { flag: AdminFlagResponse; orgId: string; onToggle: (key: string) => void; isPrerequisiteOf: boolean }) {
  const navigate = useNavigate()
  return (
    <div className="card" style={{ cursor: 'pointer' }} onClick={() => navigate(`/org/${orgId}/flags/${flag.key}`)}>
      <div style={{ padding: 14, borderBottom: '1px solid var(--border-faint)' }}>
        <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'flex-start', marginBottom: 8 }}>
          <Toggle on={flag.enabled} onClick={(e) => { e.stopPropagation(); onToggle(flag.key) }} />
          <span className={`type-pill ${flag.flag_type}`}>{flag.flag_type}</span>
        </div>
        <div style={{ fontFamily: 'var(--font-mono)', fontWeight: 600, fontSize: 13, marginBottom: 2 }}>{flag.key}</div>
        <div style={{ fontSize: 12, color: 'var(--fg-muted)' }}>{flag.name}</div>
        <div style={{ marginTop: 6 }}>
          <FlagBadges flag={flag} isPrerequisiteOf={isPrerequisiteOf} />
        </div>
      </div>
      <div style={{ padding: 14 }}>
        <VariantBar variants={[{ name: flag.enabled ? 'on' : 'off', alloc: 100 }]} />
        <div style={{ display: 'flex', justifyContent: 'space-between', marginTop: 10, fontSize: 11, color: 'var(--fg-muted)' }}>
          <span>v{flag.version}</span>
          <span>{flag.updated_at ? new Date(flag.updated_at).toLocaleDateString() : '—'}</span>
        </div>
      </div>
    </div>
  )
}

export function FlagsList() {
  const navigate = useNavigate()
  const { tweaks } = useTweaks()
  const { projectId, orgId } = useOrgContext()
  const { can } = usePermissions()
  const [layout, setLayout] = useState<'table' | 'cards' | 'grouped'>(tweaks.flagsLayout)
  const [search, setSearch] = useState('')
  const [typeFilter, setTypeFilter] = useState<FlagTypeFilter>('all')
  const [showArchived, setShowArchived] = useState(false)
  const [optimisticEnabled, setOptimisticEnabled] = useState<Map<string, boolean>>(new Map())
  const [showCreate, setShowCreate] = useState(false)

  const canWrite = can(PERMISSIONS.FLAG_WRITE)

  const { data: flags, loading, error, hasNext, hasPrev, onNext, onPrev } = usePaginatedList<AdminFlagResponse>(
    async ({ cursor, limit, signal }) => {
      if (!projectId) return { items: [], next_cursor: null }
      const qs = new URLSearchParams({ limit: String(limit) })
      if (cursor != null) qs.set('cursor', cursor)
      if (showArchived) qs.set('include_archived', 'true')
      const { data } = await api.get<CursorPage<AdminFlagResponse>>(
        `/v1/projects/${projectId}/flags?${qs}`,
        { signal },
      )
      const items = data.items ?? (Array.isArray(data) ? data : [])
      setOptimisticEnabled(new Map(items.map((f) => [f.key, f.enabled])))
      return { items, next_cursor: data.next_cursor ?? null }
    },
    [projectId, showArchived],
    PER_PAGE,
  )

  function toggleFlag(key: string) {
    const current = optimisticEnabled.get(key) ?? flags.find((f) => f.key === key)?.enabled ?? false
    const next = !current
    // Optimistic update
    setOptimisticEnabled((prev) => new Map(prev).set(key, next))
    // Fire API call
    const flag = flags.find((f) => f.key === key)
    if (!flag || !projectId) return
    api.put(`/v1/projects/${projectId}/flags/${key}`, { enabled: next, version: flag.version })
      .catch(() => {
        // Revert on failure
        setOptimisticEnabled((prev) => new Map(prev).set(key, current))
      })
  }

  const displayFlags = flags.map((f) => ({ ...f, enabled: optimisticEnabled.get(f.key) ?? f.enabled }))
  // Keys referenced as a prerequisite by some flag in the page → "is a prerequisite" badge.
  const prerequisiteOf = buildPrerequisiteOfSet(flags)

  const filtered = displayFlags.filter((f) => {
    if (search && !f.key.includes(search) && !f.name.toLowerCase().includes(search.toLowerCase())) return false
    if (typeFilter !== 'all' && f.flag_type !== typeFilter) return false
    return true
  })

  const onCount = filtered.filter((f) => f.enabled).length
  const offCount = filtered.filter((f) => !f.enabled).length

  const flagsLockFallback = (
    <>
      <PageHeader crumbs={['Flags']} title="Feature Flags" subtitle="Manage feature flags for your project." />
      <div className="page-body">
        <div className="card" style={{ padding: 48, textAlign: 'center' }}>
          <I.lock size={28} stroke="var(--fg-subtle)" style={{ marginBottom: 12 }} />
          <div style={{ fontWeight: 600, fontSize: 15, marginBottom: 6 }}>Access restricted</div>
          <div style={{ color: 'var(--fg-muted)', fontSize: 13 }}>You do not have permission to view feature flags.</div>
        </div>
      </div>
    </>
  )

  if (!projectId) {
    return (
      <PermissionGate permission={PERMISSIONS.FLAG_READ} fallback={flagsLockFallback}>
        <>
          <PageHeader
            crumbs={['Flags']}
            title="Feature Flags"
            subtitle="Manage feature flags for your project."
          />
          <div className="page-body">
            <div className="card">
              <EmptyState
                icon={<I.flag size={20} />}
                title="No project selected"
                desc="No project selected — set a project ID in environments settings"
                action={<button className="btn primary" onClick={() => navigate(`/org/${orgId}/environments`)}>Go to Environments</button>}
              />
            </div>
          </div>
        </>
      </PermissionGate>
    )
  }

  return (
    <PermissionGate permission={PERMISSIONS.FLAG_READ} fallback={flagsLockFallback}>
      <>
        <PageHeader
        crumbs={['Flags']}
        title="Feature Flags"
        subtitle="First-true rule wins; flag types are immutable after creation."
        actions={
          <>
            <button
              className={`btn ${showArchived ? 'active' : ''}`}
              onClick={() => setShowArchived((v) => !v)}
            >
              <I.archive size={13} /> {showArchived ? 'Hide archived' : 'Show archived'}
            </button>
            <button
              className="btn primary"
              disabled={!canWrite}
              title={canWrite ? undefined : 'No permission to create flags'}
              style={{ opacity: canWrite ? 1 : 0.35 }}
              onClick={() => canWrite && setShowCreate(true)}
            >
              <I.plus size={14} /> New flag
            </button>
          </>
        }
      />
      <div className="page-body">
        {loading && (
          <div style={{ display: 'flex', justifyContent: 'center', padding: 48 }}>
            <LoadingSpinner label="Loading flags…" />
          </div>
        )}

        {error && !loading && (
          <ErrorBanner
            message={error}
            icon={<I.alert size={14} />}
          />
        )}

        {!loading && !error && (
          <>
            <div style={{ display: 'flex', gap: 12, alignItems: 'center', marginBottom: 8, flexWrap: 'wrap' }}>
              <div className="search-input" style={{ maxWidth: 280 }}>
                <I.search size={14} />
                <input
                  className="input"
                  placeholder="Filter by key, name…"
                  value={search}
                  onChange={(e) => setSearch(e.target.value)}
                />
              </div>
              {/* Type filter chips */}
              <div style={{ display: 'flex', gap: 4, flexWrap: 'wrap' }}>
                {(['all', 'bool', 'string', 'int', 'double', 'json'] as const).map((t) => (
                  <button
                    key={t}
                    className={`btn sm ${typeFilter === t ? 'active' : ''}`}
                    style={{ fontFamily: t !== 'all' ? 'var(--font-mono)' : undefined }}
                    onClick={() => setTypeFilter(t)}
                  >
                    {t}
                  </button>
                ))}
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
                <EmptyState
                  icon={<I.flag size={20} />}
                  title="No flags yet"
                  desc="Create your first feature flag to start controlling feature rollouts."
                  action={<button className="btn primary" onClick={() => setShowCreate(true)}><I.plus size={13} /> New flag</button>}
                />
              </div>
            )}

            {flags.length > 0 && filtered.length === 0 && (
              <div className="card">
                <EmptyState
                  icon={<I.search size={20} />}
                  title="No results"
                  desc="No flags match the current filter. Try adjusting your search or filters."
                  action={<button className="btn" onClick={() => { setSearch(''); }}>Clear filter</button>}
                />
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
                      {filtered.map((f) => <FlagTableRow key={f.key} flag={f} orgId={orgId} onToggle={toggleFlag} isPrerequisiteOf={prerequisiteOf.has(f.key)} />)}
                    </tbody>
                  </table>
                </div>
                <Pagination hasPrev={hasPrev} hasNext={hasNext} onPrev={onPrev} onNext={onNext} />
              </div>
            )}

            {layout === 'cards' && flags.length > 0 && (
              <div style={{ display: 'grid', gridTemplateColumns: 'repeat(auto-fill, minmax(320px, 1fr))', gap: 12 }}>
                {filtered.map((f) => <FlagCard key={f.key} flag={f} orgId={orgId} onToggle={toggleFlag} isPrerequisiteOf={prerequisiteOf.has(f.key)} />)}
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
                            <Toggle on={f.enabled} onClick={(e) => { e.stopPropagation(); toggleFlag(f.key) }} />
                          </td>
                          <td>
                            <span style={{ display: 'flex', alignItems: 'center', gap: 6, flexWrap: 'wrap' }}>
                              <span className="mono-key">{f.key}</span>
                              <FlagBadges flag={f} isPrerequisiteOf={prerequisiteOf.has(f.key)} />
                            </span>
                          </td>
                          <td><span className={`type-pill ${f.flag_type}`}>{f.flag_type}</span></td>
                          <td style={{ color: 'var(--fg-muted)', fontFamily: 'var(--font-mono)', fontSize: 11 }}>
                            {f.updated_at ? new Date(f.updated_at).toLocaleDateString() : '—'}
                          </td>
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

      {showCreate && <CreateFlagModal onClose={() => setShowCreate(false)} />}
      </>
    </PermissionGate>
  )
}
