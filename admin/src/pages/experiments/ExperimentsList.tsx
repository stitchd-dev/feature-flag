import { useState } from 'react'
import { useNavigate } from 'react-router-dom'
import { PageHeader } from '../../components/primitives'
import { I } from '../../components/icons'
import { LoadingSpinner } from '../../components/LoadingSpinner'
import { ErrorBanner } from '../../components/ErrorBanner'
import { EmptyState } from '../../components/EmptyState'
import { usePaginatedList } from '../../hooks/usePaginatedList'
import { useOrgContext } from '../../context/OrgContext'
import { api } from '../../lib/api'

interface ExperimentResponse {
  experiment_id: string
  environment_id: string
  key: string
  name: string
  description: string
  flag_key: string
  status: string // "draft" | "running" | "stopped" | "completed"
  model: string // "frequentist" | "bayesian"
  primary_metric: string
  variants: number
  started_at: string | null
  ended_at: string | null
  created_at: string
  updated_at: string
}

type StateFilter = 'All' | 'Running' | 'Draft' | 'Stopped' | 'Completed'
const STATE_FILTERS: StateFilter[] = ['All', 'Running', 'Draft', 'Stopped', 'Completed']

function stateBadgeClass(state: string): string {
  if (state === 'running') return 'info'
  if (state === 'completed') return 'success'
  if (state === 'stopped') return 'danger'
  return ''
}

function stateMatch(exp: ExperimentResponse, filter: StateFilter): boolean {
  if (filter === 'All') return true
  return exp.status === filter.toLowerCase()
}

export function ExperimentsList() {
  const navigate = useNavigate()
  const { envId, orgId } = useOrgContext()
  const [search, setSearch] = useState('')
  const [stateFilter, setStateFilter] = useState<StateFilter>('All')

  const { data: experiments, loading, error } = usePaginatedList<ExperimentResponse>(
    async ({ signal }) => {
      if (!envId) return { items: [], total: 0 }
      const { data } = await api.get<{ items: ExperimentResponse[]; total: number }>(
        `/v1/environments/${envId}/experiments`,
        { signal },
      )
      const items = data.items ?? (Array.isArray(data) ? data : [])
      return { items, total: data.total ?? items.length }
    },
    [envId],
  )

  const filtered = experiments.filter((e) =>
    stateMatch(e, stateFilter) &&
    (!search || e.name.toLowerCase().includes(search.toLowerCase()) || e.key.includes(search) || e.flag_key.includes(search))
  )

  if (!envId) {
    return (
      <>
        <PageHeader
          crumbs={['Experiments']}
          title="Experiments"
          subtitle="A/B and multivariate tests."
        />
        <div className="page-body">
          <div className="card">
            <EmptyState
              icon={<I.beaker size={20} />}
              title="No environment selected"
              desc="No environment selected — set an environment ID in environments settings"
              action={<button className="btn primary" onClick={() => navigate(`/org/${orgId}/environments`)}>Go to Environments</button>}
            />
          </div>
        </div>
      </>
    )
  }

  return (
    <>
      <PageHeader
        crumbs={['Experiments']}
        title="Experiments"
        subtitle="Stats compute every 60 minutes. Active experiments lock their bound flag for the duration."
        actions={
          <>
            <button className="btn"><I.filter size={13} /> All states</button>
            <button className="btn primary"><I.plus size={14} /> New experiment</button>
          </>
        }
      />
      <div className="page-body">
        {loading && (
          <div style={{ display: 'flex', justifyContent: 'center', padding: 48 }}>
            <LoadingSpinner label="Loading experiments…" />
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
            <div style={{ display: 'flex', gap: 12, marginBottom: 16 }}>
              <div className="search-input">
                <I.search size={14} />
                <input
                  className="input"
                  placeholder="Filter experiments…"
                  value={search}
                  onChange={(e) => setSearch(e.target.value)}
                />
              </div>
              <div style={{ display: 'flex', gap: 4, padding: 3, background: 'var(--bg-sunken)', borderRadius: 8, border: '1px solid var(--border)' }}>
                {STATE_FILTERS.map((t) => (
                  <button
                    key={t}
                    className="btn sm"
                    style={{ border: 'none', background: stateFilter === t ? 'var(--surface)' : 'transparent', color: stateFilter === t ? 'var(--fg)' : 'var(--fg-muted)', boxShadow: stateFilter === t ? 'var(--shadow-xs)' : 'none' }}
                    onClick={() => setStateFilter(t)}
                  >
                    {t}
                  </button>
                ))}
              </div>
            </div>

            {experiments.length === 0 && (
              <div className="card">
                <EmptyState
                  icon={<I.beaker size={20} />}
                  title="No experiments yet"
                  desc="Create your first experiment to start running A/B tests."
                  action={<button className="btn primary"><I.plus size={13} /> New experiment</button>}
                />
              </div>
            )}

            {experiments.length > 0 && (
              <div className="card">
                <table className="table">
                  <thead>
                    <tr>
                      <th>Experiment</th>
                      <th>Flag</th>
                      <th>Model</th>
                      <th>Status</th>
                      <th>Primary metric</th>
                      <th>Variants</th>
                      <th>Updated</th>
                      <th></th>
                    </tr>
                  </thead>
                  <tbody>
                    {filtered.map((e) => (
                      <tr key={e.key} className="row-clickable" onClick={() => navigate(`/org/${orgId}/experiments/${e.key}`)}>
                        <td>
                          <div style={{ fontWeight: 600 }}>{e.name}</div>
                          <div style={{ fontSize: 11, color: 'var(--fg-muted)', fontFamily: 'var(--font-mono)' }}>{e.key}</div>
                        </td>
                        <td><span className="mono-key" style={{ fontSize: 11 }}>{e.flag_key}</span></td>
                        <td><span className="badge">{e.model}</span></td>
                        <td>
                          <span className={`badge ${stateBadgeClass(e.status)}`}>
                            {e.status === 'running' && <span className="dot" style={{ background: 'currentColor' }} />}
                            {e.status}
                          </span>
                        </td>
                        <td><span className="mono-key" style={{ fontSize: 11 }}>{e.primary_metric}</span></td>
                        <td style={{ fontFamily: 'var(--font-mono)' }}>{e.variants}</td>
                        <td style={{ color: 'var(--fg-muted)', fontFamily: 'var(--font-mono)', fontSize: 11 }}>{new Date(e.updated_at).toLocaleDateString()}</td>
                        <td><I.chevronRight size={14} stroke="var(--fg-subtle)" /></td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>
            )}
          </>
        )}
      </div>
    </>
  )
}
