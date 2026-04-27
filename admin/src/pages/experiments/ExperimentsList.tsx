import { useState } from 'react'
import { useNavigate } from 'react-router-dom'
import { PageHeader } from '../../components/primitives'
import { I } from '../../components/icons'
import { EXPERIMENTS } from '../../lib/mockData'
import type { Experiment } from '../../lib/mockData'

type StateFilter = 'All' | 'Running' | 'Draft' | 'Stopped' | 'Completed'

const STATE_FILTERS: StateFilter[] = ['All', 'Running', 'Draft', 'Stopped', 'Completed']

function stateMatch(exp: Experiment, filter: StateFilter): boolean {
  if (filter === 'All') return true
  return exp.state === filter.toLowerCase()
}

function stateBadgeClass(state: string): string {
  if (state === 'running') return 'info'
  if (state === 'completed') return 'success'
  if (state === 'stopped') return 'danger'
  return ''
}

export function ExperimentsList() {
  const navigate = useNavigate()
  const [search, setSearch] = useState('')
  const [stateFilter, setStateFilter] = useState<StateFilter>('All')

  const filtered = EXPERIMENTS.filter((e) =>
    stateMatch(e, stateFilter) &&
    (!search || e.name.toLowerCase().includes(search.toLowerCase()) || e.key.includes(search) || e.flag.includes(search))
  )

  return (
    <>
      <PageHeader
        crumbs={['stitchd-app', 'Experiments']}
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
        <div className="card">
          <table className="table">
            <thead>
              <tr>
                <th>Experiment</th>
                <th>Flag</th>
                <th>Model</th>
                <th>Status</th>
                <th>Primary metric</th>
                <th>Lift</th>
                <th>Confidence</th>
                <th>Samples</th>
                <th></th>
              </tr>
            </thead>
            <tbody>
              {filtered.map((e) => (
                <tr key={e.key} className="row-clickable" onClick={() => navigate(`/experiments/${e.key}`)}>
                  <td>
                    <div style={{ fontWeight: 600 }}>{e.name}</div>
                    <div style={{ fontSize: 11, color: 'var(--fg-muted)', fontFamily: 'var(--font-mono)' }}>{e.key}</div>
                  </td>
                  <td><span className="mono-key" style={{ fontSize: 11 }}>{e.flag}</span></td>
                  <td><span className="badge">{e.model}</span></td>
                  <td>
                    <span className={`badge ${stateBadgeClass(e.state)}`}>
                      {e.state === 'running' && <span className="dot" style={{ background: 'currentColor' }} />}
                      {e.state}
                    </span>
                  </td>
                  <td><span className="mono-key" style={{ fontSize: 11 }}>{e.primary}</span></td>
                  <td style={{ color: e.lift.startsWith('+') ? 'var(--success)' : e.lift.startsWith('−') ? 'var(--danger)' : 'var(--fg-faint)', fontFamily: 'var(--font-mono)', fontWeight: 600 }}>{e.lift}</td>
                  <td>
                    <div style={{ display: 'flex', alignItems: 'center', gap: 6 }}>
                      <div style={{ width: 60, height: 4, background: 'var(--bg-sunken)', borderRadius: 2, overflow: 'hidden' }}>
                        <div style={{ width: `${e.confidence}%`, height: '100%', background: e.confidence >= 95 ? 'var(--success)' : e.confidence >= 80 ? 'var(--warning)' : 'var(--fg-faint)' }} />
                      </div>
                      <span style={{ fontFamily: 'var(--font-mono)', fontSize: 11 }}>{e.confidence}%</span>
                    </div>
                  </td>
                  <td style={{ fontFamily: 'var(--font-mono)' }}>{e.samples}</td>
                  <td><I.chevronRight size={14} stroke="var(--fg-subtle)" /></td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      </div>
    </>
  )
}
