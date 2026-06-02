/**
 * Exclusion-groups management page (Phase 8 — xexp, P8.T1).
 *
 * Lists the current environment's mutual-exclusion groups with a per-group
 * capacity view (allocated vs free bp bar + member count) and create / edit /
 * delete actions. Follows the SegmentsList conventions: env-scoped routing,
 * `usePaginatedList`, the shared list/empty/error primitives, and Formik+Yup
 * modals.
 */
import { useEffect, useState } from 'react'
import { useNavigate } from 'react-router-dom'
import { PageHeader } from '../../../components/primitives'
import { I } from '../../../components/icons'
import { Pagination } from '../../../components/Pagination'
import { LoadingSpinner } from '../../../components/LoadingSpinner'
import { ErrorBanner } from '../../../components/ErrorBanner'
import { EmptyState } from '../../../components/EmptyState'
import { ConfirmDialog } from '../../../components/ConfirmDialog'
import { usePaginatedList } from '../../../hooks/usePaginatedList'
import { useOrgContext } from '../../../context/OrgContext'
import { extractErrorMessage } from '../../../lib/errors'
import { listExperiments } from '../../../lib/api'
import {
  listExclusionGroups,
  deleteExclusionGroup,
  type ExclusionGroup,
} from '../../../lib/api/exclusionGroups'
import { countMembersByGroup } from './capacity'
import { CapacityBar } from './CapacityBar'
import { ExclusionGroupModal } from './ExclusionGroupModal'

const PER_PAGE = 50

export function ExclusionGroups() {
  const navigate = useNavigate()
  const { envId, orgId } = useOrgContext()
  const [search, setSearch] = useState('')
  const [showCreate, setShowCreate] = useState(false)
  const [editGroup, setEditGroup] = useState<ExclusionGroup | null>(null)
  const [deleteTarget, setDeleteTarget] = useState<ExclusionGroup | null>(null)
  const [deleteError, setDeleteError] = useState<string | null>(null)

  const {
    data: groups,
    total,
    loading,
    error,
    page,
    onPageChange,
    refresh,
  } = usePaginatedList<ExclusionGroup>(
    async ({ page: p, perPage, signal }) => {
      if (!envId) return { items: [], total: 0 }
      const res = await listExclusionGroups(
        envId,
        { page: p, per_page: perPage },
        signal,
      )
      const items = res.items ?? []
      return { items, total: res.total ?? items.length }
    },
    [envId],
    PER_PAGE,
  )

  // Member counts — derived by tallying experiments' exclusion_group_id.
  const [memberCounts, setMemberCounts] = useState<Record<string, number>>({})
  useEffect(() => {
    if (!envId) return
    const ctrl = new AbortController()
    listExperiments(envId, ctrl.signal)
      .then(({ items }) => {
        if (ctrl.signal.aborted) return
        setMemberCounts(countMembersByGroup(items))
      })
      .catch(() => undefined)
    return () => ctrl.abort()
  }, [envId, total])

  const filtered = groups.filter(
    (g) =>
      !search ||
      g.name.toLowerCase().includes(search.toLowerCase()) ||
      (g.description ?? '').toLowerCase().includes(search.toLowerCase()),
  )

  async function confirmDelete() {
    if (!envId || !deleteTarget) return
    setDeleteError(null)
    try {
      await deleteExclusionGroup(envId, deleteTarget.id)
      setDeleteTarget(null)
      refresh()
    } catch (err: unknown) {
      setDeleteError(extractErrorMessage(err))
    }
  }

  if (!envId) {
    return (
      <>
        <PageHeader
          crumbs={['Experiments', 'Exclusion groups']}
          title="Exclusion groups"
          subtitle="Mutual-exclusion groups for experiments."
        />
        <div className="page-body">
          <div className="card">
            <EmptyState
              icon={<I.layers size={20} />}
              title="No environment selected"
              desc="Select an environment to view its exclusion groups."
              action={
                <button
                  className="btn primary"
                  onClick={() => navigate(`/org/${orgId}/environments`)}
                >
                  Go to Environments
                </button>
              }
            />
          </div>
        </div>
      </>
    )
  }

  return (
    <>
      <PageHeader
        crumbs={['Experiments', 'Exclusion groups']}
        title="Exclusion groups"
        subtitle="Experiments in the same group never share traffic — allocations are carved from a shared budget."
        actions={
          <button className="btn primary" onClick={() => setShowCreate(true)}>
            <I.plus size={14} /> New group
          </button>
        }
      />
      <div className="page-body">
        {loading && (
          <div style={{ display: 'flex', justifyContent: 'center', padding: 48 }}>
            <LoadingSpinner label="Loading exclusion groups…" />
          </div>
        )}

        {error && !loading && <ErrorBanner message={error} icon={<I.alert size={14} />} />}

        {!loading && !error && (
          <>
            <div style={{ display: 'flex', gap: 12, marginBottom: 16 }}>
              <div className="search-input">
                <I.search size={14} />
                <input
                  className="input"
                  placeholder="Filter groups…"
                  value={search}
                  onChange={(e) => setSearch(e.target.value)}
                />
              </div>
            </div>

            {groups.length === 0 && (
              <div className="card">
                <EmptyState
                  icon={<I.layers size={20} />}
                  title="No exclusion groups yet"
                  desc="Create a group to carve experiment traffic out of a shared budget."
                  action={
                    <button className="btn primary" onClick={() => setShowCreate(true)}>
                      <I.plus size={13} /> New group
                    </button>
                  }
                />
              </div>
            )}

            {groups.length > 0 && filtered.length === 0 && (
              <div className="card">
                <EmptyState
                  icon={<I.search size={20} />}
                  title="No results"
                  desc="No groups match the current filter."
                  action={<button className="btn" onClick={() => setSearch('')}>Clear filter</button>}
                />
              </div>
            )}

            {groups.length > 0 && (
              <div className="card">
                <table className="table" style={{ marginBottom: 0 }}>
                  <thead>
                    <tr>
                      <th>Name</th>
                      <th>Description</th>
                      <th style={{ width: 260 }}>Capacity</th>
                      <th></th>
                    </tr>
                  </thead>
                  <tbody>
                    {filtered.map((g) => (
                      <tr key={g.id}>
                        <td>
                          <span style={{ fontWeight: 600, fontSize: 13 }}>{g.name}</span>
                        </td>
                        <td style={{ color: 'var(--fg-muted)', fontSize: 12 }}>
                          {g.description || '—'}
                        </td>
                        <td>
                          <CapacityBar group={g} memberCount={memberCounts[g.id] ?? 0} />
                        </td>
                        <td>
                          <div style={{ display: 'flex', gap: 4, justifyContent: 'flex-end' }}>
                            <button
                              className="btn sm"
                              onClick={() => setEditGroup(g)}
                              title="Edit group"
                            >
                              <I.pencil size={12} />
                            </button>
                            <button
                              className="btn sm"
                              style={{ color: 'var(--danger)' }}
                              onClick={() => {
                                setDeleteError(null)
                                setDeleteTarget(g)
                              }}
                              title="Delete group"
                            >
                              <I.trash size={12} />
                            </button>
                          </div>
                        </td>
                      </tr>
                    ))}
                  </tbody>
                </table>
                <Pagination page={page} perPage={PER_PAGE} total={total} onChange={onPageChange} />
              </div>
            )}
          </>
        )}
      </div>

      {showCreate && (
        <ExclusionGroupModal
          envId={envId}
          onClose={() => setShowCreate(false)}
          onSaved={() => {
            setShowCreate(false)
            refresh()
          }}
        />
      )}

      {editGroup && (
        <ExclusionGroupModal
          envId={envId}
          group={editGroup}
          onClose={() => setEditGroup(null)}
          onSaved={() => {
            setEditGroup(null)
            refresh()
          }}
        />
      )}

      {deleteTarget && (
        <ConfirmDialog
          title="Delete exclusion group"
          message={
            deleteError
              ? deleteError
              : `Delete "${deleteTarget.name}"? Experiments assigned to it will become ungrouped.`
          }
          confirmLabel="Delete"
          confirmDanger
          onConfirm={confirmDelete}
          onCancel={() => setDeleteTarget(null)}
        />
      )}
    </>
  )
}
