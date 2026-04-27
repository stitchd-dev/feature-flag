import { useState } from 'react'
import { useNavigate } from 'react-router-dom'
import { PageHeader } from '../../components/primitives'
import { I } from '../../components/icons'
import { SEGMENTS } from '../../lib/mockData'

export function SegmentsList() {
  const navigate = useNavigate()
  const [search, setSearch] = useState('')

  const filtered = SEGMENTS.filter((s) =>
    !search || s.key.includes(search) || s.name.toLowerCase().includes(search.toLowerCase())
  )

  return (
    <>
      <PageHeader
        crumbs={['stitchd-app', 'Segments']}
        title="Segments"
        subtitle="Reusable groups of contexts. Rule-based segments evaluate against context fields. List-based segments are explicit include/exclude lists."
        actions={
          <>
            <button className="btn"><I.upload size={13} /> Import list</button>
            <button className="btn primary"><I.plus size={14} /> New segment</button>
          </>
        }
      />
      <div className="page-body">
        <div style={{ display: 'flex', gap: 12, marginBottom: 16 }}>
          <div className="search-input">
            <I.search size={14} />
            <input
              className="input"
              placeholder="Filter segments…"
              value={search}
              onChange={(e) => setSearch(e.target.value)}
            />
          </div>
          <button className="btn"><I.filter size={13} /> All types</button>
        </div>
        <div className="card">
          <table className="table">
            <thead>
              <tr>
                <th>Key</th>
                <th>Type</th>
                <th>Context</th>
                <th>Members</th>
                <th>Used by</th>
                <th>Updated</th>
                <th></th>
              </tr>
            </thead>
            <tbody>
              {filtered.map((s) => (
                <tr key={s.key} className="row-clickable" onClick={() => navigate(`/segments/${s.key}`)}>
                  <td>
                    <div>
                      <span className="mono-key">{s.key}</span>
                      <div style={{ fontSize: 11, color: 'var(--fg-muted)', marginTop: 2 }}>{s.name}</div>
                    </div>
                  </td>
                  <td><span className={`badge ${s.type === 'rule' ? 'info' : ''}`}>{s.type === 'rule' ? 'rule-based' : 'list-based'}</span></td>
                  <td><span className="mono-key">{s.contextType}</span></td>
                  <td style={{ fontFamily: 'var(--font-mono)', fontWeight: 600 }}>{s.members.toLocaleString()}</td>
                  <td>{s.usedBy} flags</td>
                  <td style={{ color: 'var(--fg-muted)' }}>{s.updated}</td>
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
