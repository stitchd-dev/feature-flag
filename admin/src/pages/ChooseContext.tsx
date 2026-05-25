/**
 * ChooseContext — superadmin landing page when the user is a member of
 * one or more non-system organisations.
 *
 * Shows two kinds of choices:
 *   1. Manage Platform (Superadmin) — keep the current is_system=true token,
 *      land on /superadmin/orgs.
 *   2. Enter <Org Name> — call switchOrg(org_id); the auth-service issues a
 *      fresh JWT with is_system reflecting the target org (false for any
 *      non-system org), then we land on /org/{orgId}.
 *
 * For regular org users this page is a no-op redirect to /org/{orgId}.
 * For superadmins with no non-system memberships, it's a no-op redirect to
 * /superadmin.
 */
import { useState } from 'react'
import { Navigate, useNavigate } from 'react-router-dom'
import { auth } from '../lib/auth'
import { switchOrg } from '../lib/api'
import { extractErrorMessage } from '../lib/errors'
import { StitchdMark } from '../components/primitives'
import { ErrorBanner } from '../components/ErrorBanner'
import { I } from '../components/icons'

export function ChooseContext() {
  const navigate = useNavigate()
  const session = auth.getSession()
  const orgs = auth.getOrgs()
  const [busy, setBusy] = useState<string | null>(null)
  const [error, setError] = useState<string | null>(null)

  if (!session) return <Navigate to="/login" replace />
  // Non-superadmins shouldn't see this page.
  if (!session.isSystem) return <Navigate to={`/org/${session.orgId}`} replace />
  // Superadmin with no non-system org memberships — skip the chooser.
  if (orgs.length === 0) return <Navigate to="/superadmin" replace />

  function pickSuperadmin() {
    navigate('/superadmin')
  }

  async function pickOrg(orgId: string, orgName: string) {
    setBusy(orgId)
    setError(null)
    try {
      const res = await switchOrg(orgId)
      const isSystem = auth.decodeIsSystem(res.access_token)
      const roles = auth.decodeRoles(res.access_token)
      const permissions = auth.decodePermissions(res.access_token)
      const name = auth.decodeName(res.access_token)
      auth.setSession({
        token: res.access_token,
        refreshToken: res.refresh_token,
        orgId: res.org_id,
        isSystem,
        userId: session!.userId,
        email: session!.email,
        name,
        roles,
        permissions,
      })
      auth.addOrgToHistory({ orgId: res.org_id, orgName })
      navigate(`/org/${res.org_id}`)
    } catch (err: unknown) {
      setError(extractErrorMessage(err))
      setBusy(null)
    }
  }

  return (
    <div style={{ minHeight: '100vh', display: 'flex', flexDirection: 'column', alignItems: 'center', justifyContent: 'center', padding: 40, background: 'var(--bg)' }}>
      <div style={{ display: 'flex', alignItems: 'center', gap: 12, marginBottom: 32 }}>
        <StitchdMark size={36} />
        <div style={{ fontSize: 22, fontWeight: 600 }}>Stitchd</div>
      </div>

      <div style={{ maxWidth: 720, width: '100%' }}>
        <h1 style={{ fontSize: 28, fontWeight: 600, marginBottom: 8, textAlign: 'center' }}>
          Where would you like to go?
        </h1>
        <p style={{ fontSize: 14, color: 'var(--fg-muted)', marginBottom: 32, textAlign: 'center' }}>
          You're signed in as a platform superadmin and a member of {orgs.length} organisation{orgs.length === 1 ? '' : 's'}. Choose your context.
        </p>

        {error && (
          <div style={{ marginBottom: 16 }}>
            <ErrorBanner message={error} onDismiss={() => setError(null)} />
          </div>
        )}

        <div style={{ display: 'grid', gap: 12 }}>
          {/* Superadmin tile */}
          <button
            type="button"
            onClick={pickSuperadmin}
            disabled={!!busy}
            className="card"
            style={{
              display: 'flex', alignItems: 'center', gap: 16,
              padding: '18px 20px', textAlign: 'left', cursor: 'pointer',
              border: '1px solid var(--border)', background: 'var(--bg-card)',
              borderRadius: 10, transition: 'border-color 0.15s, background 0.15s',
              opacity: busy ? 0.6 : 1,
            }}
            onMouseEnter={(e) => { if (!busy) (e.currentTarget as HTMLButtonElement).style.borderColor = 'var(--accent)' }}
            onMouseLeave={(e) => { (e.currentTarget as HTMLButtonElement).style.borderColor = 'var(--border)' }}
          >
            <div style={{ width: 40, height: 40, borderRadius: 8, background: 'var(--accent)', color: '#fff', display: 'flex', alignItems: 'center', justifyContent: 'center' }}>
              <I.shield size={20} />
            </div>
            <div style={{ flex: 1 }}>
              <div style={{ fontSize: 15, fontWeight: 600, marginBottom: 2 }}>Manage Platform (Superadmin)</div>
              <div style={{ fontSize: 13, color: 'var(--fg-muted)' }}>Manage all organisations and platform settings.</div>
            </div>
            <I.chevronRight size={16} style={{ color: 'var(--fg-muted)' }} />
          </button>

          {/* Org tiles */}
          {orgs.map((org) => (
            <button
              key={org.org_id}
              type="button"
              onClick={() => pickOrg(org.org_id, org.org_name)}
              disabled={!!busy}
              className="card"
              style={{
                display: 'flex', alignItems: 'center', gap: 16,
                padding: '18px 20px', textAlign: 'left', cursor: 'pointer',
                border: '1px solid var(--border)', background: 'var(--bg-card)',
                borderRadius: 10, transition: 'border-color 0.15s, background 0.15s',
                opacity: busy && busy !== org.org_id ? 0.5 : 1,
              }}
              onMouseEnter={(e) => { if (!busy) (e.currentTarget as HTMLButtonElement).style.borderColor = 'var(--accent)' }}
              onMouseLeave={(e) => { (e.currentTarget as HTMLButtonElement).style.borderColor = 'var(--border)' }}
            >
              <div style={{ width: 40, height: 40, borderRadius: 8, background: 'var(--bg)', border: '1px solid var(--border)', color: 'var(--fg)', display: 'flex', alignItems: 'center', justifyContent: 'center', fontSize: 13, fontWeight: 600 }}>
                {org.org_name.slice(0, 2).toUpperCase()}
              </div>
              <div style={{ flex: 1 }}>
                <div style={{ fontSize: 15, fontWeight: 600, marginBottom: 2 }}>{org.org_name}</div>
                <div style={{ fontSize: 12, color: 'var(--fg-muted)' }}>
                  {busy === org.org_id ? 'Switching…' : `Enter as ${org.role.replace('_', ' ')}`}
                </div>
              </div>
              <I.chevronRight size={16} style={{ color: 'var(--fg-muted)' }} />
            </button>
          ))}
        </div>

        <div style={{ marginTop: 24, textAlign: 'center', fontSize: 12, color: 'var(--fg-muted)' }}>
          You can change context later from the org switcher in the sidebar, or by signing out and back in.
        </div>
      </div>
    </div>
  )
}
