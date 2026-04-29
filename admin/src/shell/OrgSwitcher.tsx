/**
 * OrgSwitcher — sidebar component that displays the current organisation and
 * lets the user switch to any org in their history.
 *
 * Switching requires re-authentication because the JWT is org-scoped.  The
 * user must enter their password to obtain a new token for the target org.
 */
import { useState, useRef, useEffect } from 'react'
import { useNavigate } from 'react-router-dom'
import { I } from '../components/icons'
import { auth } from '../lib/auth'
import { loginWithPassword } from '../lib/api'
import type { OrgHistoryEntry } from '../lib/auth'

// ── helpers ───────────────────────────────────────────────────────────────────

function initials(name: string): string {
  return name
    .split(/\s+/)
    .slice(0, 2)
    .map((w) => w[0]?.toUpperCase() ?? '')
    .join('')
}

// ── Re-login modal ────────────────────────────────────────────────────────────

interface SwitchModalProps {
  targetOrg: OrgHistoryEntry
  onClose: () => void
  onSuccess: () => void
}

function SwitchModal({ targetOrg, onClose, onSuccess }: SwitchModalProps) {
  const navigate = useNavigate()
  const session = auth.getSession()
  const [email, setEmail] = useState('')
  const [password, setPassword] = useState('')
  const [showPw, setShowPw] = useState(false)
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState<string | null>(null)

  // Pre-fill with current user's email if available (we don't store it, so leave blank)
  useEffect(() => {
    if (session) {
      // session.userId is a UUID; we don't store email in session → leave blank
    }
  }, [session])

  async function handleSwitch(e: React.FormEvent) {
    e.preventDefault()
    setLoading(true)
    setError(null)
    try {
      const res = await loginWithPassword(email.trim(), password, targetOrg.orgId)
      const isSystem = auth.decodeIsSystem(res.access_token)
      auth.setSession({ token: res.access_token, orgId: res.org_id, isSystem, userId: res.user_id })
      auth.addOrgToHistory({ orgId: res.org_id, orgName: targetOrg.orgName })
      onSuccess()
      if (isSystem) navigate('/superadmin')
      else navigate(`/org/${res.org_id}`)
    } catch {
      setError('Login failed — please check your credentials.')
    } finally {
      setLoading(false)
    }
  }

  return (
    <div
      style={{
        position: 'fixed', inset: 0, zIndex: 200,
        background: 'rgba(0,0,0,0.45)',
        display: 'flex', alignItems: 'center', justifyContent: 'center',
      }}
      onClick={onClose}
    >
      <div
        style={{
          background: 'var(--surface)', borderRadius: 12,
          padding: 28, width: 380, boxShadow: 'var(--shadow-xl)',
          border: '1px solid var(--border)',
        }}
        onClick={(e) => e.stopPropagation()}
      >
        <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'center', marginBottom: 20 }}>
          <div>
            <div style={{ fontWeight: 600, fontSize: 16 }}>Switch Organisation</div>
            <div style={{ fontSize: 12, color: 'var(--fg-muted)', marginTop: 2 }}>
              Re-authenticate to access <strong>{targetOrg.orgName}</strong>
            </div>
          </div>
          <button className="icon-btn" onClick={onClose}><I.chevronDown size={14} /></button>
        </div>

        <form onSubmit={handleSwitch} style={{ display: 'flex', flexDirection: 'column', gap: 14 }}>
          <div>
            <label className="field-label">Email</label>
            <input
              className="input"
              type="email"
              value={email}
              onChange={(e) => setEmail(e.target.value)}
              placeholder="your@email.com"
              required
              autoFocus
              disabled={loading}
            />
          </div>
          <div>
            <label className="field-label">Password</label>
            <div style={{ position: 'relative' }}>
              <input
                className="input"
                type={showPw ? 'text' : 'password'}
                value={password}
                onChange={(e) => setPassword(e.target.value)}
                placeholder="••••••••"
                required
                disabled={loading}
                style={{ paddingRight: 38 }}
              />
              <button
                type="button"
                onClick={() => setShowPw((v) => !v)}
                style={{ position: 'absolute', right: 10, top: '50%', transform: 'translateY(-50%)', background: 'none', border: 'none', cursor: 'pointer', color: 'var(--fg-muted)' }}
              >
                {showPw ? <I.eyeOff size={14} /> : <I.eye size={14} />}
              </button>
            </div>
          </div>

          {error && <div className="alert error" style={{ fontSize: 12 }}>{error}</div>}

          <div style={{ display: 'flex', gap: 8 }}>
            <button className="btn primary" type="submit" disabled={loading || !email || !password} style={{ flex: 1 }}>
              {loading ? 'Signing in…' : `Switch to ${targetOrg.orgName}`}
            </button>
            <button className="btn" type="button" onClick={onClose} disabled={loading}>
              Cancel
            </button>
          </div>
        </form>
      </div>
    </div>
  )
}

// ── OrgSwitcher ───────────────────────────────────────────────────────────────

interface OrgSwitcherProps {
  currentOrgId: string
  currentOrgName?: string
}

export function OrgSwitcher({ currentOrgId, currentOrgName }: OrgSwitcherProps) {
  const [open, setOpen] = useState(false)
  const [modalTarget, setModalTarget] = useState<OrgHistoryEntry | null>(null)
  const dropRef = useRef<HTMLDivElement>(null)
  const history = auth.getOrgHistory()
  const otherOrgs = history.filter((o) => o.orgId !== currentOrgId)

  const displayName = currentOrgName ?? history.find((o) => o.orgId === currentOrgId)?.orgName ?? currentOrgId
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

  function handleSelectOrg(org: OrgHistoryEntry) {
    setOpen(false)
    setModalTarget(org)
  }

  return (
    <>
      {/* Trigger */}
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
                No other organisations in history
              </div>
            ) : (
              <>
                <div style={{ padding: '6px 14px 4px', fontSize: 10, color: 'var(--fg-subtle)', textTransform: 'uppercase', letterSpacing: '0.05em' }}>
                  Switch to
                </div>
                {otherOrgs.map((org) => (
                  <button
                    key={org.orgId}
                    onClick={() => handleSelectOrg(org)}
                    style={{
                      display: 'flex', alignItems: 'center', gap: 10,
                      width: '100%', padding: '9px 14px',
                      background: 'none', border: 'none', cursor: 'pointer', textAlign: 'left',
                      color: 'var(--fg)',
                    }}
                    className="sidebar-link"
                  >
                    <div className="org-avatar" style={{ width: 24, height: 24, fontSize: 10, borderRadius: 5, flexShrink: 0 }}>
                      {initials(org.orgName)}
                    </div>
                    <div>
                      <div style={{ fontSize: 13, fontWeight: 500 }}>{org.orgName}</div>
                      <div style={{ fontSize: 11, color: 'var(--fg-muted)' }}>{org.orgId}</div>
                    </div>
                  </button>
                ))}
              </>
            )}
          </div>
        )}
      </div>

      {/* Re-auth modal */}
      {modalTarget && (
        <SwitchModal
          targetOrg={modalTarget}
          onClose={() => setModalTarget(null)}
          onSuccess={() => setModalTarget(null)}
        />
      )}
    </>
  )
}
