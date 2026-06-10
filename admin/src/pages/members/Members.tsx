import { useCallback, useEffect, useState } from 'react'
import { listOrgMembers, removeOrgMember, type OrgMemberSummary } from '../../lib/api'
import { PageHeader } from '../../components/primitives'
import { EmptyState } from '../../components/EmptyState'
import { LoadingSpinner } from '../../components/LoadingSpinner'
import { ErrorBanner } from '../../components/ErrorBanner'
import { ConfirmDialog } from '../../components/ConfirmDialog'
import { I } from '../../components/icons'
import { useOrgContext } from '../../context/OrgContext'
import { usePermissions } from '../../hooks/usePermissions'
import { MembersTable } from './MembersTable'
import { AddMemberModal } from './AddMemberModal'
import { RolesInfo } from './RolesInfo'
import { SsoProviders } from './SsoProviders'

type Tab = 'members' | 'roles' | 'sso'

/**
 * Members & Roles page — real org-member management backed by the management
 * org-user API plus the SSO-provider API. There is no email-invite, role-change
 * or custom-role API in the backend, so this page does not pretend otherwise:
 * roles are read-only, the action is "Add member", and the Roles tab documents
 * the fixed two-role model. (See track members_roles_20260610.)
 */
export function Members() {
  const { orgId } = useOrgContext()
  const { roles, loading: permLoading } = usePermissions()
  const canManage = roles.includes('org_admin')

  const [tab, setTab] = useState<Tab>('members')
  const [members, setMembers] = useState<OrgMemberSummary[]>([])
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  const [showAdd, setShowAdd] = useState(false)
  const [removing, setRemoving] = useState<OrgMemberSummary | null>(null)

  const load = useCallback(() => {
    if (!orgId) return undefined
    setLoading(true)
    setError(null)
    const controller = new AbortController()
    listOrgMembers(orgId, undefined, controller.signal)
      .then((page) => setMembers(page.items))
      .catch((err: unknown) => {
        if (!controller.signal.aborted) {
          const status = (err as { response?: { status?: number } }).response?.status
          setError(status === 403 ? 'You do not have permission to view members for this organisation.' : 'Failed to load members.')
        }
      })
      .finally(() => setLoading(false))
    return controller
  }, [orgId])

  useEffect(() => {
    const controller = load()
    return () => controller?.abort()
  }, [load])

  async function handleRemove(m: OrgMemberSummary) {
    setBusy(true)
    setError(null)
    try {
      await removeOrgMember(orgId, m.user_id)
      setRemoving(null)
      load()
    } catch (err: unknown) {
      const status = (err as { response?: { status?: number } }).response?.status
      setError(status === 409 ? 'Cannot remove this member (they may be the last admin).' : 'Failed to remove member.')
      setRemoving(null)
    } finally {
      setBusy(false)
    }
  }

  return (
    <>
      <PageHeader
        crumbs={['Members']}
        title="Members & Roles"
        subtitle="Manage who can access this organisation and how they sign in."
        actions={
          tab === 'members' && canManage ? (
            <button className="btn primary" disabled={busy} onClick={() => setShowAdd(true)}>
              <I.plus size={14} /> Add member
            </button>
          ) : undefined
        }
      />
      <div className="page-body">
        <div className="tabs">
          <button className={`tab ${tab === 'members' ? 'active' : ''}`} onClick={() => setTab('members')}>
            Members {!loading && <span className="count">{members.length}</span>}
          </button>
          <button className={`tab ${tab === 'roles' ? 'active' : ''}`} onClick={() => setTab('roles')}>Roles</button>
          <button className={`tab ${tab === 'sso' ? 'active' : ''}`} onClick={() => setTab('sso')}>SSO providers</button>
        </div>

        {tab === 'members' && (
          <>
            {error && <ErrorBanner message={error} onDismiss={() => setError(null)} />}
            {loading ? (
              <div className="card" style={{ padding: 32 }}><LoadingSpinner label="Loading members…" /></div>
            ) : members.length === 0 ? (
              <EmptyState
                icon={<I.users size={20} />}
                title="No members yet"
                desc="Add the first member to give your team access to this organisation."
                action={canManage ? (
                  <button className="btn primary" onClick={() => setShowAdd(true)}><I.plus size={13} /> Add member</button>
                ) : undefined}
              />
            ) : (
              <MembersTable members={members} canManage={canManage} onRemove={(m) => setRemoving(m)} busy={busy} />
            )}
          </>
        )}

        {tab === 'roles' && <RolesInfo />}

        {tab === 'sso' && !permLoading && <SsoProviders orgId={orgId} canManage={canManage} />}
      </div>

      {showAdd && (
        <AddMemberModal
          orgId={orgId}
          onClose={() => setShowAdd(false)}
          onCreated={() => { setShowAdd(false); load() }}
        />
      )}
      {removing && (
        <ConfirmDialog
          title="Remove member"
          message={`Remove ${removing.display_name || removing.email} from this organisation? They will lose access immediately.`}
          confirmLabel="Remove"
          confirmDanger
          onConfirm={() => void handleRemove(removing)}
          onCancel={() => setRemoving(null)}
        />
      )}
    </>
  )
}
