import { useState } from 'react'
import { listAuditLog, type AuditEntry } from '../../lib/api'
import { PageHeader } from '../../components/primitives'
import { EmptyState } from '../../components/EmptyState'
import { LoadingSpinner } from '../../components/LoadingSpinner'
import { ErrorBanner } from '../../components/ErrorBanner'
import { Pagination } from '../../components/Pagination'
import { I } from '../../components/icons'
import { useOrgContext } from '../../context/OrgContext'
import { usePaginatedList } from '../../hooks/usePaginatedList'

const PER_PAGE = 50

/**
 * Audit Log — real, org-scoped audit trail captured by the gateway edge audit
 * middleware (every successful admin mutation: actor + action + resource). No
 * fabricated data; entries appear as actions happen (no historical back-fill).
 */
export function AuditLog() {
  const { orgId } = useOrgContext()
  const [resourceType, setResourceType] = useState('')
  const [action, setAction] = useState('')

  const { data, loading, error, hasNext, hasPrev, onNext, onPrev } = usePaginatedList<AuditEntry>(
    async ({ cursor, limit, signal }) => {
      if (!orgId) return { items: [], next_cursor: null }
      const page = await listAuditLog(
        orgId,
        { cursor, limit, resource_type: resourceType || undefined, action: action || undefined },
        signal,
      )
      return { items: page.items, next_cursor: page.next_cursor }
    },
    [orgId, resourceType, action],
    PER_PAGE,
  )

  return (
    <>
      <PageHeader
        crumbs={['Audit']}
        title="Audit Log"
        subtitle="Every successful admin mutation is recorded with actor, action and resource. Capture began when audit logging was enabled — earlier changes are not back-filled."
      />
      <div className="page-body">
        <div style={{ display: 'flex', gap: 8, marginBottom: 16, flexWrap: 'wrap' }}>
          <div className="search-input">
            <I.filter size={14} />
            <input
              className="input"
              placeholder="Resource type (e.g. flag)"
              value={resourceType}
              onChange={(e) => setResourceType(e.target.value.trim())}
            />
          </div>
          <div className="search-input">
            <I.search size={14} />
            <input
              className="input"
              placeholder="Action (e.g. flag.update)"
              value={action}
              onChange={(e) => setAction(e.target.value.trim())}
            />
          </div>
        </div>

        {error && <ErrorBanner message={error} />}

        {loading ? (
          <div className="card" style={{ padding: 32 }}>
            <LoadingSpinner label="Loading audit log…" />
          </div>
        ) : data.length === 0 ? (
          <EmptyState
            icon={<I.history size={20} />}
            title="No audit entries"
            desc={
              resourceType || action
                ? 'No entries match these filters.'
                : 'Admin actions (flag, segment, experiment, member, SDK key… changes) will appear here as they happen.'
            }
          />
        ) : (
          <div className="card">
            <table className="table">
              <thead>
                <tr>
                  <th>When</th>
                  <th>Actor</th>
                  <th>Action</th>
                  <th>Resource</th>
                </tr>
              </thead>
              <tbody>
                {data.map((e) => (
                  <tr key={e.id}>
                    <td style={{ fontFamily: 'var(--font-mono)', fontSize: 12, color: 'var(--fg-muted)' }}>
                      {new Date(e.created_at).toLocaleString()}
                    </td>
                    <td>
                      {e.actor_email ? (
                        <strong>{e.actor_email}</strong>
                      ) : e.actor_id ? (
                        <span className="mono-key">{e.actor_id.slice(0, 8)}…</span>
                      ) : (
                        <span style={{ display: 'flex', alignItems: 'center', gap: 6, color: 'var(--fg-muted)' }}>
                          <I.cog size={12} /> system
                        </span>
                      )}
                    </td>
                    <td><span className="badge">{e.action}</span></td>
                    <td>
                      <span className="mono-key">
                        {e.resource_type}
                        {e.resource_ref ? ` · ${e.resource_ref}` : ''}
                      </span>
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
            <Pagination hasPrev={hasPrev} hasNext={hasNext} onPrev={onPrev} onNext={onNext} />
          </div>
        )}
      </div>
    </>
  )
}
