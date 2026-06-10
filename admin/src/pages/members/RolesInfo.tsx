import { I } from '../../components/icons'
import { ROLE_MODEL } from './membersHelpers'

/**
 * Honest description of the platform's fixed RBAC model. The backend has no
 * custom-role-definition API — roles are the two-value `org_admin`/`org_member`
 * enum — so this tab documents that reality instead of pretending custom roles
 * are configurable.
 */
export function RolesInfo() {
  return (
    <div className="card">
      <div className="card-header">
        <div className="card-title"><I.shield size={14} /> Organisation roles</div>
      </div>
      <div className="card-body">
        <p style={{ fontSize: 12, color: 'var(--fg-muted)', marginTop: 0, marginBottom: 16 }}>
          Roles are assigned per member when they are added. The platform uses a
          fixed two-role model — there are no custom role definitions.
        </p>
        <div style={{ display: 'flex', flexDirection: 'column', gap: 12 }}>
          {ROLE_MODEL.map((r) => (
            <div
              key={r.role}
              style={{ display: 'flex', gap: 12, padding: '12px 14px', border: '1px solid var(--border-faint)', borderRadius: 8 }}
            >
              <span className={r.role === 'org_admin' ? 'badge accent' : 'badge'} style={{ height: 'fit-content' }}>
                {r.label}
              </span>
              <div style={{ flex: 1 }}>
                <div style={{ fontFamily: 'var(--font-mono)', fontSize: 11, color: 'var(--fg-subtle)', marginBottom: 4 }}>{r.role}</div>
                <div style={{ fontSize: 13, color: 'var(--fg-muted)', lineHeight: 1.5 }}>{r.summary}</div>
              </div>
            </div>
          ))}
        </div>
      </div>
    </div>
  )
}
