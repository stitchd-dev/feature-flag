import { I } from '../../components/icons'
import type { OrgMemberSummary } from '../../lib/api'
import { memberInitials, roleBadge } from './membersHelpers'

interface Props {
  members: OrgMemberSummary[]
  canManage: boolean
  onRemove: (member: OrgMemberSummary) => void
  busy?: boolean
}

/** Presentational table of org members. Pure props in → markup out. */
export function MembersTable({ members, canManage, onRemove, busy = false }: Props) {
  return (
    <div className="card">
      <table className="table">
        <thead>
          <tr>
            <th>Member</th>
            <th>Role</th>
            <th>Joined</th>
            {canManage && <th />}
          </tr>
        </thead>
        <tbody>
          {members.map((m) => {
            const badge = roleBadge(m.role)
            return (
              <tr key={m.user_id}>
                <td>
                  <div style={{ display: 'flex', alignItems: 'center', gap: 10 }}>
                    <div className="user-avatar" style={{ width: 30, height: 30, fontSize: 11, flexShrink: 0 }}>
                      {memberInitials(m.display_name, m.email)}
                    </div>
                    <div>
                      <div style={{ fontWeight: 600 }}>{m.display_name || m.email}</div>
                      <div style={{ fontSize: 11, color: 'var(--fg-muted)' }}>{m.email}</div>
                    </div>
                  </div>
                </td>
                <td><span className={badge.className}>{badge.label}</span></td>
                <td style={{ color: 'var(--fg-muted)', fontFamily: 'var(--font-mono)', fontSize: 12 }}>
                  {m.created_at ? new Date(m.created_at).toLocaleDateString() : '—'}
                </td>
                {canManage && (
                  <td style={{ textAlign: 'right' }}>
                    <button
                      className="icon-btn icon-btn--danger"
                      title="Remove member"
                      aria-label="Remove member"
                      disabled={busy}
                      onClick={() => onRemove(m)}
                    >
                      <I.trash size={13} />
                    </button>
                  </td>
                )}
              </tr>
            )
          })}
        </tbody>
      </table>
    </div>
  )
}
