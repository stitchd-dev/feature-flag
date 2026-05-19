/**
 * OrgSwitcher — sidebar component that displays the current organisation and
 * lets the user switch to any org they belong to without re-entering credentials.
 *
 * Uses the server-fetched org list (stored after login) and the SwitchOrg RPC
 * to obtain a fresh JWT for the target org seamlessly.
 */
import { useState } from 'react'
import { useNavigate } from 'react-router-dom'
import { I } from '../components/icons'
import { Dropdown } from '../components/Dropdown'
import { auth } from '../lib/auth'
import { switchOrg } from '../lib/api'

// ── helpers ───────────────────────────────────────────────────────────────────

function initials(name: string | undefined | null): string {
  if (!name) return '?'
  return name
    .split(/\s+/)
    .slice(0, 2)
    .map((w) => w[0]?.toUpperCase() ?? '')
    .join('')
}

// ── OrgSwitcher ───────────────────────────────────────────────────────────────

interface OrgSwitcherProps {
  currentOrgId: string
  currentOrgName?: string
}

export function OrgSwitcher({ currentOrgId, currentOrgName }: OrgSwitcherProps) {
  const navigate = useNavigate()
  const [open, setOpen] = useState(false)
  const [switchingOrgId, setSwitchingOrgId] = useState<string | null>(null)

  const orgs = auth.getOrgs()
  const otherOrgs = orgs.filter((o) => o.org_id !== currentOrgId)

  const displayName =
    currentOrgName ??
    orgs.find((o) => o.org_id === currentOrgId)?.org_name ??
    currentOrgId
  const abbr = initials(displayName)

  async function handleSelectOrg(orgId: string) {
    setSwitchingOrgId(orgId)
    try {
      const res = await switchOrg(orgId)
      const isSystem = auth.decodeIsSystem(res.access_token)
      const roles = auth.decodeRoles(res.access_token)
      const permissions = auth.decodePermissions(res.access_token)
      const currentUserId = auth.getSession()?.userId ?? ''
      auth.setSession({ token: res.access_token, refreshToken: res.refresh_token, orgId: res.org_id, isSystem, userId: currentUserId, roles, permissions })
      auth.setOrgs(auth.getOrgs())
      if (isSystem) {
        navigate('/superadmin')
      } else {
        navigate(`/org/${res.org_id}`)
      }
    } catch (err) {
      console.error('org switch failed', err)
    } finally {
      setSwitchingOrgId(null)
    }
  }

  const trigger = (
    <button
      className="org-switcher button-reset"
      title={`Current org: ${displayName}`}
      onClick={() => setOpen((v) => !v)}
    >
      <div className="org-avatar">{abbr || '??'}</div>
      <div className="org-meta">
        <div className="org-name" style={{ overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap', maxWidth: 120 }}>
          {displayName}
        </div>
        <div className="org-sub" style={{ overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap', maxWidth: 120 }}>
          {currentOrgId}
        </div>
      </div>
      <I.chevronDown size={14} className="org-chevron" style={{ transform: open ? 'rotate(180deg)' : undefined, transition: 'transform 0.15s' }} />
    </button>
  )

  return (
    <Dropdown
      trigger={trigger}
      isOpen={open}
      onOpenChange={setOpen}
      panelStyle={{ left: 0, right: 0 }}
    >
      {otherOrgs.length === 0 ? (
        <div style={{ padding: '12px 14px', fontSize: 12, color: 'var(--fg-muted)' }}>
          No other organisations
        </div>
      ) : (
        <>
          <div style={{ padding: '6px 14px 4px', fontSize: 10, color: 'var(--fg-subtle)', textTransform: 'uppercase', letterSpacing: '0.05em' }}>
            Switch to
          </div>
          {otherOrgs.map((org) => {
            const loading = switchingOrgId === org.org_id
            return (
              <button
                key={org.org_id}
                onClick={() => { void handleSelectOrg(org.org_id) }}
                disabled={loading || switchingOrgId !== null}
                className="sidebar-link dropdown-item"
                style={{
                  display: 'flex', alignItems: 'center', gap: 10,
                  width: '100%', padding: '9px 14px',
                  cursor: loading ? 'wait' : 'pointer',
                  color: 'var(--fg)',
                  opacity: switchingOrgId !== null && !loading ? 0.5 : 1,
                }}
              >
                <div className="org-avatar" style={{ width: 24, height: 24, fontSize: 10, borderRadius: 5, flexShrink: 0 }}>
                  {loading ? '…' : initials(org.org_name)}
                </div>
                <div>
                  <div style={{ fontSize: 13, fontWeight: 500 }}>{org.org_name}</div>
                  <div style={{ fontSize: 11, color: 'var(--fg-muted)' }}>{org.org_id}</div>
                </div>
              </button>
            )
          })}
        </>
      )}
    </Dropdown>
  )
}
