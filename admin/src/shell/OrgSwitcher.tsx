/**
 * OrgSwitcher — sidebar component that displays the current organisation and
 * lets the user switch to any org they belong to without re-entering credentials.
 *
 * Uses the server-fetched org list (stored after login) and the SwitchOrg RPC
 * to obtain a fresh JWT for the target org seamlessly.
 */
import { useState, useRef, useEffect } from 'react'
import { useNavigate } from 'react-router-dom'
import { I } from '../components/icons'
import { auth } from '../lib/auth'
import { switchOrg } from '../lib/api'

// ── helpers ───────────────────────────────────────────────────────────────────

function initials(name: string): string {
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
  const dropRef = useRef<HTMLDivElement>(null)

  const orgs = auth.getOrgs()
  const otherOrgs = orgs.filter((o) => o.org_id !== currentOrgId)

  const displayName =
    currentOrgName ??
    orgs.find((o) => o.org_id === currentOrgId)?.org_name ??
    currentOrgId
  const abbr = initials(displayName)

  // Close dropdown on outside click
  useEffect(() => {
    function onOutside(e: MouseEvent) {
      if (dropRef.current && !dropRef.current.contains(e.target as Node)) {
        setOpen(false)
      }
    }
    if (open) document.addEventListener('mousedown', onOutside)
    return () => document.removeEventListener('mousedown', onOutside)
  }, [open])

  async function handleSelectOrg(orgId: string) {
    setOpen(false)
    setSwitchingOrgId(orgId)
    try {
      const res = await switchOrg(orgId)
      const isSystem = auth.decodeIsSystem(res.access_token)
      const roles = auth.decodeRoles(res.access_token)
      const permissions = auth.decodePermissions(res.access_token)
      // Preserve userId from current session — SwitchOrgResponse has no user_id field
      const currentUserId = auth.getSession()?.userId ?? ''
      auth.setSession({ token: res.access_token, orgId: res.org_id, isSystem, userId: currentUserId, roles, permissions })
      // Refresh the org list with the new token context
      auth.setOrgs(auth.getOrgs())
      if (isSystem) {
        navigate('/superadmin')
      } else {
        navigate(`/org/${res.org_id}`)
      }
    } catch (err) {
      console.error('org switch failed', err)
      // User stays on current org — no crash
    } finally {
      setSwitchingOrgId(null)
    }
  }

  return (
    <div ref={dropRef} style={{ position: 'relative' }}>
      <button
        className="org-switcher"
        onClick={() => setOpen((v) => !v)}
        style={{ width: '100%', cursor: 'pointer', background: 'none', border: 'none', padding: 0, textAlign: 'left' }}
        title={`Current org: ${displayName}`}
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

      {/* Dropdown */}
      {open && (
        <div style={{
          position: 'absolute', top: '100%', left: 0, right: 0, zIndex: 100,
          background: 'var(--surface)', border: '1px solid var(--border)',
          borderRadius: 8, boxShadow: 'var(--shadow-md)', overflow: 'hidden',
          marginTop: 4,
        }}>
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
                    onClick={() => handleSelectOrg(org.org_id)}
                    disabled={loading || switchingOrgId !== null}
                    style={{
                      display: 'flex', alignItems: 'center', gap: 10,
                      width: '100%', padding: '9px 14px',
                      background: 'none', border: 'none', cursor: loading ? 'wait' : 'pointer', textAlign: 'left',
                      color: 'var(--fg)',
                      opacity: switchingOrgId !== null && !loading ? 0.5 : 1,
                    }}
                    className="sidebar-link"
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
        </div>
      )}
    </div>
  )
}
